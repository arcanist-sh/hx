//! Hackage index mirroring and management.
//!
//! This module provides:
//! - Direct download of the Hackage package index
//! - Incremental index updates using HTTP conditional requests
//! - Index staleness tracking and auto-update
//! - Offline-capable dependency resolution

use chrono::{DateTime, Utc};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tracing::info;

/// Hackage index URL.
const HACKAGE_INDEX_URL: &str = "https://hackage.haskell.org/01-index.tar.gz";

/// Default staleness threshold in hours.
const DEFAULT_STALENESS_HOURS: u64 = 24;

/// Error type for mirror operations.
#[derive(Debug, Error)]
pub enum MirrorError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("failed to parse index state: {0}")]
    ParseState(#[from] serde_json::Error),

    #[error("index not found - run `hx index update` first")]
    IndexNotFound,

    #[error("index update failed: {0}")]
    UpdateFailed(String),
}

/// Metadata about the mirrored index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexState {
    /// When the index was last updated
    pub last_updated: DateTime<Utc>,
    /// HTTP ETag for conditional requests
    pub etag: Option<String>,
    /// HTTP Last-Modified header
    pub last_modified: Option<String>,
    /// Size of the index file in bytes
    pub size_bytes: u64,
    /// SHA256 hash of the index file
    pub sha256: String,
    /// Number of packages in the index (if known)
    pub package_count: Option<usize>,
    /// Number of versions in the index (if known)
    pub version_count: Option<usize>,
}

impl IndexState {
    /// Check if the index is stale based on staleness threshold.
    pub fn is_stale(&self, threshold_hours: u64) -> bool {
        let age = Utc::now() - self.last_updated;
        age.num_hours() as u64 >= threshold_hours
    }

    /// Get age of the index in human-readable format.
    pub fn age_string(&self) -> String {
        let age = Utc::now() - self.last_updated;
        if age.num_days() > 0 {
            format!("{} days ago", age.num_days())
        } else if age.num_hours() > 0 {
            format!("{} hours ago", age.num_hours())
        } else if age.num_minutes() > 0 {
            format!("{} minutes ago", age.num_minutes())
        } else {
            "just now".to_string()
        }
    }
}

/// Options for index mirroring operations.
#[derive(Debug, Clone)]
pub struct MirrorOptions {
    /// HTTP timeout in seconds
    pub timeout: u64,
    /// Show progress during download
    pub show_progress: bool,
    /// Force full download even if index exists
    pub force: bool,
    /// Staleness threshold in hours (0 = always update)
    pub staleness_hours: u64,
}

impl Default for MirrorOptions {
    fn default() -> Self {
        Self {
            timeout: 300, // 5 minutes for large index
            show_progress: true,
            force: false,
            staleness_hours: DEFAULT_STALENESS_HOURS,
        }
    }
}

/// Result of an index update operation.
#[derive(Debug)]
pub struct UpdateResult {
    /// Whether the index was actually downloaded
    pub downloaded: bool,
    /// Size of the downloaded file (if downloaded)
    pub bytes_downloaded: u64,
    /// The new index state
    pub state: IndexState,
}

/// Get the hx-managed index directory.
pub fn index_dir() -> Option<PathBuf> {
    let home = dirs_next::home_dir()?;
    Some(home.join(".cache").join("hx").join("index"))
}

/// Get the path to the mirrored index file.
pub fn index_path() -> Option<PathBuf> {
    Some(index_dir()?.join("01-index.tar.gz"))
}

/// Get the path to the index state file.
pub fn index_state_path() -> Option<PathBuf> {
    Some(index_dir()?.join("state.json"))
}

/// Load the current index state.
pub fn load_index_state() -> Result<IndexState, MirrorError> {
    let state_path = index_state_path().ok_or(MirrorError::IndexNotFound)?;

    if !state_path.exists() {
        return Err(MirrorError::IndexNotFound);
    }

    let file = File::open(&state_path)?;
    let reader = BufReader::new(file);
    let state: IndexState = serde_json::from_reader(reader)?;

    Ok(state)
}

/// Save the index state.
fn save_index_state(state: &IndexState) -> Result<(), MirrorError> {
    let state_path = index_state_path().ok_or(MirrorError::IndexNotFound)?;

    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_vec_pretty(state)?;
    hx_core::atomic_write(&state_path, content)?;

    Ok(())
}

/// Check if the index exists and is not stale.
pub fn index_is_current(staleness_hours: u64) -> bool {
    if let Ok(state) = load_index_state()
        && let Some(path) = index_path()
    {
        return path.exists() && !state.is_stale(staleness_hours);
    }
    false
}

/// Get the best available index path.
///
/// Prefers hx-managed index, falls back to cabal's index.
pub fn best_index_path() -> Option<PathBuf> {
    // First try hx-managed index
    if let Some(path) = index_path()
        && path.exists()
    {
        return Some(path);
    }

    // Fall back to cabal's index
    crate::index::default_index_path()
}

/// Update the package index from Hackage.
///
/// Uses HTTP conditional requests to avoid re-downloading if not needed.
pub async fn update_index(options: &MirrorOptions) -> Result<UpdateResult, MirrorError> {
    let idx_path = index_path().ok_or(MirrorError::IndexNotFound)?;
    let idx_dir = index_dir().ok_or(MirrorError::IndexNotFound)?;

    // Ensure directory exists
    fs::create_dir_all(&idx_dir)?;

    // Load existing state for conditional request
    let existing_state = load_index_state().ok();

    // Build HTTP client
    let client = Client::builder()
        .timeout(Duration::from_secs(options.timeout))
        .build()?;

    // Build request with conditional headers
    let mut request = client.get(HACKAGE_INDEX_URL);

    if !options.force
        && let Some(ref state) = existing_state
    {
        if let Some(ref etag) = state.etag {
            request = request.header("If-None-Match", etag);
        }
        if let Some(ref last_modified) = state.last_modified {
            request = request.header("If-Modified-Since", last_modified);
        }
    }

    info!("Checking Hackage index at {}", HACKAGE_INDEX_URL);

    let response = request.send().await?;

    // Check if not modified (304)
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        info!("Index is up to date");

        // A 304 without local state (e.g. a misbehaving proxy) must not panic
        let Some(state) = existing_state else {
            return Err(MirrorError::UpdateFailed(
                "server returned 304 Not Modified but no local index state exists; \
                 retry with --force"
                    .to_string(),
            ));
        };
        return Ok(UpdateResult {
            downloaded: false,
            bytes_downloaded: 0,
            state,
        });
    }

    // Check for success
    if !response.status().is_success() {
        return Err(MirrorError::UpdateFailed(format!(
            "HTTP {} from Hackage",
            response.status()
        )));
    }

    // Extract headers for state
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let content_length = response.content_length();

    info!("Downloading Hackage index...");

    // Set up progress bar
    let pb = if options.show_progress {
        let pb = if let Some(len) = content_length {
            ProgressBar::new(len)
        } else {
            ProgressBar::new_spinner()
        };
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.set_message("Downloading index");
        Some(pb)
    } else {
        None
    };

    // Download to temp file first
    let temp_path = idx_path.with_extension("tmp");
    let mut file = File::create(&temp_path)?;
    let mut hasher = Sha256::new();
    let mut total_bytes = 0u64;

    let mut stream = response.bytes_stream();
    use futures::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        total_bytes += chunk.len() as u64;

        if let Some(ref pb) = pb {
            pb.set_position(total_bytes);
        }
    }

    file.flush()?;
    drop(file);

    if let Some(pb) = pb {
        pb.finish_with_message("Download complete");
    }

    // Compute hash
    let sha256 = format!("{:x}", hasher.finalize());

    // Move temp file to final location
    fs::rename(&temp_path, &idx_path)?;

    info!("Downloaded {} bytes to {}", total_bytes, idx_path.display());

    // Save state
    let state = IndexState {
        last_updated: Utc::now(),
        etag,
        last_modified,
        size_bytes: total_bytes,
        sha256,
        package_count: None,
        version_count: None,
    };

    save_index_state(&state)?;

    Ok(UpdateResult {
        downloaded: true,
        bytes_downloaded: total_bytes,
        state,
    })
}

/// Get index status information.
pub fn index_status() -> Result<IndexStatus, MirrorError> {
    let state = load_index_state()?;
    let path = index_path().ok_or(MirrorError::IndexNotFound)?;

    if !path.exists() {
        return Err(MirrorError::IndexNotFound);
    }

    let metadata = fs::metadata(&path)?;

    Ok(IndexStatus {
        path,
        state,
        file_size: metadata.len(),
    })
}

/// Index status information.
#[derive(Debug)]
pub struct IndexStatus {
    /// Path to the index file
    pub path: PathBuf,
    /// Index state metadata
    pub state: IndexState,
    /// Current file size on disk
    pub file_size: u64,
}

impl IndexStatus {
    /// Format size in human-readable format.
    pub fn size_string(&self) -> String {
        let bytes = self.file_size;
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
}

/// Clear the mirrored index.
pub fn clear_index() -> Result<(), MirrorError> {
    if let Some(dir) = index_dir()
        && dir.exists()
    {
        fs::remove_dir_all(&dir)?;
        info!("Cleared index directory: {}", dir.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_state_staleness() {
        let recent = IndexState {
            last_updated: Utc::now(),
            etag: None,
            last_modified: None,
            size_bytes: 1000,
            sha256: "abc".to_string(),
            package_count: None,
            version_count: None,
        };

        assert!(!recent.is_stale(24));

        let old = IndexState {
            last_updated: Utc::now() - chrono::Duration::hours(48),
            etag: None,
            last_modified: None,
            size_bytes: 1000,
            sha256: "abc".to_string(),
            package_count: None,
            version_count: None,
        };

        assert!(old.is_stale(24));
    }

    #[test]
    fn test_age_string() {
        let now = IndexState {
            last_updated: Utc::now(),
            etag: None,
            last_modified: None,
            size_bytes: 0,
            sha256: String::new(),
            package_count: None,
            version_count: None,
        };

        assert_eq!(now.age_string(), "just now");
    }

    #[test]
    fn test_size_string() {
        let status = IndexStatus {
            path: PathBuf::from("/tmp/test"),
            state: IndexState {
                last_updated: Utc::now(),
                etag: None,
                last_modified: None,
                size_bytes: 0,
                sha256: String::new(),
                package_count: None,
                version_count: None,
            },
            file_size: 150_000_000,
        };

        assert_eq!(status.size_string(), "150.00 MB");
    }

    #[test]
    fn test_index_dir() {
        // Should return Some path
        let dir = index_dir();
        assert!(dir.is_some());
        assert!(dir.unwrap().ends_with("hx/index"));
    }
}
