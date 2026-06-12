//! BHC package database management.
//!
//! This module manages a filesystem-based per-version package database
//! used during native BHC compilation. Package `.conf` files are stored
//! directly in the database directory. Packages are registered, queried,
//! and cached through this interface.

use crate::native::BhcNativeError;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// A BHC package database backed by the filesystem.
///
/// Each database is version-specific and stored under a cache directory.
/// Package `.conf` files live directly inside the database directory.
pub struct BhcPackageDb {
    /// Path to the package database directory.
    pub path: PathBuf,
    /// BHC version this database is associated with.
    pub bhc_version: String,
    /// Set of registered package identifiers.
    pub(crate) registered: HashSet<String>,
}

impl BhcPackageDb {
    /// Open or create a package database for the given BHC version.
    ///
    /// The database is stored at `<cache_dir>/bhc-<version>/package.db`.
    /// If the directory does not exist it will be created.
    pub async fn open(bhc_version: &str, cache_dir: &Path) -> Result<Self, BhcNativeError> {
        let db_path = cache_dir
            .join(format!("bhc-{}", bhc_version))
            .join("package.db");

        info!("Opening BHC package DB at {}", db_path.display());

        let mut db = Self {
            path: db_path,
            bhc_version: bhc_version.to_string(),
            registered: HashSet::new(),
        };

        // Create the directory if needed
        if !db.path.exists() {
            db.init().await?;
        }

        db.load_registered().await?;

        Ok(db)
    }

    /// Initialize the package database directory.
    async fn init(&self) -> Result<(), BhcNativeError> {
        debug!("Initializing package DB at {}", self.path.display());

        tokio::fs::create_dir_all(&self.path)
            .await
            .map_err(|e| BhcNativeError::Io {
                message: format!(
                    "failed to create package DB directory: {}",
                    self.path.display()
                ),
                source: e,
            })?;

        Ok(())
    }

    /// Load the list of registered packages by scanning `.conf` files.
    async fn load_registered(&mut self) -> Result<(), BhcNativeError> {
        debug!("Loading registered packages from {}", self.path.display());

        self.registered.clear();

        let mut entries = match tokio::fs::read_dir(&self.path).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("Package DB directory does not exist yet");
                return Ok(());
            }
            Err(e) => {
                return Err(BhcNativeError::Io {
                    message: format!(
                        "failed to read package DB directory: {}",
                        self.path.display()
                    ),
                    source: e,
                });
            }
        };

        while let Some(entry) = entries.next_entry().await.map_err(|e| BhcNativeError::Io {
            message: format!("failed to read directory entry in {}", self.path.display()),
            source: e,
        })? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "conf") {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        if let Some(id) = extract_package_id(&content) {
                            self.registered.insert(id);
                        } else if let Some(stem) = path.file_stem() {
                            self.registered.insert(stem.to_string_lossy().to_string());
                        }
                    }
                    Err(e) => {
                        debug!("Failed to read conf file {}: {}", path.display(), e);
                    }
                }
            }
        }

        debug!("Loaded {} registered packages", self.registered.len());

        Ok(())
    }

    /// Register a package from a `.conf` file into the database.
    ///
    /// Copies the `.conf` file into the database directory and extracts
    /// the package identifier from it.
    pub async fn register(&mut self, conf_file: &Path) -> Result<String, BhcNativeError> {
        info!("Registering package from {}", conf_file.display());

        // Read the conf file to extract the package id
        let content =
            tokio::fs::read_to_string(conf_file)
                .await
                .map_err(|e| BhcNativeError::Io {
                    message: format!("failed to read conf file: {}", conf_file.display()),
                    source: e,
                })?;

        let package_id = extract_package_id(&content).unwrap_or_else(|| {
            conf_file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        });

        // Ensure the database directory exists
        tokio::fs::create_dir_all(&self.path)
            .await
            .map_err(|e| BhcNativeError::Io {
                message: format!(
                    "failed to create package DB directory: {}",
                    self.path.display()
                ),
                source: e,
            })?;

        // Copy the conf file into the database directory
        let dest = self.path.join(format!("{}.conf", package_id));
        tokio::fs::copy(conf_file, &dest)
            .await
            .map_err(|e| BhcNativeError::Io {
                message: format!(
                    "failed to copy conf file to {}: {}",
                    dest.display(),
                    conf_file.display()
                ),
                source: e,
            })?;

        self.registered.insert(package_id.clone());

        Ok(package_id)
    }

    /// Check whether a package identifier is registered.
    pub fn is_registered(&self, package_id: &str) -> bool {
        self.registered.contains(package_id)
    }

    /// Check whether a package with a given name and version is registered.
    ///
    /// Looks for entries matching `<name>-<version>` as a prefix.
    pub fn has_package(&self, name: &str, version: &str) -> bool {
        let prefix = format!("{}-{}", name, version);
        self.registered.iter().any(|id| id.starts_with(&prefix))
    }

    /// Return the set of all registered package identifiers.
    pub fn registered_packages(&self) -> &HashSet<String> {
        &self.registered
    }

    /// Recache the package database by re-scanning `.conf` files.
    pub async fn recache(&mut self) -> Result<(), BhcNativeError> {
        debug!("Recaching package DB at {}", self.path.display());
        self.load_registered().await
    }

    /// Return the path to the package database directory.
    pub fn db_path(&self) -> &Path {
        &self.path
    }

    /// Return BHC flags that point to this package database.
    ///
    /// Returns `["--package-db", "<path>"]`.
    pub fn bhc_flags(&self) -> Vec<String> {
        vec![
            "--package-db".to_string(),
            self.path.to_string_lossy().to_string(),
        ]
    }
}

/// Extract the `id:` field from a GHC/BHC package `.conf` file.
///
/// Looks for a line starting with `id:` and returns the value.
fn extract_package_id(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("id:") {
            let id = rest.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Return the default cache directory for hx.
///
/// Uses the platform cache directory via `directories::BaseDirs` and
/// appends `hx`. Falls back to `~/.cache/hx` if unavailable.
pub fn default_cache_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.cache_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("hx")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_package_id() {
        let content = "\
name: base
version: 4.19.0
id: base-4.19.0-abcdef1234
key: base-4.19.0-abcdef1234
";
        assert_eq!(
            extract_package_id(content),
            Some("base-4.19.0-abcdef1234".to_string())
        );
    }

    #[test]
    fn test_extract_package_id_missing() {
        let content = "\
name: base
version: 4.19.0
key: base-4.19.0-abcdef1234
";
        assert_eq!(extract_package_id(content), None);
    }

    #[test]
    fn test_extract_package_id_empty_value() {
        let content = "id:   \nname: foo\n";
        assert_eq!(extract_package_id(content), None);
    }

    #[test]
    fn test_default_cache_dir() {
        let dir = default_cache_dir();
        let dir_str = dir.to_string_lossy();
        assert!(
            dir_str.ends_with("hx"),
            "expected path ending with 'hx', got: {}",
            dir_str
        );
    }

    #[test]
    fn test_bhc_flags() {
        let db = BhcPackageDb {
            path: PathBuf::from("/tmp/test/bhc-2026.2.0/package.db"),
            bhc_version: "2026.2.0".to_string(),
            registered: HashSet::new(),
        };

        let flags = db.bhc_flags();
        assert_eq!(flags.len(), 2);
        assert_eq!(flags[0], "--package-db");
        assert_eq!(flags[1], "/tmp/test/bhc-2026.2.0/package.db");
    }

    #[test]
    fn test_is_registered() {
        let mut registered = HashSet::new();
        registered.insert("base-4.19.0-abc123".to_string());
        registered.insert("text-2.1.0-def456".to_string());

        let db = BhcPackageDb {
            path: PathBuf::from("/tmp/test/package.db"),
            bhc_version: "2026.2.0".to_string(),
            registered,
        };

        assert!(db.is_registered("base-4.19.0-abc123"));
        assert!(!db.is_registered("containers-0.7.0-xyz"));
    }

    #[test]
    fn test_has_package() {
        let mut registered = HashSet::new();
        registered.insert("base-4.19.0-abc123".to_string());
        registered.insert("text-2.1.0-def456".to_string());

        let db = BhcPackageDb {
            path: PathBuf::from("/tmp/test/package.db"),
            bhc_version: "2026.2.0".to_string(),
            registered,
        };

        assert!(db.has_package("base", "4.19.0"));
        assert!(db.has_package("text", "2.1.0"));
        assert!(!db.has_package("containers", "0.7.0"));
    }

    #[tokio::test]
    async fn test_register_and_load() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_dir = temp.path().join("package.db");
        std::fs::create_dir_all(&db_dir).unwrap();

        let mut db = BhcPackageDb {
            path: db_dir.clone(),
            bhc_version: "2026.2.0".to_string(),
            registered: HashSet::new(),
        };

        // Create a conf file
        let conf_content = "name: test-pkg\nversion: 1.0.0\nid: test-pkg-1.0.0-abc123\n";
        let conf_path = temp.path().join("test-pkg.conf");
        std::fs::write(&conf_path, conf_content).unwrap();

        // Register it
        let id = db.register(&conf_path).await.unwrap();
        assert_eq!(id, "test-pkg-1.0.0-abc123");
        assert!(db.is_registered("test-pkg-1.0.0-abc123"));

        // Verify the conf file was copied
        assert!(db_dir.join("test-pkg-1.0.0-abc123.conf").exists());

        // Load from scratch
        let mut db2 = BhcPackageDb {
            path: db_dir,
            bhc_version: "2026.2.0".to_string(),
            registered: HashSet::new(),
        };
        db2.load_registered().await.unwrap();
        assert!(db2.is_registered("test-pkg-1.0.0-abc123"));
    }
}
