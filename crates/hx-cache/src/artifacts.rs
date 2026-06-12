//! Binary artifact caching for compiled Haskell modules.
//!
//! This module provides content-addressed storage for compiled artifacts (.o, .hi files).
//! Artifacts are keyed by a hash of:
//! - Source file content
//! - GHC version and flags
//! - Dependency versions
//!
//! This enables sharing compiled artifacts across projects with identical inputs.

use hx_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, info};

/// Directory name for artifact cache.
const ARTIFACTS_DIR: &str = "artifacts";

/// Index file name.
const INDEX_FILE: &str = "index.json";

/// Metadata about a cached artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactEntry {
    /// The hash key for this artifact.
    pub hash: String,
    /// Module name (e.g., "Data.List").
    pub module_name: String,
    /// Source file hash.
    pub source_hash: String,
    /// GHC version used to compile.
    pub ghc_version: String,
    /// GHC flags used.
    pub ghc_flags: Vec<String>,
    /// Dependencies with versions.
    pub dependencies: HashMap<String, String>,
    /// Size in bytes of all artifacts.
    pub size_bytes: u64,
    /// When the artifact was created.
    pub created_at: u64,
    /// When the artifact was last accessed.
    pub last_accessed: u64,
    /// Files stored (relative paths within artifact dir).
    pub files: Vec<String>,
}

/// Index of all cached artifacts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtifactIndex {
    /// Map of hash -> artifact entry.
    pub entries: HashMap<String, ArtifactEntry>,
    /// Total size of all artifacts in bytes.
    pub total_size: u64,
}

impl ArtifactIndex {
    /// Load the artifact index from disk.
    pub fn load(cache_dir: &Path) -> Result<Self> {
        let index_path = cache_dir.join(ARTIFACTS_DIR).join(INDEX_FILE);

        if !index_path.exists() {
            return Ok(Self::default());
        }

        let file = File::open(&index_path).map_err(|e| Error::Io {
            message: "failed to open artifact index".to_string(),
            path: Some(index_path.clone()),
            source: e,
        })?;

        let reader = BufReader::new(file);
        serde_json::from_reader(reader).map_err(|e| Error::Config {
            message: format!("failed to parse artifact index: {}", e),
            path: Some(index_path),
            source: None,
            fixes: vec![],
        })
    }

    /// Save the artifact index to disk.
    pub fn save(&self, cache_dir: &Path) -> Result<()> {
        let artifacts_dir = cache_dir.join(ARTIFACTS_DIR);
        fs::create_dir_all(&artifacts_dir).map_err(|e| Error::Io {
            message: "failed to create artifacts directory".to_string(),
            path: Some(artifacts_dir.clone()),
            source: e,
        })?;

        let index_path = artifacts_dir.join(INDEX_FILE);
        let content = serde_json::to_vec_pretty(self).map_err(|e| Error::Config {
            message: format!("failed to serialize artifact index: {}", e),
            path: Some(index_path.clone()),
            source: None,
            fixes: vec![],
        })?;

        hx_core::atomic_write(&index_path, content).map_err(|e| Error::Io {
            message: "failed to write artifact index".to_string(),
            path: Some(index_path),
            source: e,
        })
    }

    /// Get an artifact by hash.
    pub fn get(&self, hash: &str) -> Option<&ArtifactEntry> {
        self.entries.get(hash)
    }

    /// Add an artifact entry.
    pub fn add(&mut self, entry: ArtifactEntry) {
        self.total_size += entry.size_bytes;
        self.entries.insert(entry.hash.clone(), entry);
    }

    /// Remove an artifact by hash.
    pub fn remove(&mut self, hash: &str) -> Option<ArtifactEntry> {
        if let Some(entry) = self.entries.remove(hash) {
            self.total_size = self.total_size.saturating_sub(entry.size_bytes);
            Some(entry)
        } else {
            None
        }
    }

    /// Get statistics about the artifact cache.
    pub fn stats(&self) -> ArtifactStats {
        ArtifactStats {
            entry_count: self.entries.len(),
            total_size: self.total_size,
        }
    }
}

/// Statistics about the artifact cache.
#[derive(Debug, Clone)]
pub struct ArtifactStats {
    /// Number of entries in the cache.
    pub entry_count: usize,
    /// Total size in bytes.
    pub total_size: u64,
}

impl ArtifactStats {
    /// Format the total size in human-readable format.
    pub fn size_string(&self) -> String {
        format_size(self.total_size)
    }
}

/// Compute the artifact hash from inputs.
pub fn compute_artifact_hash(
    source_hash: &str,
    ghc_version: &str,
    ghc_flags: &[String],
    dependencies: &HashMap<String, String>,
) -> String {
    let mut hasher = Sha256::new();

    hasher.update(source_hash.as_bytes());
    hasher.update(b"|");
    hasher.update(ghc_version.as_bytes());
    hasher.update(b"|");

    // Sort flags for determinism
    let mut flags: Vec<_> = ghc_flags.iter().collect();
    flags.sort();
    for flag in flags {
        hasher.update(flag.as_bytes());
        hasher.update(b";");
    }
    hasher.update(b"|");

    // Sort dependencies for determinism
    let mut deps: Vec<_> = dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| *k);
    for (name, version) in deps {
        hasher.update(name.as_bytes());
        hasher.update(b"=");
        hasher.update(version.as_bytes());
        hasher.update(b";");
    }

    format!("{:x}", hasher.finalize())
}

/// Compute hash of a file's contents.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| Error::Io {
        message: "failed to open file for hashing".to_string(),
        path: Some(path.to_path_buf()),
        source: e,
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file.read(&mut buffer).map_err(|e| Error::Io {
            message: "failed to read file for hashing".to_string(),
            path: Some(path.to_path_buf()),
            source: e,
        })?;

        if bytes_read == 0 {
            break;
        }

        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Store artifacts for a module.
pub fn store_artifacts(
    cache_dir: &Path,
    module_name: &str,
    source_hash: &str,
    ghc_version: &str,
    ghc_flags: &[String],
    dependencies: &HashMap<String, String>,
    artifact_files: &[PathBuf],
) -> Result<String> {
    let hash = compute_artifact_hash(source_hash, ghc_version, ghc_flags, dependencies);
    let artifact_dir = cache_dir.join(ARTIFACTS_DIR).join(&hash[..2]).join(&hash);

    // Create artifact directory
    fs::create_dir_all(&artifact_dir).map_err(|e| Error::Io {
        message: "failed to create artifact directory".to_string(),
        path: Some(artifact_dir.clone()),
        source: e,
    })?;

    let mut files = Vec::new();
    let mut total_size = 0u64;

    // Copy artifact files
    for src_path in artifact_files {
        if let Some(filename) = src_path.file_name() {
            let dest_path = artifact_dir.join(filename);
            fs::copy(src_path, &dest_path).map_err(|e| Error::Io {
                message: format!("failed to copy artifact: {}", src_path.display()),
                path: Some(src_path.clone()),
                source: e,
            })?;

            if let Ok(metadata) = fs::metadata(&dest_path) {
                total_size += metadata.len();
            }

            files.push(filename.to_string_lossy().to_string());
        }
    }

    // Update index
    let mut index = ArtifactIndex::load(cache_dir)?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let entry = ArtifactEntry {
        hash: hash.clone(),
        module_name: module_name.to_string(),
        source_hash: source_hash.to_string(),
        ghc_version: ghc_version.to_string(),
        ghc_flags: ghc_flags.to_vec(),
        dependencies: dependencies.clone(),
        size_bytes: total_size,
        created_at: now,
        last_accessed: now,
        files,
    };

    index.add(entry);
    index.save(cache_dir)?;

    debug!(
        "Stored artifacts for {} ({} bytes)",
        module_name, total_size
    );

    Ok(hash)
}

/// Retrieve artifacts for a module.
pub fn retrieve_artifacts(
    cache_dir: &Path,
    source_hash: &str,
    ghc_version: &str,
    ghc_flags: &[String],
    dependencies: &HashMap<String, String>,
    dest_dir: &Path,
) -> Result<Option<Vec<PathBuf>>> {
    let hash = compute_artifact_hash(source_hash, ghc_version, ghc_flags, dependencies);

    let mut index = ArtifactIndex::load(cache_dir)?;

    let entry = match index.get(&hash) {
        Some(e) => e.clone(),
        None => return Ok(None),
    };

    let artifact_dir = cache_dir.join(ARTIFACTS_DIR).join(&hash[..2]).join(&hash);

    if !artifact_dir.exists() {
        // Index is stale, remove entry
        index.remove(&hash);
        index.save(cache_dir)?;
        return Ok(None);
    }

    // Copy files to destination
    let mut copied_files = Vec::new();
    for filename in &entry.files {
        let src_path = artifact_dir.join(filename);
        let dest_path = dest_dir.join(filename);

        if src_path.exists() {
            fs::copy(&src_path, &dest_path).map_err(|e| Error::Io {
                message: format!("failed to copy artifact: {}", src_path.display()),
                path: Some(src_path.clone()),
                source: e,
            })?;
            copied_files.push(dest_path);
        }
    }

    // Update last accessed time
    if let Some(entry) = index.entries.get_mut(&hash) {
        entry.last_accessed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }
    index.save(cache_dir)?;

    debug!("Retrieved artifacts for hash {}", &hash[..8]);

    Ok(Some(copied_files))
}

/// Prune artifacts older than the given age in days.
pub fn prune_artifacts(cache_dir: &Path, max_age_days: u64) -> Result<PruneResult> {
    let mut index = ArtifactIndex::load(cache_dir)?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let max_age_secs = max_age_days * 24 * 60 * 60;
    let cutoff = now.saturating_sub(max_age_secs);

    let mut to_remove = Vec::new();
    for (hash, entry) in &index.entries {
        if entry.last_accessed < cutoff {
            to_remove.push(hash.clone());
        }
    }

    let mut removed_count = 0;
    let mut removed_size = 0u64;

    for hash in to_remove {
        if let Some(entry) = index.remove(&hash) {
            removed_size += entry.size_bytes;
            removed_count += 1;

            // Remove files
            let artifact_dir = cache_dir.join(ARTIFACTS_DIR).join(&hash[..2]).join(&hash);
            if artifact_dir.exists() {
                let _ = fs::remove_dir_all(&artifact_dir);
            }
        }
    }

    index.save(cache_dir)?;

    info!(
        "Pruned {} artifacts ({} freed)",
        removed_count,
        format_size(removed_size)
    );

    Ok(PruneResult {
        removed_count,
        removed_size,
    })
}

/// Result of pruning operation.
#[derive(Debug, Clone)]
pub struct PruneResult {
    /// Number of artifacts removed.
    pub removed_count: usize,
    /// Bytes freed.
    pub removed_size: u64,
}

/// Clear all artifacts.
pub fn clear_artifacts(cache_dir: &Path) -> Result<()> {
    let artifacts_dir = cache_dir.join(ARTIFACTS_DIR);

    if artifacts_dir.exists() {
        fs::remove_dir_all(&artifacts_dir).map_err(|e| Error::Io {
            message: "failed to clear artifacts".to_string(),
            path: Some(artifacts_dir),
            source: e,
        })?;
    }

    info!("Cleared artifact cache");

    Ok(())
}

/// Format bytes in human-readable format.
fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.2} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} bytes", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_compute_artifact_hash() {
        let hash1 = compute_artifact_hash(
            "abc123",
            "9.8.2",
            &["-O2".to_string()],
            &HashMap::from([("base".to_string(), "4.18.0".to_string())]),
        );

        let hash2 = compute_artifact_hash(
            "abc123",
            "9.8.2",
            &["-O2".to_string()],
            &HashMap::from([("base".to_string(), "4.18.0".to_string())]),
        );

        assert_eq!(hash1, hash2);

        // Different source should produce different hash
        let hash3 = compute_artifact_hash(
            "def456",
            "9.8.2",
            &["-O2".to_string()],
            &HashMap::from([("base".to_string(), "4.18.0".to_string())]),
        );

        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_artifact_index() {
        let temp = tempdir().unwrap();
        let cache_dir = temp.path();

        // Empty index
        let index = ArtifactIndex::load(cache_dir).unwrap();
        assert_eq!(index.entries.len(), 0);

        // Add entry
        let mut index = index;
        index.add(ArtifactEntry {
            hash: "abc123".to_string(),
            module_name: "Test".to_string(),
            source_hash: "src123".to_string(),
            ghc_version: "9.8.2".to_string(),
            ghc_flags: vec![],
            dependencies: HashMap::new(),
            size_bytes: 1000,
            created_at: 0,
            last_accessed: 0,
            files: vec!["Test.o".to_string()],
        });

        index.save(cache_dir).unwrap();

        // Reload
        let reloaded = ArtifactIndex::load(cache_dir).unwrap();
        assert_eq!(reloaded.entries.len(), 1);
        assert!(reloaded.get("abc123").is_some());
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 bytes");
        assert_eq!(format_size(1500), "1.50 KB");
        assert_eq!(format_size(1_500_000), "1.50 MB");
        assert_eq!(format_size(1_500_000_000), "1.50 GB");
    }
}
