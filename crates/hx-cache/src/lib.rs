//! Cache management for hx.
//!
//! This crate handles:
//! - Global cache directories
//! - Project-local cache (.hx/)
//! - Cache keys and artifact storage
//! - Shared build store with fingerprint tracking
//! - Source fingerprinting for incremental builds
//! - Build state tracking
//! - Binary artifact caching for compiled modules

pub mod artifacts;
pub mod bhc_store;
pub mod build_state;
pub mod source;
pub mod store;

use directories::ProjectDirs;
use hx_core::error::{Error, Result};
use std::path::{Path, PathBuf};
use tracing::debug;

pub use artifacts::{
    ArtifactEntry, ArtifactIndex, ArtifactStats, PruneResult, clear_artifacts,
    compute_artifact_hash, hash_file, prune_artifacts, retrieve_artifacts, store_artifacts,
};
pub use bhc_store::{BhcPackageCacheEntry, BhcPackageCacheIndex, calculate_bhc_cache_key};
pub use build_state::{BuildState, PackageBuildInfo, PackageStatus};
pub use source::{
    SourceFingerprint, compute_source_fingerprint, load_source_fingerprint, save_source_fingerprint,
};
pub use store::{
    BuildCacheEntry, PackageCacheEntry, PackageCacheIndex, PackageCacheStats, StoreIndex,
    StoreStats, calculate_fingerprint, calculate_package_cache_key, store_disk_size,
};

/// Get the global cache directory.
///
/// Respects `$XDG_CACHE_HOME` if set to a non-empty value, on any platform.
/// Otherwise falls back to the platform default:
/// - Linux: `~/.cache/hx`
/// - macOS: `~/Library/Caches/io.raskell.hx`
/// - Windows: `%LOCALAPPDATA%\raskell\hx\cache`
pub fn global_cache_dir() -> Result<PathBuf> {
    if let Some(dir) = xdg_dir(std::env::var("XDG_CACHE_HOME").ok()) {
        return Ok(dir);
    }
    let dirs = ProjectDirs::from("io", "raskell", "hx")
        .ok_or_else(|| Error::config("could not determine home directory for cache"))?;
    Ok(dirs.cache_dir().to_path_buf())
}

/// Get the Cabal store directory within the global cache.
///
/// Always `<global cache>/cabal/store`, so it moves with the platform cache
/// directory rather than living under `~/.hx`.
pub fn cabal_store_dir() -> Result<PathBuf> {
    Ok(global_cache_dir()?.join("cabal").join("store"))
}

/// Get the global config directory.
///
/// Respects `$XDG_CONFIG_HOME` if set to a non-empty value, on any platform.
/// Otherwise falls back to the platform default:
/// - Linux: `~/.config/hx`
/// - macOS: `~/Library/Application Support/io.raskell.hx`
/// - Windows: `%APPDATA%\raskell\hx\config`
pub fn global_config_dir() -> Result<PathBuf> {
    if let Some(dir) = xdg_dir(std::env::var("XDG_CONFIG_HOME").ok()) {
        return Ok(dir);
    }
    let dirs = ProjectDirs::from("io", "raskell", "hx")
        .ok_or_else(|| Error::config("could not determine home directory for config"))?;
    Ok(dirs.config_dir().to_path_buf())
}

fn xdg_dir(env_value: Option<String>) -> Option<PathBuf> {
    env_value
        .filter(|v| !v.is_empty())
        .map(|v| PathBuf::from(v).join("hx"))
}

/// Get the global config file path.
///
/// - Linux: `~/.config/hx/config.toml`
/// - macOS: `~/Library/Application Support/io.raskell.hx/config.toml`
/// - Windows: `%APPDATA%\raskell\hx\config\config.toml`
pub fn global_config_file() -> Result<PathBuf> {
    Ok(global_config_dir()?.join("config.toml"))
}

/// Get the toolchain directory for hx-managed installations.
///
/// Uses `~/.hx/toolchains` on all platforms. This avoids paths with spaces
/// which cause issues with GHC's build system (especially on macOS where
/// the standard data directory contains "Application Support").
///
/// Structure:
/// ```text
/// ~/.hx/toolchains/
///   ghc/
///     9.8.2/bin/ghc, ghc-pkg, ...
///     9.6.4/...
///   cabal/
///     3.12.1.0/bin/cabal
///   downloads/
///     ghc-9.8.2-aarch64-apple-darwin.tar.xz
///   manifest.json
/// ```
pub fn toolchain_dir() -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .ok_or_else(|| Error::config("could not determine home directory"))?;
    Ok(home.home_dir().join(".hx").join("toolchains"))
}

/// Get the bin directory for hx-managed tool symlinks.
///
/// Uses `~/.hx/bin` on all platforms.
///
/// This directory can be added to PATH for direct access to hx-managed tools.
pub fn toolchain_bin_dir() -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .ok_or_else(|| Error::config("could not determine home directory"))?;
    Ok(home.home_dir().join(".hx").join("bin"))
}

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
pub fn clean_global_cache() -> Result<()> {
    let cache_dir = global_cache_dir()?;
    if cache_dir.exists() {
        debug!("Removing global cache: {}", cache_dir.display());
        std::fs::remove_dir_all(&cache_dir).map_err(|e| Error::Io {
            message: "failed to remove global cache".to_string(),
            path: Some(cache_dir),
            source: e,
        })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn xdg_dir_appends_hx_to_set_value() {
        assert_eq!(
            xdg_dir(Some("/custom/config".to_string())),
            Some(PathBuf::from("/custom/config/hx"))
        );
    }

    #[test]
    fn xdg_dir_falls_back_when_unset() {
        assert_eq!(xdg_dir(None), None);
    }

    #[test]
    fn xdg_dir_falls_back_when_empty() {
        assert_eq!(xdg_dir(Some(String::new())), None);
    }

    // std::env::set_var is process-global and `cargo test` runs tests in
    // parallel by default, so any test that touches real XDG_* env vars
    // must serialize against the others via this lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn global_config_and_cache_dirs_use_distinct_xdg_vars() {
        let _guard = ENV_LOCK.lock().unwrap();

        // SAFETY: serialized by ENV_LOCK against other tests in this module.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/tmp/hx-xdg-test/config");
            std::env::set_var("XDG_CACHE_HOME", "/tmp/hx-xdg-test/cache");
        }

        let config = global_config_dir().unwrap();
        let cache = global_cache_dir().unwrap();

        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("XDG_CACHE_HOME");
        }

        assert_ne!(
            config, cache,
            "config and cache dirs must not collapse to the same path"
        );
        assert_eq!(config, PathBuf::from("/tmp/hx-xdg-test/config/hx"));
        assert_eq!(cache, PathBuf::from("/tmp/hx-xdg-test/cache/hx"));
    }

    #[test]
    fn global_config_dir_ignores_xdg_cache_home() {
        let _guard = ENV_LOCK.lock().unwrap();

        // SAFETY: serialized by ENV_LOCK against other tests in this module.
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::set_var("XDG_CACHE_HOME", "/tmp/hx-xdg-test/cache-only");
        }

        let config = global_config_dir().unwrap();

        unsafe {
            std::env::remove_var("XDG_CACHE_HOME");
        }

        assert_ne!(
            config,
            PathBuf::from("/tmp/hx-xdg-test/cache-only/hx"),
            "XDG_CACHE_HOME must not leak into the config dir resolution"
        );
    }
}
