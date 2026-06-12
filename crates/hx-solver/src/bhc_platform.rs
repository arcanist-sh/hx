//! BHC Platform curated snapshots.
//!
//! Provides embedded, curated package sets for BHC projects.
//! These are similar to Stackage snapshots but tailored for BHC.

use crate::snapshot::{Snapshot, SnapshotError, SnapshotId, SnapshotMetadata, SnapshotPackage};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Embedded BHC platform data.
const BHC_PLATFORM_2026_1: &str = include_str!("../data/bhc-platform-2026.1.toml");
const BHC_PLATFORM_2026_2: &str = include_str!("../data/bhc-platform-2026.2.toml");

/// Metadata for a BHC platform snapshot.
#[derive(Debug, Clone)]
pub struct BhcPlatformInfo {
    /// Platform identifier
    pub id: &'static str,
    /// BHC version
    pub bhc_version: &'static str,
    /// Compatible GHC version
    pub ghc_compat: &'static str,
    /// Recommended BHC profile
    pub recommended_profile: &'static str,
    /// Number of packages
    pub package_count: usize,
}

/// TOML structure for parsing embedded platform data.
#[derive(Debug, Deserialize)]
struct PlatformToml {
    metadata: PlatformMetadataToml,
    packages: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct PlatformMetadataToml {
    #[allow(dead_code)]
    id: String,
    bhc_version: String,
    ghc_compat: String,
    recommended_profile: String,
    #[allow(dead_code)]
    created: Option<String>,
}

/// Diff between two BHC Platform snapshots.
#[derive(Debug, Clone)]
pub struct SnapshotDiff {
    /// Packages added in the new snapshot.
    pub added: Vec<(String, String)>,
    /// Packages removed in the new snapshot.
    pub removed: Vec<(String, String)>,
    /// Packages upgraded (name, old version, new version).
    pub upgraded: Vec<(String, String, String)>,
    /// Packages downgraded (name, old version, new version).
    pub downgraded: Vec<(String, String, String)>,
}

impl SnapshotDiff {
    /// Compute the diff between two snapshots.
    pub fn compute(old: &Snapshot, new: &Snapshot) -> Self {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut upgraded = Vec::new();
        let mut downgraded = Vec::new();

        // Find added and upgraded packages
        for (name, new_pkg) in &new.packages {
            match old.packages.get(name) {
                None => {
                    added.push((name.clone(), new_pkg.version.clone()));
                }
                Some(old_pkg) => {
                    if old_pkg.version != new_pkg.version {
                        // Compare versions lexicographically as a simple heuristic;
                        // PVP versions sort correctly this way for most cases.
                        if new_pkg.version > old_pkg.version {
                            upgraded.push((
                                name.clone(),
                                old_pkg.version.clone(),
                                new_pkg.version.clone(),
                            ));
                        } else {
                            downgraded.push((
                                name.clone(),
                                old_pkg.version.clone(),
                                new_pkg.version.clone(),
                            ));
                        }
                    }
                }
            }
        }

        // Find removed packages
        for (name, old_pkg) in &old.packages {
            if !new.packages.contains_key(name) {
                removed.push((name.clone(), old_pkg.version.clone()));
            }
        }

        // Sort all lists by package name for stable output
        added.sort_by(|a, b| a.0.cmp(&b.0));
        removed.sort_by(|a, b| a.0.cmp(&b.0));
        upgraded.sort_by(|a, b| a.0.cmp(&b.0));
        downgraded.sort_by(|a, b| a.0.cmp(&b.0));

        Self {
            added,
            removed,
            upgraded,
            downgraded,
        }
    }
}

/// An entry in the remote snapshot registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRegistryEntry {
    /// Platform identifier (e.g., "bhc-platform-2026.1")
    pub id: String,
    /// URL to download the snapshot TOML
    pub url: String,
    /// BHC version
    pub bhc_version: String,
    /// Compatible GHC version
    pub ghc_compat: String,
    /// Number of packages
    pub package_count: usize,
    /// SHA256 hash of the snapshot TOML file
    pub sha256: String,
    /// Creation date
    pub created: String,
    /// Hex-encoded Ed25519 signature of the snapshot content
    #[serde(default)]
    pub signature: Option<String>,
    /// Hex-encoded Ed25519 public key (per-snapshot override)
    #[serde(default)]
    pub public_key: Option<String>,
}

/// Remote snapshot registry index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRegistry {
    /// Available snapshots
    pub snapshots: Vec<SnapshotRegistryEntry>,
    /// When the registry was last updated
    pub updated: String,
    /// Registry-level default Ed25519 public key (hex-encoded)
    #[serde(default)]
    pub public_key: Option<String>,
}

/// Cache directory for BHC Platform data.
pub fn bhc_platform_cache_dir() -> Option<PathBuf> {
    dirs_next::cache_dir().map(|d: PathBuf| d.join("hx").join("bhc-platform"))
}

/// Get the cache path for the registry index.
pub fn registry_cache_path() -> Option<PathBuf> {
    bhc_platform_cache_dir().map(|d| d.join("registry.json"))
}

/// Get the cache path for a specific snapshot.
pub fn snapshot_cache_path(id: &str) -> Option<PathBuf> {
    bhc_platform_cache_dir().map(|d| d.join(format!("{}.toml", id)))
}

/// Verify SHA256 hash of content.
pub fn verify_sha256(content: &[u8], expected: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let result = format!("{:x}", hasher.finalize());
    result == expected
}

/// Verify an Ed25519 signature over content.
pub fn verify_ed25519(
    content: &[u8],
    signature_hex: &str,
    public_key_hex: &str,
) -> Result<(), SnapshotError> {
    let key_bytes = hex::decode(public_key_hex).map_err(|e| {
        SnapshotError::InvalidPublicKey(format!("failed to decode public key hex: {}", e))
    })?;

    let key_array: [u8; 32] = key_bytes.try_into().map_err(|_| {
        SnapshotError::InvalidPublicKey("public key must be exactly 32 bytes".to_string())
    })?;

    let verifying_key = VerifyingKey::from_bytes(&key_array).map_err(|e| {
        SnapshotError::InvalidPublicKey(format!("invalid Ed25519 public key: {}", e))
    })?;

    let sig_bytes = hex::decode(signature_hex).map_err(|e| {
        SnapshotError::SignatureInvalid(format!("failed to decode signature hex: {}", e))
    })?;

    let sig_array: [u8; 64] = sig_bytes.try_into().map_err(|_| {
        SnapshotError::SignatureInvalid("signature must be exactly 64 bytes".to_string())
    })?;

    let signature = Signature::from_bytes(&sig_array);

    verifying_key
        .verify_strict(content, &signature)
        .map_err(|e| SnapshotError::SignatureInvalid(format!("Ed25519 verification failed: {}", e)))
}

/// Fetch the snapshot registry from the remote server.
pub async fn fetch_snapshot_registry() -> Result<SnapshotRegistry, SnapshotError> {
    info!("Fetching BHC Platform snapshot registry");

    let client = reqwest::Client::builder()
        .user_agent("hx/0.6.0")
        .build()
        .map_err(|e| SnapshotError::FetchError(format!("failed to create HTTP client: {}", e)))?;

    let url = "https://bhc-platform.raskell.io/index.json";
    debug!("Fetching registry from: {}", url);

    let response =
        client.get(url).send().await.map_err(|e| {
            SnapshotError::RegistryOffline(format!("failed to fetch registry: {}", e))
        })?;

    if !response.status().is_success() {
        return Err(SnapshotError::RegistryOffline(format!(
            "registry returned HTTP {}",
            response.status()
        )));
    }

    let content = response.text().await.map_err(|e| {
        SnapshotError::FetchError(format!("failed to read registry response: {}", e))
    })?;

    let registry: SnapshotRegistry = serde_json::from_str(&content)
        .map_err(|e| SnapshotError::ParseError(format!("failed to parse registry JSON: {}", e)))?;

    // Cache the registry
    if let Some(cache_path) = registry_cache_path() {
        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&cache_path, &content) {
            warn!("Failed to cache registry: {}", e);
        }
    }

    Ok(registry)
}

/// Load registry from cache if fresh (within TTL), otherwise fetch.
pub async fn load_or_fetch_registry() -> Result<SnapshotRegistry, SnapshotError> {
    // Try cache first (24h TTL)
    if let Some(cache_path) = registry_cache_path()
        && cache_path.exists()
        && let Ok(metadata) = std::fs::metadata(&cache_path)
        && let Ok(modified) = metadata.modified()
    {
        let age = std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age < std::time::Duration::from_secs(24 * 60 * 60) {
            debug!("Loading registry from cache (age: {:?})", age);
            if let Ok(content) = std::fs::read_to_string(&cache_path)
                && let Ok(registry) = serde_json::from_str::<SnapshotRegistry>(&content)
            {
                return Ok(registry);
            }
        }
    }

    // Fetch from network
    fetch_snapshot_registry().await
}

/// Fetch a remote snapshot by ID, verify its SHA256, and cache locally.
pub async fn fetch_remote_snapshot(
    id: &str,
    registry: &SnapshotRegistry,
) -> Result<Snapshot, SnapshotError> {
    let entry = registry
        .snapshots
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| {
            SnapshotError::NotFound(format!("snapshot '{}' not found in remote registry", id))
        })?;

    info!("Fetching remote snapshot: {}", id);

    let client = reqwest::Client::builder()
        .user_agent("hx/0.6.0")
        .build()
        .map_err(|e| SnapshotError::FetchError(format!("failed to create HTTP client: {}", e)))?;

    let response = client.get(&entry.url).send().await.map_err(|e| {
        SnapshotError::FetchError(format!("failed to fetch snapshot '{}': {}", id, e))
    })?;

    if !response.status().is_success() {
        return Err(SnapshotError::FetchError(format!(
            "snapshot '{}' returned HTTP {}",
            id,
            response.status()
        )));
    }

    let content = response.text().await.map_err(|e| {
        SnapshotError::FetchError(format!("failed to read snapshot response: {}", e))
    })?;

    // Verify SHA256
    if !verify_sha256(content.as_bytes(), &entry.sha256) {
        return Err(SnapshotError::ChecksumMismatch(format!(
            "SHA256 mismatch for snapshot '{}': content does not match expected hash",
            id
        )));
    }

    // Verify Ed25519 signature if present
    if let Some(ref signature) = entry.signature {
        // Prefer a user-pinned key: a key delivered by the registry itself
        // cannot authenticate that same registry
        let pinned_key = std::env::var("HX_BHC_PLATFORM_PUBKEY").ok();
        let public_key = match pinned_key.as_deref() {
            Some(key) => key,
            None => {
                let key = entry
                    .public_key
                    .as_ref()
                    .or(registry.public_key.as_ref())
                    .ok_or_else(|| {
                        SnapshotError::InvalidPublicKey(
                            "snapshot has a signature but no public key is available".to_string(),
                        )
                    })?;
                warn!(
                    "Verifying snapshot '{}' with a public key supplied by the registry itself; \
                     set HX_BHC_PLATFORM_PUBKEY to pin an independent trust root",
                    id
                );
                key
            }
        };
        debug!("Verifying Ed25519 signature for snapshot: {}", id);
        verify_ed25519(content.as_bytes(), signature, public_key)?;
        info!("Signature verified for snapshot: {}", id);
    }

    // Cache locally
    if let Some(cache_path) = snapshot_cache_path(id) {
        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&cache_path, &content) {
            warn!("Failed to cache snapshot: {}", e);
        }
    }

    // Parse the snapshot
    let snapshot_id = SnapshotId::parse(id)?;
    parse_platform_toml(&content, &snapshot_id)
}

/// Check if a BHC version is compatible with a platform's BHC version.
///
/// Uses major.minor prefix matching: `0.2.3` is compatible with platform
/// requiring `0.2.0`, but `0.3.0` is not.
fn version_compatible(installed: &str, platform_version: &str) -> bool {
    let installed_parts: Vec<&str> = installed.split('.').collect();
    let platform_parts: Vec<&str> = platform_version.split('.').collect();

    if installed_parts.len() < 2 || platform_parts.len() < 2 {
        return false;
    }

    installed_parts[0] == platform_parts[0] && installed_parts[1] == platform_parts[1]
}

/// Find the best BHC Platform snapshot for an installed BHC version.
///
/// Uses major.minor prefix matching: `0.2.x` matches platforms with
/// `bhc_version` `0.2.0`, `0.3.x` matches `0.3.0`, etc.
/// Returns the latest compatible platform when multiple match.
pub fn find_platform_for_bhc(bhc_version: &str) -> Option<BhcPlatformInfo> {
    let platforms = list_platforms();
    platforms
        .into_iter()
        .rfind(|p| version_compatible(bhc_version, p.bhc_version))
}

/// Return the newest embedded BHC Platform snapshot.
pub fn latest_platform() -> Option<BhcPlatformInfo> {
    list_platforms().into_iter().last()
}

/// List all available BHC platform snapshots.
pub fn list_platforms() -> Vec<BhcPlatformInfo> {
    let mut platforms = Vec::new();

    // Parse each embedded platform to get metadata
    if let Ok(parsed) = toml::from_str::<PlatformToml>(BHC_PLATFORM_2026_1) {
        platforms.push(BhcPlatformInfo {
            id: "bhc-platform-2026.1",
            bhc_version: "0.2.0",
            ghc_compat: "9.8.2",
            recommended_profile: "default",
            package_count: parsed.packages.len(),
        });
    }

    if let Ok(parsed) = toml::from_str::<PlatformToml>(BHC_PLATFORM_2026_2) {
        platforms.push(BhcPlatformInfo {
            id: "bhc-platform-2026.2",
            bhc_version: "0.3.0",
            ghc_compat: "9.10.1",
            recommended_profile: "default",
            package_count: parsed.packages.len(),
        });
    }

    platforms
}

/// Parse platform TOML content into a Snapshot.
fn parse_platform_toml(
    toml_content: &str,
    snapshot_id: &SnapshotId,
) -> Result<Snapshot, SnapshotError> {
    let parsed: PlatformToml = toml::from_str(toml_content).map_err(|e| {
        SnapshotError::ParseError(format!("failed to parse BHC platform data: {}", e))
    })?;

    let mut packages = HashMap::new();
    for (name, version) in &parsed.packages {
        packages.insert(
            name.clone(),
            SnapshotPackage {
                name: name.clone(),
                version: version.clone(),
                flags: HashMap::new(),
                hidden: false,
            },
        );
    }

    let metadata = SnapshotMetadata {
        snapshot: snapshot_id.clone(),
        ghc_version: parsed.metadata.ghc_compat,
        package_count: packages.len(),
        created: parsed.metadata.created,
        bhc_version: Some(parsed.metadata.bhc_version),
        recommended_profile: Some(parsed.metadata.recommended_profile),
    };

    Ok(Snapshot { metadata, packages })
}

/// Load a BHC Platform snapshot by its identifier.
pub fn load_bhc_platform(snapshot_id: &SnapshotId) -> Result<Snapshot, SnapshotError> {
    let key = snapshot_id.key();
    debug!("Loading BHC platform snapshot: {}", key);

    let toml_content = match key.as_str() {
        "bhc-platform-2026.1" => BHC_PLATFORM_2026_1,
        "bhc-platform-2026.2" => BHC_PLATFORM_2026_2,
        _ => {
            return Err(SnapshotError::NotFound(format!(
                "BHC platform '{}' not found. Available: bhc-platform-2026.1, bhc-platform-2026.2",
                key
            )));
        }
    };

    parse_platform_toml(toml_content, snapshot_id)
}

/// Load a BHC Platform snapshot, trying embedded data first, then local cache,
/// then remote registry.
pub async fn load_bhc_platform_async(snapshot_id: &SnapshotId) -> Result<Snapshot, SnapshotError> {
    let key = snapshot_id.key();

    // Fast path: try embedded snapshots
    match key.as_str() {
        "bhc-platform-2026.1" | "bhc-platform-2026.2" => {
            return load_bhc_platform(snapshot_id);
        }
        _ => {}
    }

    // Try local cache
    if let Some(cache_path) = snapshot_cache_path(&key)
        && cache_path.exists()
    {
        debug!("Loading BHC platform from cache: {}", cache_path.display());
        if let Ok(content) = std::fs::read_to_string(&cache_path) {
            match parse_platform_toml(&content, snapshot_id) {
                Ok(snapshot) => return Ok(snapshot),
                Err(e) => {
                    warn!("Failed to parse cached snapshot: {}", e);
                }
            }
        }
    }

    // Try remote registry
    let registry = load_or_fetch_registry().await?;
    fetch_remote_snapshot(&key, &registry).await
}

/// Load an overlay snapshot from a TOML file on disk.
///
/// Overlay files use the same format as regular BHC platform TOML snapshots.
pub fn load_overlay(path: &Path) -> Result<Snapshot, SnapshotError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        SnapshotError::IoError(std::io::Error::new(
            e.kind(),
            format!("failed to read overlay file '{}': {}", path.display(), e),
        ))
    })?;

    // Use a synthetic snapshot ID derived from the filename
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("overlay");
    let snapshot_id = SnapshotId::parse("bhc-platform-0000.0").unwrap_or_else(|_| SnapshotId {
        snapshot_type: crate::snapshot::SnapshotType::BhcPlatform,
        raw: stem.to_string(),
        major: Some(0),
        minor: Some(0),
        date: None,
    });

    parse_platform_toml(&content, &snapshot_id).map_err(|e| {
        SnapshotError::ParseError(format!(
            "failed to parse overlay '{}': {}",
            path.display(),
            e
        ))
    })
}

/// Apply overlay snapshots on top of a base snapshot.
///
/// Overlays are applied in order; later overlays override earlier ones
/// for the same package name.
pub fn apply_overlays(base: Snapshot, overlays: &[Snapshot]) -> Snapshot {
    let mut packages = base.packages;

    for overlay in overlays {
        for (name, pkg) in &overlay.packages {
            packages.insert(name.clone(), pkg.clone());
        }
    }

    let metadata = SnapshotMetadata {
        package_count: packages.len(),
        ..base.metadata
    };

    Snapshot { metadata, packages }
}

/// Load a BHC Platform snapshot with overlays applied.
pub async fn load_bhc_platform_with_overlays(
    snapshot_id: &SnapshotId,
    overlay_paths: &[PathBuf],
) -> Result<Snapshot, SnapshotError> {
    let base = load_bhc_platform_async(snapshot_id).await?;

    if overlay_paths.is_empty() {
        return Ok(base);
    }

    let mut overlays = Vec::with_capacity(overlay_paths.len());
    for path in overlay_paths {
        debug!("Loading overlay: {}", path.display());
        let overlay = load_overlay(path)?;
        info!(
            "Loaded overlay '{}' with {} packages",
            path.display(),
            overlay.packages.len()
        );
        overlays.push(overlay);
    }

    Ok(apply_overlays(base, &overlays))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_platforms() {
        let platforms = list_platforms();
        assert!(!platforms.is_empty());
        assert_eq!(platforms[0].id, "bhc-platform-2026.1");
        assert_eq!(platforms[0].bhc_version, "0.2.0");
        assert!(platforms[0].package_count > 50);
    }

    #[test]
    fn test_list_platforms_returns_both() {
        let platforms = list_platforms();
        assert_eq!(platforms.len(), 2);
        assert_eq!(platforms[0].id, "bhc-platform-2026.1");
        assert_eq!(platforms[1].id, "bhc-platform-2026.2");
        assert_eq!(platforms[1].bhc_version, "0.3.0");
        assert_eq!(platforms[1].ghc_compat, "9.10.1");
    }

    #[test]
    fn test_load_bhc_platform() {
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let snapshot = load_bhc_platform(&id).unwrap();
        assert!(snapshot.packages.len() > 50);
        assert!(snapshot.contains("aeson"));
        assert!(snapshot.contains("servant"));
        assert!(snapshot.contains("hmatrix"));
        assert!(snapshot.contains("vector"));
        assert_eq!(snapshot.metadata.bhc_version, Some("0.2.0".to_string()));
    }

    #[test]
    fn test_load_bhc_platform_2026_2() {
        let id = SnapshotId::parse("bhc-platform-2026.2").unwrap();
        let snapshot = load_bhc_platform(&id).unwrap();
        assert!(
            snapshot.packages.len() >= 85,
            "expected ~90 packages, got {}",
            snapshot.packages.len()
        );
        assert!(snapshot.contains("aeson"));
        assert!(snapshot.contains("scotty"));
        assert!(snapshot.contains("cassava"));
        assert!(snapshot.contains("crypton"));
        assert!(snapshot.contains("streamly"));
        assert!(snapshot.contains("generics-sop"));
        assert!(snapshot.contains("dhall"));
        assert!(snapshot.contains("base64"));
        assert_eq!(snapshot.metadata.bhc_version, Some("0.3.0".to_string()));
        assert_eq!(snapshot.metadata.ghc_version, "9.10.1");
    }

    #[test]
    fn test_load_bhc_platform_not_found() {
        let id = SnapshotId::parse("bhc-platform-2099.9").unwrap();
        let err = load_bhc_platform(&id).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bhc-platform-2026.1"));
        assert!(msg.contains("bhc-platform-2026.2"));
    }

    #[test]
    fn test_snapshot_diff_added() {
        let id1 = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let id2 = SnapshotId::parse("bhc-platform-2026.2").unwrap();
        let snap1 = load_bhc_platform(&id1).unwrap();
        let snap2 = load_bhc_platform(&id2).unwrap();

        let diff = SnapshotDiff::compute(&snap1, &snap2);

        // 2026.2 adds many new packages
        assert!(!diff.added.is_empty(), "expected added packages");
        // Check specific additions
        let added_names: Vec<&str> = diff.added.iter().map(|(n, _)| n.as_str()).collect();
        assert!(added_names.contains(&"scotty"));
        assert!(added_names.contains(&"cassava"));
        assert!(added_names.contains(&"crypton"));
    }

    #[test]
    fn test_snapshot_diff_removed() {
        // Create two small snapshots for testing
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let mut old = load_bhc_platform(&id).unwrap();
        let new = old.clone();

        // Add a package to old that's not in new
        old.packages.insert(
            "fake-removed-pkg".to_string(),
            SnapshotPackage {
                name: "fake-removed-pkg".to_string(),
                version: "1.0.0".to_string(),
                flags: HashMap::new(),
                hidden: false,
            },
        );

        let diff = SnapshotDiff::compute(&old, &new);
        let removed_names: Vec<&str> = diff.removed.iter().map(|(n, _)| n.as_str()).collect();
        assert!(removed_names.contains(&"fake-removed-pkg"));
    }

    #[test]
    fn test_snapshot_diff_upgraded() {
        let id1 = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let id2 = SnapshotId::parse("bhc-platform-2026.2").unwrap();
        let snap1 = load_bhc_platform(&id1).unwrap();
        let snap2 = load_bhc_platform(&id2).unwrap();

        let diff = SnapshotDiff::compute(&snap1, &snap2);

        // Several packages should be upgraded between snapshots
        assert!(!diff.upgraded.is_empty(), "expected upgraded packages");
        // base was upgraded from 4.19.1.0 to 4.20.0.0
        let base_upgrade = diff.upgraded.iter().find(|(name, _, _)| name == "base");
        assert!(base_upgrade.is_some(), "base should be upgraded");
        let (_, old_ver, new_ver) = base_upgrade.unwrap();
        assert_eq!(old_ver, "4.19.1.0");
        assert_eq!(new_ver, "4.20.0.0");
    }

    #[test]
    fn test_snapshot_diff_identical() {
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let snap = load_bhc_platform(&id).unwrap();
        let diff = SnapshotDiff::compute(&snap, &snap);

        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.upgraded.is_empty());
        assert!(diff.downgraded.is_empty());
    }

    #[test]
    fn test_sha256_verification() {
        let content = b"hello world";
        // Known SHA256 of "hello world"
        let correct_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert!(verify_sha256(content, correct_hash));
        assert!(!verify_sha256(
            content,
            "0000000000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn test_parse_registry_json() {
        let json = r#"{
            "snapshots": [
                {
                    "id": "bhc-platform-2026.1",
                    "url": "https://bhc-platform.raskell.io/snapshots/bhc-platform-2026.1.toml",
                    "bhc_version": "0.2.0",
                    "ghc_compat": "9.8.2",
                    "package_count": 70,
                    "sha256": "abc123",
                    "created": "2026-01-15"
                }
            ],
            "updated": "2026-06-15T00:00:00Z"
        }"#;
        let registry: SnapshotRegistry = serde_json::from_str(json).unwrap();
        assert_eq!(registry.snapshots.len(), 1);
        assert_eq!(registry.snapshots[0].id, "bhc-platform-2026.1");
        assert_eq!(registry.snapshots[0].package_count, 70);
        assert_eq!(registry.updated, "2026-06-15T00:00:00Z");
    }

    #[tokio::test]
    async fn test_load_bhc_platform_async_embedded_first() {
        // Embedded snapshots should be returned immediately without network
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let snapshot = load_bhc_platform_async(&id).await.unwrap();
        assert!(snapshot.contains("aeson"));
        assert_eq!(snapshot.metadata.bhc_version, Some("0.2.0".to_string()));
    }

    // ─── Ed25519 signature tests ──────────────────────────────────────────────

    #[test]
    fn test_verify_ed25519_valid() {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[1u8; 32]);
        let content = b"snapshot content";
        let signature = signing_key.sign(content);

        let sig_hex = hex::encode(signature.to_bytes());
        let pub_hex = hex::encode(signing_key.verifying_key().to_bytes());

        assert!(verify_ed25519(content, &sig_hex, &pub_hex).is_ok());
    }

    #[test]
    fn test_verify_ed25519_invalid_signature() {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[1u8; 32]);
        let content = b"snapshot content";
        let _signature = signing_key.sign(content);

        // Use a different (wrong) signature
        let wrong_sig = hex::encode([0xab; 64]);
        let pub_hex = hex::encode(signing_key.verifying_key().to_bytes());

        let err = verify_ed25519(content, &wrong_sig, &pub_hex).unwrap_err();
        assert!(
            matches!(err, SnapshotError::SignatureInvalid(_)),
            "expected SignatureInvalid, got: {}",
            err
        );
    }

    #[test]
    fn test_verify_ed25519_invalid_key() {
        let err = verify_ed25519(b"content", "aa".repeat(64).as_str(), "not-hex").unwrap_err();
        assert!(
            matches!(err, SnapshotError::InvalidPublicKey(_)),
            "expected InvalidPublicKey, got: {}",
            err
        );

        // Wrong length key
        let err = verify_ed25519(b"content", "aa".repeat(64).as_str(), "aabb").unwrap_err();
        assert!(
            matches!(err, SnapshotError::InvalidPublicKey(_)),
            "expected InvalidPublicKey for wrong length, got: {}",
            err
        );
    }

    #[test]
    fn test_registry_with_signatures_parses() {
        let json = r#"{
            "snapshots": [
                {
                    "id": "bhc-platform-2026.1",
                    "url": "https://example.com/snapshot.toml",
                    "bhc_version": "0.2.0",
                    "ghc_compat": "9.8.2",
                    "package_count": 70,
                    "sha256": "abc123",
                    "created": "2026-01-15",
                    "signature": "deadbeef",
                    "public_key": "cafebabe"
                }
            ],
            "updated": "2026-06-15T00:00:00Z",
            "public_key": "registrykey123"
        }"#;
        let registry: SnapshotRegistry = serde_json::from_str(json).unwrap();
        assert_eq!(registry.public_key, Some("registrykey123".to_string()));
        assert_eq!(
            registry.snapshots[0].signature,
            Some("deadbeef".to_string())
        );
        assert_eq!(
            registry.snapshots[0].public_key,
            Some("cafebabe".to_string())
        );
    }

    #[test]
    fn test_registry_without_signatures_parses() {
        let json = r#"{
            "snapshots": [
                {
                    "id": "bhc-platform-2026.1",
                    "url": "https://example.com/snapshot.toml",
                    "bhc_version": "0.2.0",
                    "ghc_compat": "9.8.2",
                    "package_count": 70,
                    "sha256": "abc123",
                    "created": "2026-01-15"
                }
            ],
            "updated": "2026-06-15T00:00:00Z"
        }"#;
        let registry: SnapshotRegistry = serde_json::from_str(json).unwrap();
        assert!(registry.public_key.is_none());
        assert!(registry.snapshots[0].signature.is_none());
        assert!(registry.snapshots[0].public_key.is_none());
    }

    // ─── Overlay tests ────────────────────────────────────────────────────────

    #[test]
    fn test_load_overlay_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let overlay_path = dir.path().join("overlay.toml");
        std::fs::write(
            &overlay_path,
            r#"
[metadata]
id = "overlay"
bhc_version = "0.0.0"
ghc_compat = "9.8.2"
recommended_profile = "default"

[packages]
my-internal-lib = "1.0.0"
custom-utils = "2.3.0"
"#,
        )
        .unwrap();

        let overlay = load_overlay(&overlay_path).unwrap();
        assert_eq!(overlay.packages.len(), 2);
        assert!(overlay.contains("my-internal-lib"));
        assert!(overlay.contains("custom-utils"));
        assert_eq!(overlay.get_version("my-internal-lib"), Some("1.0.0"));
    }

    #[test]
    fn test_apply_overlay_adds_packages() {
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let base = load_bhc_platform(&id).unwrap();
        let base_count = base.packages.len();

        assert!(!base.contains("my-new-package"));

        let mut overlay = Snapshot {
            metadata: base.metadata.clone(),
            packages: HashMap::new(),
        };
        overlay.packages.insert(
            "my-new-package".to_string(),
            SnapshotPackage {
                name: "my-new-package".to_string(),
                version: "1.0.0".to_string(),
                flags: HashMap::new(),
                hidden: false,
            },
        );

        let merged = apply_overlays(base, &[overlay]);
        assert!(merged.contains("my-new-package"));
        assert_eq!(merged.packages.len(), base_count + 1);
        assert_eq!(merged.metadata.package_count, base_count + 1);
    }

    #[test]
    fn test_apply_overlay_replaces_version() {
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let base = load_bhc_platform(&id).unwrap();
        let original_version = base.get_version("aeson").unwrap().to_string();

        let mut overlay = Snapshot {
            metadata: base.metadata.clone(),
            packages: HashMap::new(),
        };
        overlay.packages.insert(
            "aeson".to_string(),
            SnapshotPackage {
                name: "aeson".to_string(),
                version: "99.0.0".to_string(),
                flags: HashMap::new(),
                hidden: false,
            },
        );

        let merged = apply_overlays(base, &[overlay]);
        assert_eq!(merged.get_version("aeson"), Some("99.0.0"));
        assert_ne!(original_version, "99.0.0");
    }

    #[test]
    fn test_apply_multiple_overlays_order() {
        let id = SnapshotId::parse("bhc-platform-2026.1").unwrap();
        let base = load_bhc_platform(&id).unwrap();

        let make_overlay = |version: &str| {
            let mut overlay = Snapshot {
                metadata: base.metadata.clone(),
                packages: HashMap::new(),
            };
            overlay.packages.insert(
                "test-pkg".to_string(),
                SnapshotPackage {
                    name: "test-pkg".to_string(),
                    version: version.to_string(),
                    flags: HashMap::new(),
                    hidden: false,
                },
            );
            overlay
        };

        let overlay1 = make_overlay("1.0.0");
        let overlay2 = make_overlay("2.0.0");

        // Later overlay should win
        let merged = apply_overlays(base, &[overlay1, overlay2]);
        assert_eq!(merged.get_version("test-pkg"), Some("2.0.0"));
    }

    #[test]
    fn test_overlay_file_not_found() {
        let err = load_overlay(Path::new("/nonexistent/overlay.toml")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("/nonexistent/overlay.toml"),
            "error should contain the path: {}",
            msg
        );
    }

    // ─── find_platform_for_bhc / latest_platform tests ──────────────────────

    #[test]
    fn test_find_platform_for_bhc_exact() {
        let p = find_platform_for_bhc("0.2.0").unwrap();
        assert_eq!(p.id, "bhc-platform-2026.1");
    }

    #[test]
    fn test_find_platform_for_bhc_patch() {
        // 0.2.3 should match the 0.2.0 platform
        let p = find_platform_for_bhc("0.2.3").unwrap();
        assert_eq!(p.id, "bhc-platform-2026.1");
    }

    #[test]
    fn test_find_platform_for_bhc_0_3() {
        let p = find_platform_for_bhc("0.3.0").unwrap();
        assert_eq!(p.id, "bhc-platform-2026.2");
    }

    #[test]
    fn test_find_platform_for_bhc_no_match() {
        assert!(find_platform_for_bhc("0.9.0").is_none());
        assert!(find_platform_for_bhc("1.0.0").is_none());
    }

    #[test]
    fn test_latest_platform() {
        let p = latest_platform().unwrap();
        assert_eq!(p.id, "bhc-platform-2026.2");
    }

    #[test]
    fn test_version_compatible() {
        assert!(version_compatible("0.2.0", "0.2.0"));
        assert!(version_compatible("0.2.5", "0.2.0"));
        assert!(!version_compatible("0.3.0", "0.2.0"));
        assert!(!version_compatible("1.2.0", "0.2.0"));
        assert!(!version_compatible("bad", "0.2.0"));
    }
}
