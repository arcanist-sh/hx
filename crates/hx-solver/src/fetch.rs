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

    #[error("hash mismatch for {package}: expected {expected}, got {actual}")]
    HashMismatch {
        package: String,
        expected: String,
        actual: String,
    },

    #[error("package not found: {0}")]
    NotFound(String),
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
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            jobs: 8,
            cache_dir: default_package_cache_dir().unwrap_or_else(|| PathBuf::from(".hx/packages")),
            verify_hashes: true,
            timeout: 60,
        }
    }
}

/// Get the default package cache directory.
pub fn default_package_cache_dir() -> Option<PathBuf> {
    let home = dirs_next::home_dir()?;
    Some(home.join(".cache").join("hx").join("packages"))
}

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

/// Compute SHA256 hash of a file.
fn compute_file_hash(path: &Path) -> Result<String, FetchError> {
    Ok(sha256_file(path)?)
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

    #[test]
    fn test_default_package_cache_dir() {
        // This test just ensures the function doesn't panic
        let _cache_dir = default_package_cache_dir();
    }
}
