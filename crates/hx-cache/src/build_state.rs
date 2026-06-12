//! Build state tracking for incremental builds.
//!
//! This module tracks the build status of packages to enable:
//! - Resuming builds after failures
//! - Skipping already-built packages
//! - Tracking build timing for optimization

use hx_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Status of a package build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageStatus {
    /// Not yet built
    Pending,
    /// Build is in progress
    Building,
    /// Successfully built
    Success,
    /// Build failed
    Failed,
}

/// Information about a package's build state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageBuildInfo {
    /// Current build status
    pub status: PackageStatus,
    /// Source fingerprint when this build was done
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    /// Build duration in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_time_secs: Option<f64>,
    /// Timestamp of last build attempt
    pub last_build: u64,
    /// Error message if build failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Build state for a project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildState {
    /// Schema version for forward compatibility
    pub version: u32,
    /// Source fingerprint this state was built against
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    /// Lock fingerprint this state was built against
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_fingerprint: Option<String>,
    /// GHC version used for this build
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ghc_version: Option<String>,
    /// Per-package build information
    #[serde(default)]
    pub packages: HashMap<String, PackageBuildInfo>,
    /// Timestamp of last build
    pub last_build: u64,
}

impl BuildState {
    /// Create a new empty build state.
    pub fn new() -> Self {
        Self {
            version: 1,
            source_fingerprint: None,
            lock_fingerprint: None,
            ghc_version: None,
            packages: HashMap::new(),
            last_build: current_timestamp(),
        }
    }

    /// Load build state from disk.
    pub fn load(project_root: &Path) -> Result<Self> {
        let path = build_state_path(project_root);

        if !path.exists() {
            return Ok(Self::new());
        }

        let content = std::fs::read_to_string(&path).map_err(|e| Error::Io {
            message: "failed to read build state".to_string(),
            path: Some(path),
            source: e,
        })?;

        let state: BuildState = serde_json::from_str(&content)
            .map_err(|e| Error::config(format!("invalid build state: {}", e)))?;

        Ok(state)
    }

    /// Save build state to disk.
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let path = build_state_path(project_root);

        // Ensure .hx directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                message: "failed to create .hx directory".to_string(),
                path: Some(parent.to_path_buf()),
                source: e,
            })?;
        }

        let content = serde_json::to_string_pretty(self)
            .map_err(|e| Error::config(format!("failed to serialize build state: {}", e)))?;

        hx_core::atomic_write(&path, content).map_err(|e| Error::Io {
            message: "failed to write build state".to_string(),
            path: Some(path),
            source: e,
        })?;

        Ok(())
    }

    /// Check if the build state is fresh (fingerprints match).
    pub fn is_fresh(&self, source_fingerprint: &str, lock_fingerprint: Option<&str>) -> bool {
        // Source fingerprint must match
        if self.source_fingerprint.as_deref() != Some(source_fingerprint) {
            return false;
        }

        // Lock fingerprint must match if provided
        if let Some(lock_fp) = lock_fingerprint
            && self.lock_fingerprint.as_deref() != Some(lock_fp)
        {
            return false;
        }

        // All packages must be successful
        self.packages
            .values()
            .all(|p| p.status == PackageStatus::Success)
    }

    /// Check if a specific package needs rebuilding.
    pub fn package_needs_rebuild(&self, package: &str, source_hash: &str) -> bool {
        match self.packages.get(package) {
            None => true, // Never built
            Some(info) => {
                // Needs rebuild if failed or source changed
                info.status != PackageStatus::Success
                    || info.source_hash.as_deref() != Some(source_hash)
            }
        }
    }

    /// Mark a package as starting to build.
    pub fn mark_building(&mut self, package: &str) {
        self.packages.insert(
            package.to_string(),
            PackageBuildInfo {
                status: PackageStatus::Building,
                source_hash: None,
                build_time_secs: None,
                last_build: current_timestamp(),
                error: None,
            },
        );
        self.last_build = current_timestamp();
    }

    /// Mark a package as successfully built.
    pub fn mark_success(&mut self, package: &str, source_hash: &str, build_time_secs: f64) {
        self.packages.insert(
            package.to_string(),
            PackageBuildInfo {
                status: PackageStatus::Success,
                source_hash: Some(source_hash.to_string()),
                build_time_secs: Some(build_time_secs),
                last_build: current_timestamp(),
                error: None,
            },
        );
        self.last_build = current_timestamp();
    }

    /// Mark a package as failed.
    pub fn mark_failed(&mut self, package: &str, error: &str) {
        self.packages.insert(
            package.to_string(),
            PackageBuildInfo {
                status: PackageStatus::Failed,
                source_hash: None,
                build_time_secs: None,
                last_build: current_timestamp(),
                error: Some(error.to_string()),
            },
        );
        self.last_build = current_timestamp();
    }

    /// Update fingerprints after a successful build.
    pub fn update_fingerprints(
        &mut self,
        source_fingerprint: &str,
        lock_fingerprint: Option<&str>,
        ghc_version: Option<&str>,
    ) {
        self.source_fingerprint = Some(source_fingerprint.to_string());
        self.lock_fingerprint = lock_fingerprint.map(|s| s.to_string());
        self.ghc_version = ghc_version.map(|s| s.to_string());
    }

    /// Get the count of packages by status.
    pub fn status_counts(&self) -> (usize, usize, usize) {
        let mut success = 0;
        let mut failed = 0;
        let mut pending = 0;

        for info in self.packages.values() {
            match info.status {
                PackageStatus::Success => success += 1,
                PackageStatus::Failed => failed += 1,
                PackageStatus::Pending | PackageStatus::Building => pending += 1,
            }
        }

        (success, failed, pending)
    }

    /// Clear all package states (for clean rebuilds).
    pub fn clear(&mut self) {
        self.packages.clear();
        self.source_fingerprint = None;
        self.lock_fingerprint = None;
    }
}

/// Path to the build state file.
fn build_state_path(project_root: &Path) -> PathBuf {
    project_root.join(".hx").join("build-state.json")
}

/// Get current timestamp as Unix seconds.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_new_build_state() {
        let state = BuildState::new();
        assert_eq!(state.version, 1);
        assert!(state.packages.is_empty());
    }

    #[test]
    fn test_mark_success() {
        let mut state = BuildState::new();
        state.mark_success("mylib", "sha256:abc123", 2.5);

        let info = state.packages.get("mylib").unwrap();
        assert_eq!(info.status, PackageStatus::Success);
        assert_eq!(info.source_hash, Some("sha256:abc123".to_string()));
        assert_eq!(info.build_time_secs, Some(2.5));
    }

    #[test]
    fn test_is_fresh() {
        let mut state = BuildState::new();
        state.update_fingerprints("sha256:source", Some("sha256:lock"), Some("9.8.2"));
        state.mark_success("pkg1", "sha256:abc", 1.0);

        // Fresh when fingerprints match
        assert!(state.is_fresh("sha256:source", Some("sha256:lock")));

        // Not fresh when source changes
        assert!(!state.is_fresh("sha256:different", Some("sha256:lock")));

        // Not fresh when lock changes
        assert!(!state.is_fresh("sha256:source", Some("sha256:different")));
    }

    #[test]
    fn test_package_needs_rebuild() {
        let mut state = BuildState::new();
        state.mark_success("pkg1", "sha256:v1", 1.0);

        // Same hash - no rebuild needed
        assert!(!state.package_needs_rebuild("pkg1", "sha256:v1"));

        // Different hash - rebuild needed
        assert!(state.package_needs_rebuild("pkg1", "sha256:v2"));

        // Unknown package - rebuild needed
        assert!(state.package_needs_rebuild("pkg2", "sha256:any"));
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        let mut state = BuildState::new();
        state.update_fingerprints("sha256:source", Some("sha256:lock"), Some("9.8.2"));
        state.mark_success("pkg1", "sha256:abc", 2.5);
        state.mark_failed("pkg2", "type error");

        state.save(root).unwrap();

        let loaded = BuildState::load(root).unwrap();
        assert_eq!(loaded.source_fingerprint, Some("sha256:source".to_string()));
        assert_eq!(loaded.ghc_version, Some("9.8.2".to_string()));
        assert_eq!(
            loaded.packages.get("pkg1").unwrap().status,
            PackageStatus::Success
        );
        assert_eq!(
            loaded.packages.get("pkg2").unwrap().status,
            PackageStatus::Failed
        );
    }

    #[test]
    fn test_status_counts() {
        let mut state = BuildState::new();
        state.mark_success("pkg1", "hash1", 1.0);
        state.mark_success("pkg2", "hash2", 1.0);
        state.mark_failed("pkg3", "error");
        state.mark_building("pkg4");

        let (success, failed, pending) = state.status_counts();
        assert_eq!(success, 2);
        assert_eq!(failed, 1);
        assert_eq!(pending, 1);
    }
}
