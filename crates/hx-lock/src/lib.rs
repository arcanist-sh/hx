//! Lockfile management for hx.
//!
//! This crate handles:
//! - Reading and writing hx.lock
//! - Converting from Cabal freeze/plan
//! - Fingerprint calculation

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use thiserror::Error;

/// Error type for lock operations.
#[derive(Debug, Error)]
pub enum LockError {
    #[error("failed to read lockfile at {path}: {source}")]
    ReadError {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse lockfile: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("failed to serialize lockfile: {0}")]
    SerializeError(#[from] toml::ser::Error),

    #[error("lockfile version mismatch: expected {expected}, found {found}")]
    VersionMismatch { expected: u32, found: u32 },
}

/// Current lockfile format version.
pub const LOCK_VERSION: u32 = 1;

/// A locked package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPackage {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Source (hackage, workspace, git, etc.)
    #[serde(default = "default_source")]
    pub source: String,
    /// Content hash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

fn default_source() -> String {
    "hackage".to_string()
}

/// Toolchain section of the lockfile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedToolchain {
    /// GHC version
    pub ghc: Option<String>,
    /// Cabal version
    pub cabal: Option<String>,
}

/// Build plan section of the lockfile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPlan {
    /// Compiler ID (e.g., "ghc-9.8.2")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler_id: Option<String>,
    /// Target platform
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Hackage index state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_state: Option<String>,
    /// Stackage snapshot (e.g., "lts-22.28", "nightly-2024-01-15")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// Overall plan hash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// A workspace package (local package in the workspace).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspacePackageInfo {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Path relative to workspace root
    pub path: String,
}

/// Workspace section of the lockfile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedWorkspace {
    /// Whether this lockfile represents a workspace
    #[serde(default)]
    pub is_workspace: bool,
    /// Workspace packages (local packages)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<WorkspacePackageInfo>,
}

/// The hx.lock lockfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile format version
    pub version: u32,
    /// When the lockfile was created/updated
    pub created_at: DateTime<Utc>,
    /// Toolchain versions
    #[serde(default)]
    pub toolchain: LockedToolchain,
    /// Build plan metadata
    #[serde(default)]
    pub plan: LockedPlan,
    /// Workspace information (for multi-package projects)
    #[serde(default, skip_serializing_if = "is_default_workspace")]
    pub workspace: LockedWorkspace,
    /// Locked packages (external dependencies)
    #[serde(default)]
    pub packages: Vec<LockedPackage>,
}

fn is_default_workspace(w: &LockedWorkspace) -> bool {
    !w.is_workspace && w.packages.is_empty()
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

impl Lockfile {
    /// Create a new empty lockfile.
    pub fn new() -> Self {
        Self {
            version: LOCK_VERSION,
            created_at: Utc::now(),
            toolchain: LockedToolchain::default(),
            plan: LockedPlan::default(),
            workspace: LockedWorkspace::default(),
            packages: Vec::new(),
        }
    }

    /// Parse a lockfile from a TOML string.
    pub fn parse(s: &str) -> Result<Self, LockError> {
        let lock: Lockfile = toml::from_str(s)?;
        if lock.version != LOCK_VERSION {
            return Err(LockError::VersionMismatch {
                expected: LOCK_VERSION,
                found: lock.version,
            });
        }
        Ok(lock)
    }

    /// Parse a lockfile from a file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, LockError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| LockError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;
        Self::parse(&content)
    }

    /// Serialize the lockfile to a TOML string.
    pub fn to_string(&self) -> Result<String, LockError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Write the lockfile to a file (atomically, to survive interruption).
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), LockError> {
        let path = path.as_ref();
        let content = self.to_string()?;
        hx_core::atomic_write(path, content).map_err(|e| LockError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    /// Calculate a fingerprint for the lockfile.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();

        // Include toolchain
        if let Some(ref ghc) = self.toolchain.ghc {
            hasher.update(format!("ghc:{}", ghc));
        }
        if let Some(ref cabal) = self.toolchain.cabal {
            hasher.update(format!("cabal:{}", cabal));
        }

        // Include plan metadata
        if let Some(ref platform) = self.plan.platform {
            hasher.update(format!("platform:{}", platform));
        }
        if let Some(ref index_state) = self.plan.index_state {
            hasher.update(format!("index:{}", index_state));
        }
        if let Some(ref snapshot) = self.plan.snapshot {
            hasher.update(format!("snapshot:{}", snapshot));
        }

        // Include workspace packages (sorted for determinism)
        if self.workspace.is_workspace {
            hasher.update("workspace:true");
            let mut workspace_pkgs: Vec<_> = self.workspace.packages.iter().collect();
            workspace_pkgs.sort_by(|a, b| a.name.cmp(&b.name));
            for pkg in workspace_pkgs {
                hasher.update(format!("local:{}@{}:{}", pkg.name, pkg.version, pkg.path));
            }
        }

        // Include external packages (sorted for determinism)
        let mut packages: Vec<_> = self.packages.iter().collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        for pkg in packages {
            hasher.update(format!("{}@{}", pkg.name, pkg.version));
        }

        let result = hasher.finalize();
        format!("sha256:{}", hex::encode(result))
    }

    /// Add a package to the lockfile.
    pub fn add_package(&mut self, pkg: LockedPackage) {
        // Remove existing package with same name
        self.packages.retain(|p| p.name != pkg.name);
        self.packages.push(pkg);
    }

    /// Set the toolchain versions.
    pub fn set_toolchain(&mut self, ghc: Option<String>, cabal: Option<String>) {
        self.toolchain.ghc = ghc;
        self.toolchain.cabal = cabal;
    }

    /// Set the Stackage snapshot.
    pub fn set_snapshot(&mut self, snapshot: Option<String>) {
        self.plan.snapshot = snapshot;
    }

    /// Set workspace information.
    pub fn set_workspace(&mut self, packages: Vec<WorkspacePackageInfo>) {
        self.workspace.is_workspace = !packages.is_empty();
        self.workspace.packages = packages;
    }

    /// Check if this is a workspace lockfile.
    pub fn is_workspace(&self) -> bool {
        self.workspace.is_workspace
    }

    /// Get the workspace package names.
    pub fn workspace_package_names(&self) -> Vec<&str> {
        self.workspace
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect()
    }
}

/// Parse a Cabal freeze file and extract constraints.
pub fn parse_freeze_file(content: &str) -> Vec<LockedPackage> {
    let mut packages = Vec::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with("--") {
            continue;
        }

        // Look for constraint lines like "constraints: pkg ==version"
        // or "             pkg ==version,"
        let constraint = line
            .strip_prefix("constraints:")
            .or(Some(line))
            .map(|s| s.trim().trim_end_matches(','));

        if let Some(constraint) = constraint
            && let Some((name, version)) = parse_constraint(constraint)
        {
            packages.push(LockedPackage {
                name,
                version,
                source: "hackage".to_string(),
                hash: None,
            });
        }
    }

    packages
}

fn parse_constraint(s: &str) -> Option<(String, String)> {
    // Parse "pkg ==version" or "pkg ==version,"
    let s = s.trim().trim_end_matches(',');
    let parts: Vec<&str> = s.split(" ==").collect();
    if parts.len() == 2 {
        let name = parts[0].trim();
        let version = parts[1].trim();
        if !name.is_empty() && !version.is_empty() && !name.starts_with("any.") {
            return Some((name.to_string(), version.to_string()));
        }
    }
    None
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lockfile_roundtrip() {
        let mut lock = Lockfile::new();
        lock.set_toolchain(Some("9.8.2".to_string()), Some("3.12.1.0".to_string()));
        lock.add_package(LockedPackage {
            name: "text".to_string(),
            version: "2.1.1".to_string(),
            source: "hackage".to_string(),
            hash: None,
        });

        let toml = lock.to_string().unwrap();
        let parsed = Lockfile::parse(&toml).unwrap();

        assert_eq!(parsed.toolchain.ghc, Some("9.8.2".to_string()));
        assert_eq!(parsed.packages.len(), 1);
        assert_eq!(parsed.packages[0].name, "text");
    }

    #[test]
    fn test_workspace_lockfile_roundtrip() {
        let mut lock = Lockfile::new();
        lock.set_toolchain(Some("9.8.2".to_string()), Some("3.12.1.0".to_string()));

        // Set workspace packages
        lock.set_workspace(vec![
            WorkspacePackageInfo {
                name: "mylib".to_string(),
                version: "0.1.0".to_string(),
                path: "packages/mylib".to_string(),
            },
            WorkspacePackageInfo {
                name: "myapp".to_string(),
                version: "0.1.0".to_string(),
                path: "packages/myapp".to_string(),
            },
        ]);

        // Add external dependencies
        lock.add_package(LockedPackage {
            name: "text".to_string(),
            version: "2.1.1".to_string(),
            source: "hackage".to_string(),
            hash: None,
        });

        let toml = lock.to_string().unwrap();
        let parsed = Lockfile::parse(&toml).unwrap();

        assert!(parsed.is_workspace());
        assert_eq!(parsed.workspace.packages.len(), 2);
        assert_eq!(parsed.packages.len(), 1);

        let names = parsed.workspace_package_names();
        assert!(names.contains(&"mylib"));
        assert!(names.contains(&"myapp"));
    }

    #[test]
    fn test_workspace_fingerprint_includes_packages() {
        let mut lock1 = Lockfile::new();
        lock1.set_workspace(vec![WorkspacePackageInfo {
            name: "pkg1".to_string(),
            version: "0.1.0".to_string(),
            path: "packages/pkg1".to_string(),
        }]);

        let mut lock2 = Lockfile::new();
        lock2.set_workspace(vec![WorkspacePackageInfo {
            name: "pkg2".to_string(),
            version: "0.1.0".to_string(),
            path: "packages/pkg2".to_string(),
        }]);

        // Different workspace packages should produce different fingerprints
        assert_ne!(lock1.fingerprint(), lock2.fingerprint());
    }

    #[test]
    fn test_parse_constraint() {
        assert_eq!(
            parse_constraint("text ==2.1.1"),
            Some(("text".to_string(), "2.1.1".to_string()))
        );
        assert_eq!(
            parse_constraint("  aeson ==2.2.0.0,"),
            Some(("aeson".to_string(), "2.2.0.0".to_string()))
        );
        assert_eq!(parse_constraint("any.base ==4.19"), None);
    }
}
