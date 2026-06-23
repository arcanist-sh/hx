//! Package database management for native builds.
//!
//! This module handles GHC package database operations:
//! - Creating and initializing package databases
//! - Registering compiled packages
//! - Querying package availability
//! - Recaching the database

use hx_core::{Error, Fix, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// A GHC package database.
///
/// Package databases store metadata about installed packages, including
/// their locations, dependencies, and exposed modules. This allows GHC
/// to find and link against installed packages.
#[derive(Debug, Clone)]
pub struct PackageDb {
    /// Path to the package database directory
    pub path: PathBuf,
    /// GHC version this database is for
    pub ghc_version: String,
    /// Path to ghc-pkg executable
    pub ghc_pkg_path: PathBuf,
    /// Set of registered package IDs (cached)
    registered: HashSet<String>,
}

impl PackageDb {
    /// Open or create a package database for the given GHC version.
    ///
    /// The database is stored at `~/.cache/hx/ghc-<version>/package.db/`.
    /// If the database doesn't exist, it will be initialized.
    pub async fn open(ghc_version: &str, cache_dir: &Path) -> Result<Self> {
        let db_path = cache_dir
            .join(format!("ghc-{}", ghc_version))
            .join("package.db");

        // Find ghc-pkg
        let ghc_pkg_path = find_ghc_pkg(ghc_version).await?;

        let mut db = Self {
            path: db_path,
            ghc_version: ghc_version.to_string(),
            ghc_pkg_path,
            registered: HashSet::new(),
        };

        // Initialize if needed
        if !db.path.exists() {
            db.init().await?;
        }

        // Load registered packages
        db.load_registered().await?;

        Ok(db)
    }

    /// Initialize a new package database.
    async fn init(&self) -> Result<()> {
        info!("Initializing package database at {}", self.path.display());

        // Create parent directories
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                message: "failed to create package database directory".to_string(),
                path: Some(parent.to_path_buf()),
                source: e,
            })?;
        }

        // Run ghc-pkg init
        let output = Command::new(&self.ghc_pkg_path)
            .arg("init")
            .arg(&self.path)
            .output()
            .await
            .map_err(|e| Error::Io {
                message: "failed to run ghc-pkg init".to_string(),
                path: Some(self.ghc_pkg_path.clone()),
                source: e,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::CommandFailed {
                command: "ghc-pkg init".to_string(),
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: stderr.to_string(),
                fixes: vec![],
            });
        }

        Ok(())
    }

    /// Load the set of registered package IDs.
    async fn load_registered(&mut self) -> Result<()> {
        self.registered.clear();

        if !self.path.exists() {
            return Ok(());
        }

        let output = Command::new(&self.ghc_pkg_path)
            .arg("list")
            .arg("--package-db")
            .arg(&self.path)
            .arg("--simple-output")
            .output()
            .await
            .map_err(|e| Error::Io {
                message: "failed to run ghc-pkg list".to_string(),
                path: Some(self.ghc_pkg_path.clone()),
                source: e,
            })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for pkg_id in stdout.split_whitespace() {
                self.registered.insert(pkg_id.to_string());
            }
        }

        debug!(
            "Loaded {} registered packages from {}",
            self.registered.len(),
            self.path.display()
        );

        Ok(())
    }

    /// Register a package from its .conf file.
    ///
    /// Returns the package ID that was registered.
    pub async fn register(&mut self, conf_file: &Path) -> Result<String> {
        info!("Registering package from {}", conf_file.display());

        // Read the conf file to extract package-id
        let content = std::fs::read_to_string(conf_file).map_err(|e| Error::Io {
            message: "failed to read package conf file".to_string(),
            path: Some(conf_file.to_path_buf()),
            source: e,
        })?;

        let package_id = extract_package_id(&content).ok_or_else(|| Error::Config {
            message: format!(
                "could not find package-id in conf file: {}",
                conf_file.display()
            ),
            path: Some(conf_file.to_path_buf()),
            source: None,
            fixes: vec![],
        })?;

        // Check if already registered
        if self.registered.contains(&package_id) {
            debug!("Package {} already registered", package_id);
            return Ok(package_id);
        }

        // Run ghc-pkg register
        let output = Command::new(&self.ghc_pkg_path)
            .arg("register")
            .arg("--package-db")
            .arg(&self.path)
            .arg("--force") // Allow re-registration
            .arg(conf_file)
            .output()
            .await
            .map_err(|e| Error::Io {
                message: "failed to run ghc-pkg register".to_string(),
                path: Some(self.ghc_pkg_path.clone()),
                source: e,
            })?;

        if !output.status.success() {
            // ghc-pkg exits non-zero when a package with this unit-id is already
            // registered, even with `--force`. Our unit-ids are content-derived
            // and deterministic, so an existing registration of the same id is
            // already correct — treat "already exists" as success (idempotent
            // re-registration across repeated native builds). `--simple-output`
            // in `load_registered` reports the bare name-version, not the full
            // unit-id, so the early-return guard above cannot catch this.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let already_registered = stderr.contains("already exist")
                || stderr.contains("already installed");
            if !already_registered {
                return Err(Error::CommandFailed {
                    command: "ghc-pkg register".to_string(),
                    exit_code: output.status.code(),
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: stderr.to_string(),
                    fixes: vec![],
                });
            }
            debug!("Package {} already registered (ghc-pkg)", package_id);
        }

        // Update cache
        self.registered.insert(package_id.clone());

        info!("Registered package: {}", package_id);
        Ok(package_id)
    }

    /// Check if a package ID is registered.
    pub fn is_registered(&self, package_id: &str) -> bool {
        self.registered.contains(package_id)
    }

    /// Check if a package (by name and version) is registered.
    pub fn has_package(&self, name: &str, version: &str) -> bool {
        let prefix = format!("{}-{}", name, version);
        self.registered.iter().any(|id| id.starts_with(&prefix))
    }

    /// Get all registered package IDs.
    pub fn registered_packages(&self) -> &HashSet<String> {
        &self.registered
    }

    /// Recache the package database.
    ///
    /// This updates the package database cache files after modifications.
    pub async fn recache(&self) -> Result<()> {
        debug!("Recaching package database at {}", self.path.display());

        let output = Command::new(&self.ghc_pkg_path)
            .arg("recache")
            .arg("--package-db")
            .arg(&self.path)
            .output()
            .await
            .map_err(|e| Error::Io {
                message: "failed to run ghc-pkg recache".to_string(),
                path: Some(self.ghc_pkg_path.clone()),
                source: e,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("ghc-pkg recache warning: {}", stderr);
        }

        Ok(())
    }

    /// Unregister a package.
    pub async fn unregister(&mut self, package_id: &str) -> Result<()> {
        if !self.registered.contains(package_id) {
            return Ok(());
        }

        let output = Command::new(&self.ghc_pkg_path)
            .arg("unregister")
            .arg("--package-db")
            .arg(&self.path)
            .arg("--force")
            .arg(package_id)
            .output()
            .await
            .map_err(|e| Error::Io {
                message: "failed to run ghc-pkg unregister".to_string(),
                path: Some(self.ghc_pkg_path.clone()),
                source: e,
            })?;

        if !output.status.success() {
            return Err(Error::CommandFailed {
                command: "ghc-pkg unregister".to_string(),
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                fixes: vec![],
            });
        }

        self.registered.remove(package_id);
        Ok(())
    }

    /// Get package database path for use with GHC's -package-db flag.
    pub fn db_path(&self) -> &Path {
        &self.path
    }

    /// Get the GHC package flags for using this database.
    ///
    /// Returns flags like `-package-db /path/to/db`.
    pub fn ghc_flags(&self) -> Vec<String> {
        vec![
            "-package-db".to_string(),
            self.path.to_string_lossy().to_string(),
        ]
    }

    /// Describe a package (get its full info).
    pub async fn describe(&self, package_id: &str) -> Result<String> {
        let output = Command::new(&self.ghc_pkg_path)
            .arg("describe")
            .arg("--package-db")
            .arg(&self.path)
            .arg(package_id)
            .output()
            .await
            .map_err(|e| Error::Io {
                message: "failed to run ghc-pkg describe".to_string(),
                path: Some(self.ghc_pkg_path.clone()),
                source: e,
            })?;

        if !output.status.success() {
            return Err(Error::CommandFailed {
                command: format!("ghc-pkg describe {}", package_id),
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                fixes: vec![],
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Get the library directories for a package.
    pub async fn library_dirs(&self, package_id: &str) -> Result<Vec<PathBuf>> {
        let desc = self.describe(package_id).await?;
        let mut dirs = Vec::new();

        for line in desc.lines() {
            let line = line.trim();
            if line.starts_with("library-dirs:") {
                let paths = line.strip_prefix("library-dirs:").unwrap_or("").trim();
                for path in paths.split_whitespace() {
                    dirs.push(PathBuf::from(path));
                }
            }
        }

        Ok(dirs)
    }
}

/// Find the ghc-pkg executable for a specific GHC version.
async fn find_ghc_pkg(ghc_version: &str) -> Result<PathBuf> {
    // Try versioned ghc-pkg first
    let versioned = format!("ghc-pkg-{}", ghc_version);
    if let Ok(path) = which_async(&versioned).await {
        return Ok(path);
    }

    // Fall back to plain ghc-pkg
    if let Ok(path) = which_async("ghc-pkg").await {
        // Verify version matches
        let output = Command::new(&path)
            .arg("--version")
            .output()
            .await
            .map_err(|e| Error::Io {
                message: "failed to check ghc-pkg version".to_string(),
                path: Some(path.clone()),
                source: e,
            })?;

        if output.status.success() {
            let version_output = String::from_utf8_lossy(&output.stdout);
            // GHC package manager version 9.8.2
            if version_output.contains(ghc_version) {
                return Ok(path);
            }
        }

        // Return it anyway as fallback
        return Ok(path);
    }

    Err(Error::ToolchainMissing {
        tool: format!("ghc-pkg (for GHC {})", ghc_version),
        source: None,
        fixes: vec![Fix::with_command(
            "Install GHC via ghcup",
            format!("ghcup install ghc {}", ghc_version),
        )],
    })
}

/// Find an executable in PATH.
async fn which_async(name: &str) -> Result<PathBuf> {
    let output = Command::new("which")
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| Error::Io {
            message: format!("failed to run which {}", name),
            path: None,
            source: e,
        })?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }

    Err(Error::ToolchainMissing {
        tool: name.to_string(),
        source: None,
        fixes: vec![],
    })
}

/// Extract the package-id from a .conf file content.
fn extract_package_id(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(id) = line.strip_prefix("id:") {
            return Some(id.trim().to_string());
        }
    }
    None
}

/// Clear a package database.
pub async fn clear_package_db(cache_dir: &Path, ghc_version: &str) -> Result<()> {
    let db_path = cache_dir
        .join(format!("ghc-{}", ghc_version))
        .join("package.db");

    if db_path.exists() {
        std::fs::remove_dir_all(&db_path).map_err(|e| Error::Io {
            message: "failed to remove package database".to_string(),
            path: Some(db_path),
            source: e,
        })?;
    }

    Ok(())
}

/// Get the default cache directory for hx.
pub fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("hx")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_package_id() {
        let content = r#"
name:                 text
version:              2.1.1
id:                   text-2.1.1-abc123def456
key:                  text-2.1.1-abc123def456
license:              BSD-2-Clause
"#;
        let id = extract_package_id(content);
        assert_eq!(id, Some("text-2.1.1-abc123def456".to_string()));
    }

    #[test]
    fn test_extract_package_id_missing() {
        let content = r#"
name:                 text
version:              2.1.1
"#;
        let id = extract_package_id(content);
        assert_eq!(id, None);
    }

    #[test]
    fn test_default_cache_dir() {
        let dir = default_cache_dir();
        assert!(dir.ends_with("hx"));
    }

    #[test]
    fn test_ghc_flags() {
        let db = PackageDb {
            path: PathBuf::from("/home/user/.cache/hx/ghc-9.8.2/package.db"),
            ghc_version: "9.8.2".to_string(),
            ghc_pkg_path: PathBuf::from("/usr/bin/ghc-pkg"),
            registered: HashSet::new(),
        };
        let flags = db.ghc_flags();
        assert_eq!(flags.len(), 2);
        assert_eq!(flags[0], "-package-db");
        assert!(flags[1].contains("package.db"));
    }
}
