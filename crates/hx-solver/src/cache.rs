//! Caching for package index and resolution results.
//!
//! This module provides:
//! - Binary caching of the Hackage package index for fast loading
//! - Caching of resolution results keyed by dependency fingerprint

use crate::package::{InstallPlan, PackageIndex};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
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
    DecodeError(#[from] postcard::Error),

    #[error("cache version mismatch")]
    VersionMismatch,

    #[error("cache is stale")]
    Stale,
}

/// Current cache format version. Bumped to 2 when the index began evaluating
/// `.cabal` conditionals, which makes the parsed deps context-dependent, and
/// to 3 when the encoding moved from bincode to postcard. A version-2 bincode
/// header still decodes as version 2 under postcard (`02 00 00 00` reads as
/// the varint 2), so the bump is what makes old caches rejected, not misread.
const CACHE_VERSION: u32 = 3;

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

/// Encode a cache file: the header, then the payload, back to back.
fn encode_cached<T: serde::Serialize>(
    header: &CacheHeader,
    payload: &T,
) -> Result<Vec<u8>, CacheError> {
    let mut bytes = postcard::to_stdvec(header)?;
    bytes.extend(postcard::to_stdvec(payload)?);
    Ok(bytes)
}

/// Decode a cache file, rejecting other format versions before the payload
/// is touched.
fn decode_cached<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<(CacheHeader, T), CacheError> {
    let (header, rest): (CacheHeader, _) = postcard::take_from_bytes(bytes)?;

    if header.version != CACHE_VERSION {
        debug!(
            "Cache version mismatch: expected {}, found {}",
            CACHE_VERSION, header.version
        );
        return Err(CacheError::VersionMismatch);
    }

    Ok((header, postcard::from_bytes(rest)?))
}

/// Decode a cached index, rejecting indexes parsed for another build context.
fn decode_index(bytes: &[u8], context_key: &str) -> Result<PackageIndex, CacheError> {
    let (header, index) = decode_cached(bytes)?;

    // The cached deps were evaluated for a specific build context; a different
    // GHC/platform would yield a different dependency set.
    if header.context_key.as_deref() != Some(context_key) {
        debug!(
            "Index cache context mismatch: expected {:?}, found {:?}",
            context_key, header.context_key
        );
        return Err(CacheError::VersionMismatch);
    }

    Ok(index)
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

    let bytes = fs::read(&cache_path)?;
    let index = decode_index(&bytes, context_key)?;

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

    let mut writer = BufWriter::new(File::create(&cache_path)?);
    writer.write_all(&encode_cached(&header, index)?)?;
    writer.flush()?;

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

    let bytes = fs::read(&cache_path)?;
    let (_, plan): (_, InstallPlan) = decode_cached(&bytes)?;

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

    let mut writer = BufWriter::new(File::create(&cache_path)?);
    writer.write_all(&encode_cached(&header, plan)?)?;
    writer.flush()?;

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
    use crate::package::{Dependency, PackageVersion};

    fn test_header(version: u32, context_key: &str) -> CacheHeader {
        CacheHeader {
            version,
            created_at: 0,
            source_hash: Some("abc".to_string()),
            context_key: Some(context_key.to_string()),
        }
    }

    #[test]
    fn test_index_cache_roundtrip() {
        let mut index = PackageIndex::new();
        let mut pv = PackageVersion::new("aeson", "2.2.3.0".parse().unwrap());
        pv.add_dependency(Dependency::with_constraint(
            "base",
            crate::parse_constraint(">=4.18 && <5").unwrap(),
        ));
        index.add_package_version(pv);
        index.add_package_version(PackageVersion::new("text", "2.1".parse().unwrap()));

        let bytes = encode_cached(&test_header(CACHE_VERSION, "ghc-9.10.3"), &index).unwrap();
        let decoded = decode_index(&bytes, "ghc-9.10.3").unwrap();

        assert_eq!(decoded.package_count(), 2);
        assert_eq!(decoded.version_count(), 2);
        let aeson = decoded
            .get_package_version("aeson", &"2.2.3.0".parse().unwrap())
            .unwrap();
        assert_eq!(aeson.dependencies.len(), 1);
        assert_eq!(aeson.dependencies[0].name, "base");
    }

    #[test]
    fn test_index_cache_rejects_other_context_and_version() {
        let index = PackageIndex::new();

        let bytes = encode_cached(&test_header(CACHE_VERSION, "ghc-9.8.4"), &index).unwrap();
        assert!(matches!(
            decode_index(&bytes, "ghc-9.10.3"),
            Err(CacheError::VersionMismatch)
        ));

        let bytes = encode_cached(&test_header(CACHE_VERSION - 1, "ghc-9.10.3"), &index).unwrap();
        assert!(matches!(
            decode_index(&bytes, "ghc-9.10.3"),
            Err(CacheError::VersionMismatch)
        ));
    }

    /// A version-2 cache written by bincode must be rejected, not misread.
    #[test]
    fn test_index_cache_rejects_bincode_v2() {
        // bincode 1.x: fixed-width little-endian u32 version, then u64 timestamp
        let mut bytes = vec![2, 0, 0, 0];
        bytes.extend_from_slice(&1_700_000_000u64.to_le_bytes());
        bytes.extend_from_slice(&[0; 32]);
        assert!(decode_index(&bytes, "ghc-9.10.3").is_err());
    }

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
