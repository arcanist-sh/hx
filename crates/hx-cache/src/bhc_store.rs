//! BHC package build cache.
//!
//! Content-addressed cache for BHC-compiled packages, keyed by
//! package identity plus BHC-specific dimensions (profile, tensor_fusion).

use crate::global_cache_dir;
use hx_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

/// A cached BHC package entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BhcPackageCacheEntry {
    /// The package ID (e.g., "text-2.1.1-abc123")
    pub package_id: String,
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// BHC version used to compile
    pub bhc_version: String,
    /// BHC profile used (default, server, numeric, edge)
    pub profile: String,
    /// Whether tensor fusion was enabled
    pub tensor_fusion: bool,
    /// Platform
    pub platform: String,
    /// Sorted dependency package IDs
    pub dependency_ids: Vec<String>,
    /// Path to the compiled library
    pub library_path: PathBuf,
    /// Path to the registration file
    pub registration_file: PathBuf,
    /// When this package was built
    pub built_at: u64,
    /// Last time this entry was accessed
    pub last_accessed: u64,
}

/// Index for BHC package caches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BhcPackageCacheIndex {
    /// Version of the index format
    pub version: u32,
    /// Cached packages, keyed by cache key (sha256 of name, version, bhc, profile, tensor_fusion, deps)
    pub entries: HashMap<String, BhcPackageCacheEntry>,
}

impl BhcPackageCacheIndex {
    /// Current index version.
    pub const VERSION: u32 = 1;

    /// Create a new empty index.
    pub fn new() -> Self {
        Self {
            version: Self::VERSION,
            entries: HashMap::new(),
        }
    }

    /// Load the BHC package cache index from disk.
    pub fn load() -> Result<Self> {
        let path = bhc_package_cache_path()?;
        if !path.exists() {
            return Ok(Self::new());
        }

        let content = std::fs::read_to_string(&path).map_err(|e| Error::Io {
            message: "failed to read BHC package cache index".to_string(),
            path: Some(path),
            source: e,
        })?;

        let index: BhcPackageCacheIndex = serde_json::from_str(&content)
            .map_err(|e| Error::config(format!("invalid BHC package cache index: {}", e)))?;

        debug!(
            "Loaded BHC package cache with {} entries",
            index.entries.len()
        );

        Ok(index)
    }

    /// Save the BHC package cache index to disk.
    pub fn save(&self) -> Result<()> {
        let path = bhc_package_cache_path()?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                message: "failed to create BHC package cache directory".to_string(),
                path: Some(parent.to_path_buf()),
                source: e,
            })?;
        }

        let content = serde_json::to_string_pretty(self).map_err(|e| {
            Error::config(format!(
                "failed to serialize BHC package cache index: {}",
                e
            ))
        })?;

        hx_core::atomic_write(&path, content).map_err(|e| Error::Io {
            message: "failed to write BHC package cache index".to_string(),
            path: Some(path),
            source: e,
        })?;

        Ok(())
    }

    /// Get a cached BHC package entry by cache key.
    pub fn get(&self, cache_key: &str) -> Option<&BhcPackageCacheEntry> {
        self.entries.get(cache_key)
    }

    /// Record a successful BHC package build.
    pub fn record_package(&mut self, cache_key: String, entry: BhcPackageCacheEntry) {
        self.entries.insert(cache_key, entry);
    }
}

/// Get the path to the BHC package cache index file.
fn bhc_package_cache_path() -> Result<PathBuf> {
    Ok(global_cache_dir()?.join("bhc-package-cache.json"))
}

/// Calculate a cache key for a BHC package build.
///
/// The cache key is a SHA256 hash of:
/// - Package name
/// - Package version
/// - BHC version
/// - BHC profile
/// - Tensor fusion flag
/// - Sorted dependency package IDs
pub fn calculate_bhc_cache_key(
    name: &str,
    version: &str,
    bhc_version: &str,
    profile: &str,
    tensor_fusion: bool,
    dependency_ids: &[String],
) -> String {
    let mut hasher = Sha256::new();

    hasher.update(format!("name:{}", name));
    hasher.update(format!("version:{}", version));
    hasher.update(format!("bhc:{}", bhc_version));
    hasher.update(format!("profile:{}", profile));
    hasher.update(format!("tensor_fusion:{}", tensor_fusion));

    // Sort dependencies for determinism
    let mut sorted_deps = dependency_ids.to_vec();
    sorted_deps.sort();
    for dep_id in sorted_deps {
        hasher.update(format!("dep:{}", dep_id));
    }

    let result = hasher.finalize();
    format!(
        "sha256:{}",
        result
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_bhc_cache_key_deterministic() {
        let key1 = calculate_bhc_cache_key(
            "text",
            "2.1.1",
            "2026.2.0",
            "default",
            false,
            &["base-4.18.0".to_string(), "bytestring-0.12.0".to_string()],
        );

        let key2 = calculate_bhc_cache_key(
            "text",
            "2.1.1",
            "2026.2.0",
            "default",
            false,
            &["bytestring-0.12.0".to_string(), "base-4.18.0".to_string()],
        );

        // Same inputs (regardless of dep order) should produce the same key
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_calculate_bhc_cache_key_differs_by_profile() {
        let key_default = calculate_bhc_cache_key(
            "text",
            "2.1.1",
            "2026.2.0",
            "default",
            false,
            &["base-4.18.0".to_string()],
        );

        let key_numeric = calculate_bhc_cache_key(
            "text",
            "2.1.1",
            "2026.2.0",
            "numeric",
            false,
            &["base-4.18.0".to_string()],
        );

        assert_ne!(key_default, key_numeric);
    }

    #[test]
    fn test_calculate_bhc_cache_key_differs_by_tensor_fusion() {
        let key_off = calculate_bhc_cache_key(
            "text",
            "2.1.1",
            "2026.2.0",
            "default",
            false,
            &["base-4.18.0".to_string()],
        );

        let key_on = calculate_bhc_cache_key(
            "text",
            "2.1.1",
            "2026.2.0",
            "default",
            true,
            &["base-4.18.0".to_string()],
        );

        assert_ne!(key_off, key_on);
    }

    #[test]
    fn test_bhc_cache_index_operations() {
        let mut index = BhcPackageCacheIndex::new();

        let cache_key = "sha256:abc123".to_string();

        assert!(index.get(&cache_key).is_none());

        let entry = BhcPackageCacheEntry {
            package_id: "text-2.1.1-xyz789".to_string(),
            name: "text".to_string(),
            version: "2.1.1".to_string(),
            bhc_version: "2026.2.0".to_string(),
            profile: "numeric".to_string(),
            tensor_fusion: true,
            platform: "aarch64-apple-darwin".to_string(),
            dependency_ids: vec!["base-4.18.0".to_string()],
            library_path: PathBuf::from("/lib/text.a"),
            registration_file: PathBuf::from("/pkg/text.conf"),
            built_at: 1000,
            last_accessed: 1000,
        };

        index.record_package(cache_key.clone(), entry);

        let retrieved = index.get(&cache_key).unwrap();
        assert_eq!(retrieved.name, "text");
        assert_eq!(retrieved.bhc_version, "2026.2.0");
        assert_eq!(retrieved.profile, "numeric");
        assert!(retrieved.tensor_fusion);
    }
}
