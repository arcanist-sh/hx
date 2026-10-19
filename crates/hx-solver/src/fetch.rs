//! Native package fetching from Hackage.
//!
//! This module provides direct downloading of packages from Hackage,
//! replacing the need for `cabal fetch`.

use crate::package::InstallPlan;
use crate::version::Version;
use futures::stream::{self, StreamExt};
use hx_core::hash::{Digest, Sha256, sha256_file, to_hex};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Client;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::{debug, info};

/// Error type for fetch operations.
#[derive(Debug, Error)]
pub enum FetchError {
    #[error("failed to create directory: {0}")]
    CreateDir(#[from] io::Error),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("hash mismatch for {package}: expected sha256 {expected}, got {actual}")]
    HashMismatch {
        package: String,
        expected: String,
        actual: String,
    },

    #[error("package not found: {0}")]
    NotFound(String),

    #[error("invalid package version {version} for {package}")]
    InvalidVersion { package: String, version: String },
}

/// Result of fetching a package.
#[derive(Debug, Clone)]
pub struct FetchResult {
    /// Package name
    pub name: String,
    /// Package version
    pub version: Version,
    /// Path to downloaded tarball
    pub path: PathBuf,
    /// SHA256 hash of the tarball
    pub hash: String,
    /// Whether the package was already cached
    pub cached: bool,
}

/// Options for fetching packages.
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Maximum parallel downloads
    pub jobs: usize,
    /// Directory to store downloaded packages
    pub cache_dir: PathBuf,
    /// Whether to verify hashes
    pub verify_hashes: bool,
    /// HTTP timeout in seconds
    pub timeout: u64,
    /// Base URL of the Hackage mirror to download from. `file://` URLs
    /// (a local mirror with Hackage's `package/<id>/<id>.tar.gz` layout)
    /// are read directly from disk.
    pub base_url: String,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            jobs: 8,
            cache_dir: default_package_cache_dir().unwrap_or_else(|| PathBuf::from(".hx/packages")),
            verify_hashes: true,
            timeout: 60,
            base_url: HACKAGE_BASE_URL.to_string(),
        }
    }
}

/// Get the default package cache directory (see [`hx_cache::package_store_dir`]).
pub fn default_package_cache_dir() -> Option<PathBuf> {
    hx_cache::package_store_dir().ok()
}

/// Default package download location.
pub const HACKAGE_BASE_URL: &str = "https://hackage.haskell.org";

/// Hackage package URL.
fn hackage_url(name: &str, version: &Version) -> String {
    format!(
        "https://hackage.haskell.org/package/{name}-{version}/{name}-{version}.tar.gz",
        name = name,
        version = version
    )
}

/// Hackage package hash URL.
fn hackage_hash_url(name: &str, version: &Version) -> String {
    format!(
        "https://hackage.haskell.org/package/{name}-{version}/{name}-{version}.tar.gz.sha256",
        name = name,
        version = version
    )
}

/// Fetch packages from Hackage.
pub async fn fetch_packages(
    plan: &InstallPlan,
    options: &FetchOptions,
) -> Result<Vec<FetchResult>, FetchError> {
    // Ensure cache directory exists
    fs::create_dir_all(&options.cache_dir)?;

    let client = hx_core::http::builder()
        .timeout(Duration::from_secs(options.timeout))
        .build()?;

    let semaphore = Arc::new(Semaphore::new(options.jobs));
    let client = Arc::new(client);
    let options = Arc::new(options.clone());

    // Set up progress bars
    let multi_progress = MultiProgress::new();
    let overall_pb = multi_progress.add(ProgressBar::new(plan.packages.len() as u64));
    overall_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} packages")
            .unwrap()
            .progress_chars("#>-"),
    );

    // Filter out base packages that don't need fetching
    let packages_to_fetch: Vec<_> = plan
        .packages
        .iter()
        .filter(|p| !is_base_package(&p.name))
        .collect();

    let results: Vec<Result<FetchResult, FetchError>> = stream::iter(packages_to_fetch)
        .map(|pkg| {
            let client = Arc::clone(&client);
            let semaphore = Arc::clone(&semaphore);
            let options = Arc::clone(&options);
            let overall_pb = overall_pb.clone();
            let multi_progress = multi_progress.clone();

            async move {
                let _permit = semaphore.acquire().await.unwrap();

                let result = fetch_single_package(
                    &client,
                    &pkg.name,
                    &pkg.version,
                    &options,
                    &multi_progress,
                )
                .await;

                overall_pb.inc(1);
                result
            }
        })
        .buffer_unordered(options.jobs)
        .collect()
        .await;

    overall_pb.finish_with_message("Done");

    // Collect results, propagating first error
    let mut fetch_results = Vec::new();
    for result in results {
        fetch_results.push(result?);
    }

    Ok(fetch_results)
}

/// Check if a package is a base package (comes with GHC).
fn is_base_package(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "ghc-prim"
            | "ghc-bignum"
            | "integer-gmp"
            | "integer-simple"
            | "template-haskell"
            | "ghc-boot"
            | "ghc-boot-th"
            | "ghc-heap"
            | "ghci"
            | "hpc"
            | "transformers"
            | "array"
            | "binary"
            | "bytestring"
            | "containers"
            | "deepseq"
            | "directory"
            | "exceptions"
            | "filepath"
            | "mtl"
            | "parsec"
            | "pretty"
            | "process"
            | "stm"
            | "text"
            | "time"
            | "unix"
            | "Win32"
    )
}

/// Fetch a single package.
async fn fetch_single_package(
    client: &Client,
    name: &str,
    version: &Version,
    options: &FetchOptions,
    multi_progress: &MultiProgress,
) -> Result<FetchResult, FetchError> {
    let tarball_path = options
        .cache_dir
        .join(format!("{}-{}.tar.gz", name, version));

    // Check if already cached
    if tarball_path.exists() {
        debug!("Package {}-{} already cached", name, version);

        // Compute hash of existing file
        let hash = compute_file_hash(&tarball_path)?;

        return Ok(FetchResult {
            name: name.to_string(),
            version: version.clone(),
            path: tarball_path,
            hash,
            cached: true,
        });
    }

    let url = hackage_url(name, version);
    debug!("Fetching {} from {}", name, url);

    // Create progress bar for this download
    let pb = multi_progress.add(ProgressBar::new(0));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("  {spinner:.green} {msg} [{bar:30.cyan/blue}] {bytes}/{total_bytes}")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.set_message(format!("{}-{}", name, version));

    // Start download
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        pb.finish_and_clear();
        return Err(FetchError::NotFound(format!("{}-{}", name, version)));
    }

    // Get content length for progress
    if let Some(content_length) = response.content_length() {
        pb.set_length(content_length);
    }

    // Download to temp file first
    let temp_path = tarball_path.with_extension("tmp");
    let mut file = File::create(&temp_path)?;
    let mut hasher = Sha256::new();

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        pb.inc(chunk.len() as u64);
    }

    pb.finish_and_clear();

    // Compute hash
    let hash = to_hex(hasher.finalize());

    // Verify hash if enabled
    if options.verify_hashes
        && let Ok(expected_hash) = fetch_expected_hash(client, name, version).await
        && hash != expected_hash
    {
        // Remove temp file
        let _ = fs::remove_file(&temp_path);
        return Err(FetchError::HashMismatch {
            package: format!("{}-{}", name, version),
            expected: expected_hash,
            actual: hash,
        });
    }

    // Rename temp file to final location
    fs::rename(&temp_path, &tarball_path)?;

    info!("Downloaded {}-{}", name, version);

    Ok(FetchResult {
        name: name.to_string(),
        version: version.clone(),
        path: tarball_path,
        hash,
        cached: false,
    })
}

/// Fetch expected hash from Hackage.
async fn fetch_expected_hash(
    client: &Client,
    name: &str,
    version: &Version,
) -> Result<String, FetchError> {
    let url = hackage_hash_url(name, version);
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(FetchError::NotFound(format!(
            "hash for {}-{}",
            name, version
        )));
    }

    let hash = response.text().await?.trim().to_string();

    // Hash file format is "hash  filename", extract just the hash
    let hash = hash.split_whitespace().next().unwrap_or(&hash).to_string();

    Ok(hash)
}

/// Compute SHA256 hash of a file (lowercase hex).
pub fn compute_file_hash(path: &Path) -> Result<String, FetchError> {
    Ok(sha256_file(path)?)
}

/// Compute the SHA256 hash of a byte slice (lowercase hex).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// A source tarball to fetch, optionally with the hash it must have
/// (from the lockfile).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    /// Package name
    pub name: String,
    /// Exact package version
    pub version: String,
    /// Expected SHA256 of the tarball (lowercase hex). `None` = not pinned.
    pub sha256: Option<String>,
}

impl SourceSpec {
    /// `name-version`.
    pub fn package_id(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }
}

/// Where hx caches the tarball for `name-version` inside `cache_dir`.
pub fn cached_tarball_path(cache_dir: &Path, name: &str, version: &str) -> PathBuf {
    cache_dir.join(format!("{}-{}.tar.gz", name, version))
}

/// Where cabal-install keeps (and, with `--offline`, expects) the tarball for
/// `name-version` inside its repository cache (see
/// [`crate::index::cabal_repo_cache_dir`]).
pub fn cabal_cached_tarball_path(repo_dir: &Path, name: &str, version: &str) -> PathBuf {
    repo_dir
        .join(name)
        .join(version)
        .join(format!("{}-{}.tar.gz", name, version))
}

/// State of one source in a cache, as checked against its pinned hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStatus {
    /// Present and matching the pinned hash.
    Verified { path: PathBuf, sha256: String },
    /// Present, but there is no pinned hash to compare against.
    Unpinned { path: PathBuf, sha256: String },
    /// Not in the cache.
    Missing { path: PathBuf },
    /// Present with different content than the pinned hash.
    Mismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
}

impl SourceStatus {
    /// Whether the cached tarball can be used.
    pub fn is_usable(&self) -> bool {
        matches!(
            self,
            SourceStatus::Verified { .. } | SourceStatus::Unpinned { .. }
        )
    }
}

/// Check a cached tarball at `path` against `expected` (lowercase hex).
pub fn check_source_file(path: &Path, expected: Option<&str>) -> Result<SourceStatus, FetchError> {
    if !path.is_file() {
        return Ok(SourceStatus::Missing {
            path: path.to_path_buf(),
        });
    }
    let actual = compute_file_hash(path)?;
    Ok(match expected {
        Some(expected) if expected != actual => SourceStatus::Mismatch {
            path: path.to_path_buf(),
            expected: expected.to_string(),
            actual,
        },
        Some(_) => SourceStatus::Verified {
            path: path.to_path_buf(),
            sha256: actual,
        },
        None => SourceStatus::Unpinned {
            path: path.to_path_buf(),
            sha256: actual,
        },
    })
}

/// Fetch source tarballs into `options.cache_dir`, verifying each against
/// its pinned hash.
///
/// Already-cached tarballs are re-hashed rather than trusted. A hash
/// mismatch is reported as [`FetchError::HashMismatch`] and never leaves a
/// file behind for a fresh download; a mismatching *cached* file is left in
/// place for inspection. Results are returned per spec, in input order, so
/// callers can report every failure at once.
pub async fn fetch_sources(
    specs: &[SourceSpec],
    options: &FetchOptions,
) -> Vec<(SourceSpec, Result<FetchResult, FetchError>)> {
    if let Err(e) = fs::create_dir_all(&options.cache_dir) {
        let msg = e.to_string();
        return specs
            .iter()
            .map(|s| {
                (
                    s.clone(),
                    Err(FetchError::CreateDir(io::Error::other(msg.clone()))),
                )
            })
            .collect();
    }

    let client = match Client::builder()
        .timeout(Duration::from_secs(options.timeout))
        .build()
    {
        Ok(c) => Arc::new(c),
        Err(e) => {
            let msg = e.to_string();
            return specs
                .iter()
                .map(|s| (s.clone(), Err(FetchError::NotFound(msg.clone()))))
                .collect();
        }
    };

    let jobs = options.jobs.max(1);
    stream::iter(specs.iter().cloned())
        .map(|spec| {
            let client = Arc::clone(&client);
            async move {
                let result = fetch_source(&client, &spec, options).await;
                (spec, result)
            }
        })
        .buffered(jobs)
        .collect()
        .await
}

/// Fetch (or re-verify) a single source tarball.
async fn fetch_source(
    client: &Client,
    spec: &SourceSpec,
    options: &FetchOptions,
) -> Result<FetchResult, FetchError> {
    let version: Version = spec
        .version
        .parse()
        .map_err(|_| FetchError::InvalidVersion {
            package: spec.name.clone(),
            version: spec.version.clone(),
        })?;
    let tarball_path = cached_tarball_path(&options.cache_dir, &spec.name, &spec.version);
    let expected = spec.sha256.as_deref();

    match check_source_file(&tarball_path, expected)? {
        SourceStatus::Verified { path, sha256 } | SourceStatus::Unpinned { path, sha256 } => {
            debug!("Package {} already cached", spec.package_id());
            return Ok(FetchResult {
                name: spec.name.clone(),
                version,
                path,
                hash: sha256,
                cached: true,
            });
        }
        SourceStatus::Mismatch {
            expected, actual, ..
        } => {
            return Err(FetchError::HashMismatch {
                package: spec.package_id(),
                expected,
                actual,
            });
        }
        SourceStatus::Missing { .. } => {}
    }

    let id = spec.package_id();
    let temp_path = options
        .cache_dir
        .join(format!(".{}.tar.gz.{}.tmp", id, std::process::id()));
    let hash = match download_to(client, &options.base_url, spec, &temp_path).await {
        Ok(hash) => hash,
        Err(e) => {
            let _ = fs::remove_file(&temp_path);
            return Err(e);
        }
    };

    if let Some(expected) = expected
        && hash != expected
    {
        let _ = fs::remove_file(&temp_path);
        return Err(FetchError::HashMismatch {
            package: id,
            expected: expected.to_string(),
            actual: hash,
        });
    }

    fs::rename(&temp_path, &tarball_path)?;
    info!("Downloaded {}", id);

    Ok(FetchResult {
        name: spec.name.clone(),
        version,
        path: tarball_path,
        hash,
        cached: false,
    })
}

/// Download `spec`'s tarball from `base_url` into `dest`, returning its hash.
async fn download_to(
    client: &Client,
    base_url: &str,
    spec: &SourceSpec,
    dest: &Path,
) -> Result<String, FetchError> {
    let id = spec.package_id();
    let relative = format!("package/{id}/{id}.tar.gz");

    if let Some(root) = base_url.strip_prefix("file://") {
        let src = Path::new(root).join(&relative);
        if !src.is_file() {
            return Err(FetchError::NotFound(format!("{} ({})", id, src.display())));
        }
        fs::copy(&src, dest)?;
        return compute_file_hash(dest);
    }

    let url = format!("{}/{}", base_url.trim_end_matches('/'), relative);
    debug!("Fetching {} from {}", id, url);
    let response = client.get(&url).send().await?;
    if !response.status().is_success() {
        return Err(FetchError::NotFound(format!(
            "{} (HTTP {} from {})",
            id,
            response.status(),
            url
        )));
    }

    let mut file = File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        hasher.update(&chunk);
    }
    file.flush()?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// What [`seed_cabal_cache`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    /// Copied into cabal's cache.
    Added,
    /// cabal already had an identical copy.
    AlreadyPresent,
    /// cabal had a *different* copy, which was replaced by the verified one.
    Replaced,
}

/// Make a verified tarball available to cabal-install by copying it into
/// cabal's repository cache (`repo_dir`), so `cabal build --offline` can use
/// it.
pub fn seed_cabal_cache(
    tarball: &Path,
    sha256: &str,
    repo_dir: &Path,
    name: &str,
    version: &str,
) -> Result<SeedOutcome, FetchError> {
    let dest = cabal_cached_tarball_path(repo_dir, name, version);
    let outcome = match check_source_file(&dest, Some(sha256))? {
        SourceStatus::Verified { .. } => return Ok(SeedOutcome::AlreadyPresent),
        SourceStatus::Missing { .. } => SeedOutcome::Added,
        _ => SeedOutcome::Replaced,
    };
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = dest.with_extension(format!("gz.{}.tmp", std::process::id()));
    fs::copy(tarball, &temp)?;
    fs::rename(&temp, &dest)?;
    Ok(outcome)
}

/// Summary of fetch operation.
#[derive(Debug, Default)]
pub struct FetchSummary {
    /// Number of packages downloaded
    pub downloaded: usize,
    /// Number of packages already cached
    pub cached: usize,
    /// Total bytes downloaded
    pub bytes_downloaded: u64,
    /// Package hashes (name -> hash)
    pub hashes: std::collections::HashMap<String, String>,
}

impl FetchSummary {
    /// Create summary from fetch results.
    pub fn from_results(results: &[FetchResult]) -> Self {
        let mut summary = FetchSummary::default();

        for result in results {
            if result.cached {
                summary.cached += 1;
            } else {
                summary.downloaded += 1;
            }
            summary.hashes.insert(
                format!("{}-{}", result.name, result.version),
                result.hash.clone(),
            );
        }

        summary
    }
}

#[cfg(test)]
mod hash_golden_tests {
    //! Tarball hashes are compared with the index. Pinned to exact output so a hashing-library upgrade cannot
    //! silently change them.
    use super::*;

    /// 200 kB of deterministic bytes: crosses every read-buffer size used.
    fn large_input() -> Vec<u8> {
        (0..200_000u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn file_hash_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkg.tar.gz");
        std::fs::write(&path, large_input()).unwrap();
        assert_eq!(
            compute_file_hash(&path).unwrap(),
            "e24bc62381f1224fbbb74688663f8f9743b9680b193edd666835e97b06e730eb"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hackage_url() {
        let url = hackage_url("text", &"2.1.0".parse().unwrap());
        assert_eq!(
            url,
            "https://hackage.haskell.org/package/text-2.1.0/text-2.1.0.tar.gz"
        );
    }

    #[test]
    fn test_hackage_hash_url() {
        let url = hackage_hash_url("aeson", &"2.2.0".parse().unwrap());
        assert_eq!(
            url,
            "https://hackage.haskell.org/package/aeson-2.2.0/aeson-2.2.0.tar.gz.sha256"
        );
    }

    #[test]
    fn test_is_base_package() {
        assert!(is_base_package("base"));
        assert!(is_base_package("ghc-prim"));
        assert!(is_base_package("ghc-bignum"));
        assert!(is_base_package("template-haskell"));
        assert!(is_base_package("bytestring"));
        assert!(is_base_package("text"));
        assert!(is_base_package("containers"));
        assert!(is_base_package("Win32"));
        assert!(!is_base_package("aeson"));
        assert!(!is_base_package("lens"));
        assert!(!is_base_package("warp"));
    }

    #[test]
    fn test_fetch_options_default() {
        let options = FetchOptions::default();
        assert_eq!(options.jobs, 8);
        assert!(options.verify_hashes);
        assert_eq!(options.timeout, 60);
    }

    #[test]
    fn test_fetch_summary() {
        let results = vec![
            FetchResult {
                name: "pkg1".to_string(),
                version: "1.0.0".parse().unwrap(),
                path: PathBuf::from("/tmp/pkg1.tar.gz"),
                hash: "abc123".to_string(),
                cached: false,
            },
            FetchResult {
                name: "pkg2".to_string(),
                version: "2.0.0".parse().unwrap(),
                path: PathBuf::from("/tmp/pkg2.tar.gz"),
                hash: "def456".to_string(),
                cached: true,
            },
        ];

        let summary = FetchSummary::from_results(&results);
        assert_eq!(summary.downloaded, 1);
        assert_eq!(summary.cached, 1);
        assert_eq!(summary.hashes.len(), 2);
        assert_eq!(
            summary.hashes.get("pkg1-1.0.0"),
            Some(&"abc123".to_string())
        );
        assert_eq!(
            summary.hashes.get("pkg2-2.0.0"),
            Some(&"def456".to_string())
        );
    }

    /// A local `file://` mirror containing `pkg-1.0.tar.gz` with `content`.
    fn mirror_with(content: &[u8]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("package").join("pkg-1.0");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("pkg-1.0.tar.gz"), content).unwrap();
        let url = format!("file://{}", dir.path().display());
        (dir, url)
    }

    fn spec(sha256: Option<String>) -> SourceSpec {
        SourceSpec {
            name: "pkg".into(),
            version: "1.0".into(),
            sha256,
        }
    }

    fn options(cache: &Path, base_url: String) -> FetchOptions {
        FetchOptions {
            cache_dir: cache.to_path_buf(),
            base_url,
            ..FetchOptions::default()
        }
    }

    #[tokio::test]
    async fn test_fetch_sources_verifies_hash() {
        let content = b"fake sdist tarball";
        let (_mirror, url) = mirror_with(content);
        let cache = tempfile::tempdir().unwrap();
        let opts = options(cache.path(), url);

        let results = fetch_sources(&[spec(Some(sha256_hex(content)))], &opts).await;
        let fetched = results[0].1.as_ref().unwrap();
        assert!(!fetched.cached);
        assert_eq!(fetched.hash, sha256_hex(content));
        assert_eq!(fs::read(&fetched.path).unwrap(), content);

        // Second run re-verifies the cached copy without the mirror.
        let offline = options(cache.path(), "file:///nonexistent".into());
        let results = fetch_sources(&[spec(Some(sha256_hex(content)))], &offline).await;
        assert!(results[0].1.as_ref().unwrap().cached);
    }

    #[tokio::test]
    async fn test_fetch_sources_rejects_mismatch() {
        let (_mirror, url) = mirror_with(b"tampered tarball");
        let cache = tempfile::tempdir().unwrap();
        let opts = options(cache.path(), url);
        let expected = sha256_hex(b"original tarball");

        let results = fetch_sources(&[spec(Some(expected.clone()))], &opts).await;
        match &results[0].1 {
            Err(FetchError::HashMismatch {
                package,
                expected: e,
                actual,
            }) => {
                assert_eq!(package, "pkg-1.0");
                assert_eq!(e, &expected);
                assert_eq!(actual, &sha256_hex(b"tampered tarball"));
            }
            other => panic!("expected hash mismatch, got {other:?}"),
        }
        // Nothing unverified is left in the cache.
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_check_source_file_states() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkg-1.0.tar.gz");
        assert!(matches!(
            check_source_file(&path, Some("00")).unwrap(),
            SourceStatus::Missing { .. }
        ));
        fs::write(&path, b"abc").unwrap();
        let good = sha256_hex(b"abc");
        assert!(check_source_file(&path, Some(&good)).unwrap().is_usable());
        assert!(matches!(
            check_source_file(&path, None).unwrap(),
            SourceStatus::Unpinned { .. }
        ));
        assert!(matches!(
            check_source_file(&path, Some("00")).unwrap(),
            SourceStatus::Mismatch { .. }
        ));
    }

    #[test]
    fn test_seed_cabal_cache() {
        let dir = tempfile::tempdir().unwrap();
        let tarball = dir.path().join("pkg-1.0.tar.gz");
        fs::write(&tarball, b"verified").unwrap();
        let sha = sha256_hex(b"verified");
        let repo = dir.path().join("repo");

        let seed = |t: &Path| seed_cabal_cache(t, &sha, &repo, "pkg", "1.0").unwrap();
        assert_eq!(seed(&tarball), SeedOutcome::Added);
        assert_eq!(seed(&tarball), SeedOutcome::AlreadyPresent);
        let dest = cabal_cached_tarball_path(&repo, "pkg", "1.0");
        assert!(dest.ends_with("pkg/1.0/pkg-1.0.tar.gz"));
        fs::write(&dest, b"corrupt").unwrap();
        assert_eq!(seed(&tarball), SeedOutcome::Replaced);
        assert_eq!(fs::read(&dest).unwrap(), b"verified");
    }

    /// Locks-and-fetches a tiny real package from Hackage end to end.
    #[tokio::test]
    #[ignore = "requires network access to hackage.haskell.org"]
    async fn test_fetch_real_package_from_hackage() {
        let cache = tempfile::tempdir().unwrap();
        let opts = options(cache.path(), HACKAGE_BASE_URL.to_string());
        let unpinned = SourceSpec {
            name: "hello".into(),
            version: "1.0.0.2".into(),
            sha256: None,
        };
        let results = fetch_sources(std::slice::from_ref(&unpinned), &opts).await;
        let first = results[0].1.as_ref().expect("download hello");
        // Published Hackage tarballs are immutable, so this hash is stable.
        assert_eq!(
            first.hash,
            "a7c0b5a2a9e29b07f36904944e47ad6796db6af3b9431cba6b92b0bcb23bd8f4"
        );

        // Pinning the observed hash verifies the cached copy...
        let pinned = SourceSpec {
            sha256: Some(first.hash.clone()),
            ..unpinned.clone()
        };
        let results = fetch_sources(std::slice::from_ref(&pinned), &opts).await;
        assert!(results[0].1.as_ref().unwrap().cached);

        // ...and a wrong pin is rejected.
        let wrong = SourceSpec {
            sha256: Some("0".repeat(64)),
            ..unpinned
        };
        let results = fetch_sources(&[wrong], &opts).await;
        assert!(matches!(results[0].1, Err(FetchError::HashMismatch { .. })));
    }

    #[test]
    fn test_default_package_cache_dir() {
        // This test just ensures the function doesn't panic
        let _cache_dir = default_package_cache_dir();
    }
}
