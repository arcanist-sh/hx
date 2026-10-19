//! Cache management for hx.
//!
//! This crate handles:
//! - Every user-level location hx uses ([`paths`])
//! - Project-local cache (.hx/)
//! - Cache keys and artifact storage
//! - Shared build store with fingerprint tracking
//! - Source fingerprinting for incremental builds
//! - Build state tracking
//! - Binary artifact caching for compiled modules

pub mod artifacts;
pub mod bhc_store;
pub mod build_state;
pub mod paths;
pub mod source;
pub mod store;

use hx_core::error::{Error, Result};
use std::path::{Path, PathBuf};
use tracing::debug;

pub use artifacts::{
    ArtifactEntry, ArtifactIndex, ArtifactStats, PruneResult, clear_artifacts,
    compute_artifact_hash, hash_file, prune_artifacts, retrieve_artifacts, store_artifacts,
};
pub use bhc_store::{BhcPackageCacheEntry, BhcPackageCacheIndex, calculate_bhc_cache_key};
pub use build_state::{BuildState, PackageBuildInfo, PackageStatus};
pub use paths::{
    BaseLocations, HxPaths, LegacyRoot, PathSource, ResolvedPath, bhc_home_dir,
    bhc_platform_cache_dir, cabal_store_dir, global_cache_dir, global_config_dir,
    global_config_file, global_plugin_dirs, home_dir, hx_home_dir, index_mirror_dir, installed_manifest_path,
    package_store_dir, script_cache_dir, set_config_file_override, solver_cache_dir,
    stackage_cache_dir, toolchain_bin_dir, toolchain_dir,
};
pub use source::{
    SourceFingerprint, compute_source_fingerprint, load_source_fingerprint, save_source_fingerprint,
};
pub use store::{
    BuildCacheEntry, PackageCacheEntry, PackageCacheIndex, PackageCacheStats, StoreIndex,
    StoreStats, calculate_fingerprint, calculate_package_cache_key, store_disk_size,
};

/// Ensure a directory exists.
///
/// On Unix, newly created cache directories are restricted to the owner
/// (0o700) so other local users cannot tamper with cached artifacts.
pub fn ensure_dir(path: &PathBuf) -> Result<()> {
    if !path.exists() {
        debug!("Creating directory: {}", path.display());
        std::fs::create_dir_all(path).map_err(|e| Error::Io {
            message: format!("failed to create directory: {}", path.display()),
            path: Some(path.clone()),
            source: e,
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(
                |e| Error::Io {
                    message: format!("failed to set permissions on: {}", path.display()),
                    path: Some(path.clone()),
                    source: e,
                },
            )?;
        }
    }
    Ok(())
}

/// Clean the global cache.
///
/// Removes every cache root hx writes to: the global cache directory plus
/// any older-layout cache location that does not live under it (see
/// [`HxPaths::cache_roots`]).
pub fn clean_global_cache() -> Result<()> {
    for cache_dir in HxPaths::current()?.cache_roots() {
        if cache_dir.exists() {
            debug!("Removing global cache: {}", cache_dir.display());
            std::fs::remove_dir_all(&cache_dir).map_err(|e| Error::Io {
                message: "failed to remove global cache".to_string(),
                path: Some(cache_dir),
                source: e,
            })?;
        }
    }
    Ok(())
}

/// Clean a project's local cache.
pub fn clean_project_cache(project_root: &Path) -> Result<()> {
    let cache_dir = project_root.join(".hx");
    if cache_dir.exists() {
        debug!("Removing project cache: {}", cache_dir.display());
        std::fs::remove_dir_all(&cache_dir).map_err(|e| Error::Io {
            message: "failed to remove project cache".to_string(),
            path: Some(cache_dir),
            source: e,
        })?;
    }
    Ok(())
}
