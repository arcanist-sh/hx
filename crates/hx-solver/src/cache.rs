//! Caching for package index and resolution results.
//!
//! This module provides:
//! - Binary caching of the Hackage package index for fast loading
//! - Caching of resolution results keyed by dependency fingerprint

use crate::package::{InstallPlan, PackageIndex};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;
use tracing::{debug, info};

/// Error type for cache operations.
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("failed to read cache file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("failed to decode cache: {0}")]
    DecodeError(#[from] bincode::Error),

    #[error("cache version mismatch")]
    VersionMismatch,

    #[error("cache is stale")]
    Stale,
}

/// Current cache format version. Bumped to 2 when the index began evaluating
/// `.cabal` conditionals, which makes the parsed deps context-dependent.
const CACHE_VERSION: u32 = 2;

/// Header for cached files to track version and source.
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheHeader {
    /// Cache format version
    version: u32,
    /// Timestamp when cache was created (unix epoch seconds)
    created_at: u64,
    /// SHA256 of the source file (for staleness check)
    source_hash: Option<String>,
    /// Build context (GHC version, OS, arch) the index was parsed for. The
    /// parsed dependency set depends on it, so a cache built for a different
    /// context must not be reused.
    context_key: Option<String>,
}

/// Get the default cache directory for hx-solver.
pub fn cache_dir() -> Option<PathBuf> {
    let home = dirs_next::home_dir()?;
    Some(home.join(".cache").join("hx").join("solver"))
}

/// Get the path for the cached package index.
pub fn index_cache_path() -> Option<PathBuf> {
    Some(cache_dir()?.join("index.bin"))
}

/// Get the path for cached resolution results.
pub fn resolution_cache_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("resolutions"))
}

/// Load a cached package index if it exists and is valid.
///
/// The cache is considered valid if:
/// - The cache file exists
/// - The cache version matches
/// - The source tar.gz hasn't been modified since the cache was created
pub fn load_cached_index(
    source_path: &Path,
    context_key: &str,
) -> Result<PackageIndex, CacheError> {
    let cache_path = index_cache_path().ok_or_else(|| {
        CacheError::ReadError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cache directory not available",
        ))
    })?;

    if !cache_path.exists() {
        return Err(CacheError::Stale);
    }

    // Check if source is newer than cache
    let source_modified = fs::metadata(source_path)?.modified()?;
    let cache_modified = fs::metadata(&cache_path)?.modified()?;

    if source_modified > cache_modified {
        debug!("Index cache is stale (source modified)");
        return Err(CacheError::Stale);
    }

    info!("Loading cached package index from {}", cache_path.display());

    let file = File::open(&cache_path)?;
    let mut reader = BufReader::new(file);

    // Read header first
    let header: CacheHeader = bincode::deserialize_from(&mut reader)?;

    if header.version != CACHE_VERSION {
        debug!(
            "Cache version mismatch: expected {}, found {}",
            CACHE_VERSION, header.version
        );
        return Err(CacheError::VersionMismatch);
    }

    // The cached deps were evaluated for a specific build context; a different
    // GHC/platform would yield a different dependency set.
    if header.context_key.as_deref() != Some(context_key) {
        debug!(
            "Index cache context mismatch: expected {:?}, found {:?}",
            context_key, header.context_key
        );
        return Err(CacheError::VersionMismatch);
    }

    // Read the index
    let index: PackageIndex = bincode::deserialize_from(&mut reader)?;

    info!(
        "Loaded {} packages with {} versions from cache",
        index.package_count(),
        index.version_count()
    );

    Ok(index)
}

/// Save a package index to the cache, tagged with the build context it was
/// parsed for.
pub fn save_index_cache(
    index: &PackageIndex,
    source_path: &Path,
    context_key: &str,
) -> Result<(), CacheError> {
    let cache_path = index_cache_path().ok_or_else(|| {
        CacheError::ReadError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cache directory not available",
        ))
    })?;

    // Ensure cache directory exists
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)?;
    }

    info!("Saving package index cache to {}", cache_path.display());

    let file = File::create(&cache_path)?;
    let mut writer = BufWriter::new(file);

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let header = CacheHeader {
        version: CACHE_VERSION,
        created_at: now,
        source_hash: Some(hash_file(source_path).unwrap_or_default()),
        context_key: Some(context_key.to_string()),
    };

    // Write header
    bincode::serialize_into(&mut writer, &header)?;

    // Write index
    bincode::serialize_into(&mut writer, index)?;

    info!(
        "Cached {} packages with {} versions",
        index.package_count(),
        index.version_count()
    );

    Ok(())
}

/// Compute a fingerprint for a set of dependencies.
///
/// The fingerprint is a SHA256 hash of the dependency names and constraints,
/// sorted for determinism.
pub fn compute_deps_fingerprint(deps: &[(String, String)]) -> String {
    let mut hasher = Sha256::new();

    // Sort for determinism
    let mut sorted_deps: Vec<_> = deps.iter().collect();
    sorted_deps.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, constraint) in sorted_deps {
        hasher.update(format!("{}:{}", name, constraint));
    }

    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Load a cached resolution result if it exists.
pub fn load_cached_resolution(fingerprint: &str) -> Result<InstallPlan, CacheError> {
    let cache_dir = resolution_cache_dir().ok_or_else(|| {
        CacheError::ReadError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cache directory not available",
        ))
    })?;

    let cache_path = cache_dir.join(format!("{}.bin", fingerprint));

    if !cache_path.exists() {
        return Err(CacheError::Stale);
    }

    debug!("Loading cached resolution for {}", fingerprint);

    let file = File::open(&cache_path)?;
    let mut reader = BufReader::new(file);

    // Read header
    let header: CacheHeader = bincode::deserialize_from(&mut reader)?;

    if header.version != CACHE_VERSION {
        return Err(CacheError::VersionMismatch);
    }

    // Read the plan
    let plan: InstallPlan = bincode::deserialize_from(&mut reader)?;

    info!("Loaded cached resolution with {} packages", plan.len());

    Ok(plan)
}

/// Save a resolution result to the cache.
pub fn save_resolution_cache(fingerprint: &str, plan: &InstallPlan) -> Result<(), CacheError> {
    let cache_dir = resolution_cache_dir().ok_or_else(|| {
        CacheError::ReadError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cache directory not available",
        ))
    })?;

    // Ensure cache directory exists
    fs::create_dir_all(&cache_dir)?;

    let cache_path = cache_dir.join(format!("{}.bin", fingerprint));

    debug!("Saving resolution cache for {}", fingerprint);

    let file = File::create(&cache_path)?;
    let mut writer = BufWriter::new(file);

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let header = CacheHeader {
        version: CACHE_VERSION,
        created_at: now,
        source_hash: None,
        // Resolution caches are keyed by the dependency fingerprint in the
        // filename, not by build context.
        context_key: None,
    };

    // Write header
    bincode::serialize_into(&mut writer, &header)?;

    // Write plan
    bincode::serialize_into(&mut writer, plan)?;

    info!("Cached resolution with {} packages", plan.len());

    Ok(())
}

/// Clear the resolution cache.
pub fn clear_resolution_cache() -> Result<(), CacheError> {
    let cache_dir = resolution_cache_dir().ok_or_else(|| {
        CacheError::ReadError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cache directory not available",
        ))
    })?;

    if cache_dir.exists() {
        fs::remove_dir_all(&cache_dir)?;
    }

    Ok(())
}

/// Clear the index cache.
pub fn clear_index_cache() -> Result<(), CacheError> {
    let cache_path = index_cache_path().ok_or_else(|| {
        CacheError::ReadError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cache directory not available",
        ))
    })?;

    if cache_path.exists() {
        fs::remove_file(&cache_path)?;
    }

    Ok(())
}

/// Compute SHA256 hash of a file.
fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();

    std::io::copy(&mut reader, &mut hasher)?;

    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_deterministic() {
        let deps = vec![
            ("base".to_string(), ">= 4.0".to_string()),
            ("text".to_string(), ">= 2.0".to_string()),
        ];

        let fp1 = compute_deps_fingerprint(&deps);
        let fp2 = compute_deps_fingerprint(&deps);

        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_order_independent() {
        let deps1 = vec![
            ("base".to_string(), ">= 4.0".to_string()),
            ("text".to_string(), ">= 2.0".to_string()),
        ];

        let deps2 = vec![
            ("text".to_string(), ">= 2.0".to_string()),
            ("base".to_string(), ">= 4.0".to_string()),
        ];

        let fp1 = compute_deps_fingerprint(&deps1);
        let fp2 = compute_deps_fingerprint(&deps2);

        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_changes_with_deps() {
        let deps1 = vec![("base".to_string(), ">= 4.0".to_string())];

        let deps2 = vec![("base".to_string(), ">= 5.0".to_string())];

        let fp1 = compute_deps_fingerprint(&deps1);
        let fp2 = compute_deps_fingerprint(&deps2);

        assert_ne!(fp1, fp2);
    }
}
