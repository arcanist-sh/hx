//! Stackage snapshot support.
//!
//! This module provides functionality for working with Stackage snapshots:
//! - Parsing snapshot identifiers (LTS, Nightly)
//! - Fetching snapshot metadata from Stackage
//! - Creating package indices from snapshots
//! - GHC version mapping

use crate::{Package, PackageIndex, PackageVersion, Version};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, info, warn};

/// Error type for snapshot operations.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("invalid snapshot identifier: {0}")]
    InvalidIdentifier(String),

    #[error("failed to fetch snapshot: {0}")]
    FetchError(String),

    #[error("failed to parse snapshot data: {0}")]
    ParseError(String),

    #[error("snapshot not found: {0}")]
    NotFound(String),

    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("registry offline: {0}")]
    RegistryOffline(String),

    #[error("checksum mismatch: {0}")]
    ChecksumMismatch(String),

    #[error("signature verification failed: {0}")]
    SignatureInvalid(String),

    #[error("invalid public key: {0}")]
    InvalidPublicKey(String),
}

/// Type of Stackage snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotType {
    /// Long-term support snapshot (e.g., lts-24.64)
    Lts,
    /// Nightly snapshot (e.g., nightly-2026-10-19)
    Nightly,
    /// BHC Platform curated snapshot (e.g., bhc-platform-2026.1)
    BhcPlatform,
}

/// Parsed Stackage snapshot identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotId {
    /// Type of snapshot
    pub snapshot_type: SnapshotType,
    /// Raw identifier string
    pub raw: String,
    /// Major version (for LTS)
    pub major: Option<u32>,
    /// Minor version (for LTS)
    pub minor: Option<u32>,
    /// Date string (for Nightly)
    pub date: Option<String>,
}

impl SnapshotId {
    /// The GHC version this snapshot is built with, from hx's built-in table.
    ///
    /// Approximate (the latest GHC of the LTS series); the snapshot's own
    /// `with-compiler:` field is authoritative when it is available.
    pub fn default_ghc_version(&self) -> String {
        infer_ghc_version(self)
    }

    /// Parse a snapshot identifier string.
    ///
    /// Supports formats:
    /// - `lts-24.64` - LTS snapshot
    /// - `lts-24` - Latest minor in LTS 24
    /// - `nightly-2026-10-19` - Nightly snapshot
    /// - `nightly` - Latest nightly
    pub fn parse(s: &str) -> Result<Self, SnapshotError> {
        let s = s.trim().to_lowercase();

        if s.starts_with("lts") {
            Self::parse_lts(&s)
        } else if s.starts_with("nightly") {
            Self::parse_nightly(&s)
        } else if s.starts_with("bhc-platform") {
            Self::parse_bhc_platform(&s)
        } else {
            Err(SnapshotError::InvalidIdentifier(format!(
                "snapshot must start with 'lts', 'nightly', or 'bhc-platform': {}",
                s
            )))
        }
    }

    fn parse_lts(s: &str) -> Result<Self, SnapshotError> {
        if s == "lts" {
            return Ok(Self {
                snapshot_type: SnapshotType::Lts,
                raw: s.to_string(),
                major: None,
                minor: None,
                date: None,
            });
        }

        let rest = s.strip_prefix("lts-").ok_or_else(|| {
            SnapshotError::InvalidIdentifier(format!("invalid LTS format: {}", s))
        })?;

        let parts: Vec<&str> = rest.split('.').collect();
        match parts.as_slice() {
            [major] => {
                let major: u32 = major.parse().map_err(|_| {
                    SnapshotError::InvalidIdentifier(format!(
                        "invalid LTS major version: {}",
                        major
                    ))
                })?;
                Ok(Self {
                    snapshot_type: SnapshotType::Lts,
                    raw: s.to_string(),
                    major: Some(major),
                    minor: None,
                    date: None,
                })
            }
            [major, minor] => {
                let major: u32 = major.parse().map_err(|_| {
                    SnapshotError::InvalidIdentifier(format!(
                        "invalid LTS major version: {}",
                        major
                    ))
                })?;
                let minor: u32 = minor.parse().map_err(|_| {
                    SnapshotError::InvalidIdentifier(format!(
                        "invalid LTS minor version: {}",
                        minor
                    ))
                })?;
                Ok(Self {
                    snapshot_type: SnapshotType::Lts,
                    raw: s.to_string(),
                    major: Some(major),
                    minor: Some(minor),
                    date: None,
                })
            }
            _ => Err(SnapshotError::InvalidIdentifier(format!(
                "invalid LTS format: {}",
                s
            ))),
        }
    }

    fn parse_nightly(s: &str) -> Result<Self, SnapshotError> {
        if s == "nightly" {
            return Ok(Self {
                snapshot_type: SnapshotType::Nightly,
                raw: s.to_string(),
                major: None,
                minor: None,
                date: None,
            });
        }

        let date = s.strip_prefix("nightly-").ok_or_else(|| {
            SnapshotError::InvalidIdentifier(format!("invalid nightly format: {}", s))
        })?;

        // Validate date format (YYYY-MM-DD)
        let parts: Vec<&str> = date.split('-').collect();
        if parts.len() != 3 {
            return Err(SnapshotError::InvalidIdentifier(format!(
                "invalid nightly date format: {}",
                date
            )));
        }

        Ok(Self {
            snapshot_type: SnapshotType::Nightly,
            raw: s.to_string(),
            major: None,
            minor: None,
            date: Some(date.to_string()),
        })
    }

    fn parse_bhc_platform(s: &str) -> Result<Self, SnapshotError> {
        // Format: bhc-platform-YYYY.N
        let rest = s.strip_prefix("bhc-platform-").ok_or_else(|| {
            SnapshotError::InvalidIdentifier(format!("invalid BHC platform format: {}", s))
        })?;

        let parts: Vec<&str> = rest.split('.').collect();
        match parts.as_slice() {
            [year, rev] => {
                let _year: u32 = year.parse().map_err(|_| {
                    SnapshotError::InvalidIdentifier(format!("invalid BHC platform year: {}", year))
                })?;
                let _rev: u32 = rev.parse().map_err(|_| {
                    SnapshotError::InvalidIdentifier(format!(
                        "invalid BHC platform revision: {}",
                        rev
                    ))
                })?;
                Ok(Self {
                    snapshot_type: SnapshotType::BhcPlatform,
                    raw: s.to_string(),
                    major: Some(_year),
                    minor: Some(_rev),
                    date: None,
                })
            }
            _ => Err(SnapshotError::InvalidIdentifier(format!(
                "BHC platform must be bhc-platform-YYYY.N: {}",
                s
            ))),
        }
    }

    /// Get the canonical snapshot key for URLs/caching.
    pub fn key(&self) -> String {
        self.raw.clone()
    }

    /// Check if this is a fully specified snapshot (not "latest").
    pub fn is_specific(&self) -> bool {
        match self.snapshot_type {
            SnapshotType::Lts => self.major.is_some() && self.minor.is_some(),
            SnapshotType::Nightly => self.date.is_some(),
            SnapshotType::BhcPlatform => self.major.is_some() && self.minor.is_some(),
        }
    }
}

impl std::fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}

/// Metadata about a Stackage snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// Snapshot identifier
    pub snapshot: SnapshotId,
    /// GHC version used by this snapshot
    pub ghc_version: String,
    /// Number of packages in the snapshot
    pub package_count: usize,
    /// When the snapshot was created
    pub created: Option<String>,
    /// BHC version (for BHC Platform snapshots)
    #[serde(default)]
    pub bhc_version: Option<String>,
    /// Recommended BHC profile (for BHC Platform snapshots)
    #[serde(default)]
    pub recommended_profile: Option<String>,
}

/// A package version in a Stackage snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotPackage {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Cabal flags to use
    #[serde(default)]
    pub flags: HashMap<String, bool>,
    /// Whether this package is hidden (internal)
    #[serde(default)]
    pub hidden: bool,
}

/// A loaded Stackage snapshot with package versions.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Snapshot metadata
    pub metadata: SnapshotMetadata,
    /// Package versions in this snapshot
    pub packages: HashMap<String, SnapshotPackage>,
}

impl Snapshot {
    /// Get the version for a package in this snapshot.
    pub fn get_version(&self, name: &str) -> Option<&str> {
        self.packages.get(name).map(|p| p.version.as_str())
    }

    /// Check if a package is in this snapshot.
    pub fn contains(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    /// Convert to a PackageIndex for use with the resolver.
    ///
    /// This creates a restricted index containing only the packages
    /// and versions from the snapshot.
    pub fn to_package_index(&self) -> PackageIndex {
        let mut packages = HashMap::new();

        for (name, snap_pkg) in &self.packages {
            if snap_pkg.hidden {
                continue;
            }

            let version: Version = match snap_pkg.version.parse() {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        "Failed to parse version {} for {}: {}",
                        snap_pkg.version, name, e
                    );
                    continue;
                }
            };

            let pkg_version = PackageVersion {
                name: name.clone(),
                version: version.clone(),
                dependencies: Vec::new(), // Will be populated from Hackage if needed
                revision: 0,
                hash: None,
            };

            let mut package = Package::new(name.clone());
            package.add_version(pkg_version);

            packages.insert(name.clone(), package);
        }

        PackageIndex { packages }
    }
}

/// Cache directory for Stackage snapshots.
pub fn snapshot_cache_dir() -> Option<PathBuf> {
    dirs_next::cache_dir().map(|d: PathBuf| d.join("hx").join("stackage"))
}

/// Get the cache path for a specific snapshot.
pub fn snapshot_cache_path(snapshot: &SnapshotId) -> Option<PathBuf> {
    snapshot_cache_dir().map(|d| d.join(format!("{}.json", snapshot.key())))
}

/// Fetch a Stackage snapshot from the network.
pub async fn fetch_snapshot(snapshot: &SnapshotId) -> Result<Snapshot, SnapshotError> {
    info!("Fetching Stackage snapshot: {}", snapshot);

    let client = hx_core::http::client()?;

    // Stackage provides snapshot data in multiple formats
    // We'll use the JSON API
    let url = format!("https://www.stackage.org/{}/cabal.config", snapshot.key());

    debug!("Fetching snapshot from: {}", url);

    let response = client.get(&url).send().await?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(SnapshotError::NotFound(snapshot.key()));
    }

    if !response.status().is_success() {
        return Err(SnapshotError::FetchError(format!(
            "HTTP {}: {}",
            response.status(),
            url
        )));
    }

    let content = response.text().await?;
    parse_cabal_config(&content, snapshot)
}

/// Parse a Stackage cabal.config format.
///
/// Format (as served by stackage.org):
/// ```text
/// -- Stackage snapshot from: http://www.stackage.org/snapshot/lts-24.64
/// with-compiler: ghc-9.10.3
/// constraints: abstract-deque ==0.3,
///              aeson ==2.2.3.0,
///              base installed,
///              ...
/// ```
fn parse_cabal_config(content: &str, snapshot: &SnapshotId) -> Result<Snapshot, SnapshotError> {
    let mut packages = HashMap::new();
    let mut ghc_version = String::new();
    let mut in_constraints = false;

    for raw in content.lines() {
        let line = raw.trim();

        // Skip comments but extract GHC version if present
        if line.starts_with("--") {
            if ghc_version.is_empty()
                && line.contains("ghc-")
                && let Some(ver) = extract_ghc_version(line)
            {
                ghc_version = ver;
            }
            continue;
        }

        // Skip empty lines
        if line.is_empty() {
            continue;
        }

        // A field starts at column 0; continuation lines are indented.
        let constraint = if raw.starts_with(char::is_whitespace) {
            if !in_constraints {
                continue;
            }
            line
        } else if let Some(rest) = line.strip_prefix("constraints:") {
            // The first constraint shares the line with the field name.
            in_constraints = true;
            rest.trim()
        } else {
            in_constraints = false;
            if let Some(compiler) = line.strip_prefix("with-compiler:")
                && let Some(ver) = extract_ghc_version(compiler)
            {
                ghc_version = ver;
            }
            continue;
        };

        // Remove trailing comma
        let constraint = constraint.trim_end_matches(',');

        // Parse "package ==version" or "package installed"
        if let Some((name, version)) = parse_constraint_line(constraint) {
            if name == "ghc" && ghc_version.is_empty() {
                ghc_version = version.clone();
            }

            packages.insert(
                name.clone(),
                SnapshotPackage {
                    name,
                    version,
                    flags: HashMap::new(),
                    hidden: false,
                },
            );
        }
    }

    // If we couldn't extract GHC version, try to infer from LTS
    if ghc_version.is_empty() {
        ghc_version = infer_ghc_version(snapshot);
    }

    let metadata = SnapshotMetadata {
        snapshot: snapshot.clone(),
        ghc_version,
        package_count: packages.len(),
        created: None,
        bhc_version: None,
        recommended_profile: None,
    };

    Ok(Snapshot { metadata, packages })
}

/// Parse a constraint line like "aeson ==2.2.1.0".
fn parse_constraint_line(line: &str) -> Option<(String, String)> {
    // Handle "package ==version"
    if let Some(idx) = line.find("==") {
        let name = line[..idx].trim().to_string();
        let version = line[idx + 2..].trim().to_string();
        return Some((name, version));
    }

    // Handle "package installed" (skip these)
    if line.ends_with("installed") {
        return None;
    }

    None
}

/// Extract GHC version from a comment line.
fn extract_ghc_version(line: &str) -> Option<String> {
    // Look for patterns like "ghc-9.6.4" or "GHC 9.6.4"
    let lower = line.to_lowercase();

    if let Some(idx) = lower.find("ghc-") {
        let start = idx + 4;
        let rest = &line[start..];
        let version: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !version.is_empty() {
            return Some(version);
        }
    }

    if let Some(idx) = lower.find("ghc ") {
        let start = idx + 4;
        let rest = &line[start..];
        let version: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !version.is_empty() {
            return Some(version);
        }
    }

    None
}

/// Infer GHC version from snapshot identifier.
///
/// This is a fallback when we can't extract the version from the snapshot data.
fn infer_ghc_version(snapshot: &SnapshotId) -> String {
    // Known LTS -> GHC mappings (approximate)
    match snapshot.snapshot_type {
        SnapshotType::Lts => {
            if let Some(major) = snapshot.major {
                match major {
                    24 => "9.10.3".to_string(),
                    23 => "9.8.4".to_string(),
                    22 => "9.6.7".to_string(),
                    21 => "9.4.8".to_string(),
                    20 => "9.2.8".to_string(),
                    19 => "9.0.2".to_string(),
                    18 => "8.10.7".to_string(),
                    _ => "9.10.3".to_string(), // Default to recent
                }
            } else {
                "9.10.3".to_string()
            }
        }
        SnapshotType::Nightly => "9.12.4".to_string(), // Nightly typically uses latest
        SnapshotType::BhcPlatform => "9.8.2".to_string(), // BHC platforms target latest stable
    }
}

/// Load a snapshot, using cache if available.
pub async fn load_snapshot(
    snapshot: &SnapshotId,
    cache_dir: Option<&Path>,
) -> Result<Snapshot, SnapshotError> {
    // BHC Platform snapshots: try embedded first, then cache, then remote
    if snapshot.snapshot_type == SnapshotType::BhcPlatform {
        return crate::bhc_platform::load_bhc_platform_async(snapshot).await;
    }

    // Check cache first
    let cache_path = cache_dir
        .map(|d| d.join(format!("{}.json", snapshot.key())))
        .or_else(|| snapshot_cache_path(snapshot));

    if let Some(ref path) = cache_path
        && path.exists()
    {
        debug!("Loading snapshot from cache: {}", path.display());
        match load_snapshot_from_cache(path) {
            Ok(snap) => return Ok(snap),
            Err(e) => {
                warn!("Failed to load cached snapshot: {}", e);
                // Fall through to fetch
            }
        }
    }

    // Fetch from network
    let snapshot_data = fetch_snapshot(snapshot).await?;

    // Cache it
    if let Some(ref path) = cache_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = save_snapshot_to_cache(&snapshot_data, path) {
            warn!("Failed to cache snapshot: {}", e);
        }
    }

    Ok(snapshot_data)
}

/// Load a snapshot from cache.
fn load_snapshot_from_cache(path: &Path) -> Result<Snapshot, SnapshotError> {
    let content = std::fs::read_to_string(path)?;
    let cached: CachedSnapshot = serde_json::from_str(&content)?;
    Ok(cached.into_snapshot())
}

/// Save a snapshot to cache.
fn save_snapshot_to_cache(snapshot: &Snapshot, path: &Path) -> Result<(), SnapshotError> {
    let cached = CachedSnapshot::from_snapshot(snapshot);
    let content = serde_json::to_string_pretty(&cached)?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Serializable cache format for snapshots.
#[derive(Serialize, Deserialize)]
struct CachedSnapshot {
    metadata: SnapshotMetadata,
    packages: Vec<SnapshotPackage>,
}

impl CachedSnapshot {
    fn from_snapshot(snap: &Snapshot) -> Self {
        Self {
            metadata: snap.metadata.clone(),
            packages: snap.packages.values().cloned().collect(),
        }
    }

    fn into_snapshot(self) -> Snapshot {
        let packages = self
            .packages
            .into_iter()
            .map(|p| (p.name.clone(), p))
            .collect();
        Snapshot {
            metadata: self.metadata,
            packages,
        }
    }
}

/// Get the latest LTS snapshot identifier.
pub async fn get_latest_lts() -> Result<SnapshotId, SnapshotError> {
    // Stackage redirect to latest
    let client = hx_core::http::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let response = client.head("https://www.stackage.org/lts").send().await?;

    if let Some(location) = response.headers().get("location") {
        let loc = location.to_str().unwrap_or("");
        // Extract lts-XX.YY from redirect URL
        if let Some(start) = loc.find("lts-") {
            let snapshot_part = &loc[start..];
            let end = snapshot_part.find('/').unwrap_or(snapshot_part.len());
            return SnapshotId::parse(&snapshot_part[..end]);
        }
    }

    Err(SnapshotError::FetchError(
        "Could not determine latest LTS".to_string(),
    ))
}

/// Get the latest nightly snapshot identifier.
pub async fn get_latest_nightly() -> Result<SnapshotId, SnapshotError> {
    let client = hx_core::http::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let response = client
        .head("https://www.stackage.org/nightly")
        .send()
        .await?;

    if let Some(location) = response.headers().get("location") {
        let loc = location.to_str().unwrap_or("");
        if let Some(start) = loc.find("nightly-") {
            let snapshot_part = &loc[start..];
            let end = snapshot_part.find('/').unwrap_or(snapshot_part.len());
            return SnapshotId::parse(&snapshot_part[..end]);
        }
    }

    Err(SnapshotError::FetchError(
        "Could not determine latest nightly".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Header of a real stackage.org cabal.config (lts-24.64), trimmed.
    const STACKAGE_CABAL_CONFIG: &str = "\
-- NOTE: Due to revisions, this file may not work. See:
-- https://github.com/commercialhaskell/stackage-server/issues/232

-- Stackage snapshot from: http://www.stackage.org/snapshot/lts-24.64
-- Please place this file next to your .cabal file as cabal.config
-- To only use tested packages, uncomment the following line:
-- remote-repo: stackage-lts-24.64:http://www.stackage.org/lts-24.64
with-compiler: ghc-9.10.3
constraints: abstract-deque ==0.3,
             aeson ==2.2.3.0,
             base installed,
             zstd ==0.1.4.0
";

    #[test]
    fn test_parse_cabal_config_stackage_format() {
        let id = SnapshotId::parse("lts-24.64").unwrap();
        let snapshot = parse_cabal_config(STACKAGE_CABAL_CONFIG, &id).unwrap();

        assert_eq!(snapshot.metadata.ghc_version, "9.10.3");
        assert_eq!(snapshot.packages.len(), 3);
        assert_eq!(snapshot.packages["abstract-deque"].version, "0.3");
        assert_eq!(snapshot.packages["aeson"].version, "2.2.3.0");
        assert_eq!(snapshot.packages["zstd"].version, "0.1.4.0");
        assert!(!snapshot.packages.contains_key("base"));
    }

    #[test]
    fn test_parse_cabal_config_constraints_on_own_line() {
        let content = "constraints:\n    aeson ==2.2.1.0,\n    text ==2.1\n";
        let id = SnapshotId::parse("lts-22.28").unwrap();
        let snapshot = parse_cabal_config(content, &id).unwrap();

        assert_eq!(snapshot.packages.len(), 2);
        // No compiler in the file: fall back to the LTS table
        assert_eq!(snapshot.metadata.ghc_version, "9.6.7");
    }

    #[test]
    fn test_parse_lts_full() {
        let id = SnapshotId::parse("lts-22.28").unwrap();
        assert_eq!(id.snapshot_type, SnapshotType::Lts);
        assert_eq!(id.major, Some(22));
        assert_eq!(id.minor, Some(28));
        assert!(id.is_specific());
    }

    #[test]
    fn test_parse_lts_major_only() {
        let id = SnapshotId::parse("lts-22").unwrap();
        assert_eq!(id.snapshot_type, SnapshotType::Lts);
        assert_eq!(id.major, Some(22));
        assert_eq!(id.minor, None);
        assert!(!id.is_specific());
    }

    #[test]
    fn test_parse_lts_latest() {
        let id = SnapshotId::parse("lts").unwrap();
        assert_eq!(id.snapshot_type, SnapshotType::Lts);
        assert_eq!(id.major, None);
        assert!(!id.is_specific());
    }

    #[test]
    fn test_parse_nightly_full() {
        let id = SnapshotId::parse("nightly-2024-01-15").unwrap();
        assert_eq!(id.snapshot_type, SnapshotType::Nightly);
        assert_eq!(id.date, Some("2024-01-15".to_string()));
        assert!(id.is_specific());
    }

    #[test]
    fn test_parse_nightly_latest() {
        let id = SnapshotId::parse("nightly").unwrap();
        assert_eq!(id.snapshot_type, SnapshotType::Nightly);
        assert_eq!(id.date, None);
        assert!(!id.is_specific());
    }

    #[test]
    fn test_parse_bhc_platform_id() {
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        assert_eq!(id.snapshot_type, SnapshotType::BhcPlatform);
        assert_eq!(id.major, Some(2026));
        assert_eq!(id.minor, Some(1));
        assert!(id.is_specific());
    }

    #[test]
    fn test_bhc_platform_key() {
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        assert_eq!(id.key(), "bhc-platform-2026.1");
    }

    #[test]
    fn test_parse_bhc_platform_invalid() {
        assert!(SnapshotId::parse("bhc-platform-").is_err());
        assert!(SnapshotId::parse("bhc-platform-abc").is_err());
        assert!(SnapshotId::parse("bhc-platform-2026").is_err());
    }

    #[test]
    fn test_parse_case_insensitive() {
        let id = SnapshotId::parse("LTS-22.28").unwrap();
        assert_eq!(id.major, Some(22));

        let id = SnapshotId::parse("Nightly-2024-01-15").unwrap();
        assert_eq!(id.date, Some("2024-01-15".to_string()));
    }

    #[test]
    fn test_parse_invalid() {
        assert!(SnapshotId::parse("invalid").is_err());
        assert!(SnapshotId::parse("lts-abc").is_err());
        assert!(SnapshotId::parse("nightly-invalid").is_err());
    }

    #[test]
    fn test_parse_constraint_line() {
        assert_eq!(
            parse_constraint_line("aeson ==2.2.1.0"),
            Some(("aeson".to_string(), "2.2.1.0".to_string()))
        );
        assert_eq!(
            parse_constraint_line("  base ==4.18.2.0  "),
            Some(("base".to_string(), "4.18.2.0".to_string()))
        );
        assert_eq!(parse_constraint_line("rts installed"), None);
    }

    #[test]
    fn test_extract_ghc_version() {
        assert_eq!(
            extract_ghc_version("-- ghc-9.6.4"),
            Some("9.6.4".to_string())
        );
        assert_eq!(
            extract_ghc_version("-- GHC 9.6.4 for lts-22"),
            Some("9.6.4".to_string())
        );
        assert_eq!(extract_ghc_version("-- no version here"), None);
    }

    #[test]
    fn test_infer_ghc_version() {
        let lts22 = SnapshotId::parse("lts-22.28").unwrap();
        assert_eq!(infer_ghc_version(&lts22), "9.6.7");

        let lts24 = SnapshotId::parse("lts-24.64").unwrap();
        assert_eq!(infer_ghc_version(&lts24), "9.10.3");

        let lts21 = SnapshotId::parse("lts-21").unwrap();
        assert_eq!(infer_ghc_version(&lts21), "9.4.8");
    }
}
