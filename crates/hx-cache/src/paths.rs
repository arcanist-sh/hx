//! The single source of truth for every user-level location hx reads or writes.
//!
//! Every crate asks this module (through the free functions re-exported from
//! the crate root, or [`HxPaths`]) instead of computing `~/.cache/hx/...`
//! itself, so the override environment variables apply everywhere:
//!
//! | Variable          | Effect                                                    |
//! |-------------------|-----------------------------------------------------------|
//! | `HX_CACHE_DIR`    | Root for every cache location (used as-is, no `hx` added) |
//! | `XDG_CACHE_HOME`  | Cache root becomes `$XDG_CACHE_HOME/hx`                   |
//! | `HX_STORE_DIR`    | The Cabal store only                                      |
//! | `XDG_CONFIG_HOME` | Config directory becomes `$XDG_CONFIG_HOME/hx`            |
//! | `HX_CONFIG_FILE`  | The global config file only                               |
//! | `HX_HOME`         | Replaces `~/.hx` (toolchains and tool symlinks)           |
//!
//! `HX_*` variables win over `XDG_*` ones. `XDG_*` values that are empty or
//! relative are ignored, as the XDG Base Directory specification requires.
//!
//! # Stable defaults
//!
//! When none of the variables is set, every location keeps the default it
//! had in earlier releases, so upgrading never orphans existing data. Some
//! of those defaults predate the unified layout and do not live under
//! [`global_cache_dir`] on macOS and Windows; they are tagged with a
//! [`LegacyRoot`] below. On Linux all of them coincide with `~/.cache/hx`.
//!
//! `XDG_DATA_HOME` deliberately does not move `~/.hx`: toolchains are
//! installed there, GHC's build system cannot cope with the spaces in the
//! macOS data directory, and moving it would hide existing installs. Use
//! `HX_HOME` to relocate it explicitly.

use hx_core::env::EnvVars;
use hx_core::error::{Error, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where a resolved location came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSource {
    /// The built-in platform default.
    Default,
    /// An environment variable (named).
    Env(&'static str),
    /// A command-line flag (named).
    Flag(&'static str),
}

impl std::fmt::Display for PathSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathSource::Default => write!(f, "default"),
            PathSource::Env(var) => write!(f, "${var}"),
            PathSource::Flag(flag) => write!(f, "{flag}"),
        }
    }
}

/// A location together with the reason hx picked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// The effective path.
    pub path: PathBuf,
    /// What decided it.
    pub source: PathSource,
}

impl ResolvedPath {
    fn new(path: PathBuf, source: PathSource) -> Self {
        Self { path, source }
    }

    fn join(&self, sub: impl AsRef<Path>) -> Self {
        Self::new(self.path.join(sub), self.source)
    }
}

/// The platform directories hx derives its defaults from.
///
/// Normally produced by [`BaseLocations::detect`]; tests build one by hand to
/// check the layout for any platform without touching the real home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseLocations {
    /// The user's home directory.
    pub home: PathBuf,
    /// hx's own cache directory (`ProjectDirs::from("io", "raskell", "hx")`).
    pub project_cache: PathBuf,
    /// hx's own config directory (`ProjectDirs::from("io", "raskell", "hx")`).
    pub project_config: PathBuf,
    /// The platform cache root without an hx component (`BaseDirs::cache_dir`).
    pub platform_cache: PathBuf,
    /// The platform config root without an hx component (`BaseDirs::config_dir`).
    pub platform_config: PathBuf,
}

impl BaseLocations {
    /// Detect the platform directories for the current user.
    pub fn detect() -> Result<Self> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| Error::config("could not determine home directory"))?;
        let project = directories::ProjectDirs::from("io", "raskell", "hx")
            .ok_or_else(|| Error::config("could not determine home directory"))?;
        Ok(Self {
            home: base.home_dir().to_path_buf(),
            project_cache: project.cache_dir().to_path_buf(),
            project_config: project.config_dir().to_path_buf(),
            platform_cache: base.cache_dir().to_path_buf(),
            platform_config: base.config_dir().to_path_buf(),
        })
    }
}

/// Which pre-unification default a cache location keeps when no override is
/// set. Only matters on macOS and Windows; on Linux all three are
/// `~/.cache/hx` (or `$XDG_CACHE_HOME/hx`, which is an override anyway).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyRoot {
    /// [`BaseLocations::project_cache`], the documented global cache.
    Project,
    /// `~/.cache/hx` on every platform (hx-solver's historical root).
    HomeDotCache,
    /// `<platform cache>/hx`, e.g. `~/Library/Caches/hx` on macOS.
    Platform,
}

/// Every user-level location hx uses, fully resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HxPaths {
    /// Root of hx's cache (artifacts, store indexes, BHC package cache).
    pub cache: ResolvedPath,
    /// Cabal store used by hx-driven builds.
    pub cabal_store: ResolvedPath,
    /// Dependency-resolution cache.
    pub solver_cache: ResolvedPath,
    /// Downloaded package sources and the native package store (GHC/BHC package DBs).
    pub package_store: ResolvedPath,
    /// Mirrored Hackage index.
    pub index_mirror: ResolvedPath,
    /// Cached Stackage snapshots.
    pub stackage_cache: ResolvedPath,
    /// Cached BHC Platform registry and snapshots.
    pub bhc_platform_cache: ResolvedPath,
    /// Compiled `hx script` executables.
    pub script_cache: ResolvedPath,
    /// Global config directory.
    pub config_dir: ResolvedPath,
    /// Global config file.
    pub config_file: ResolvedPath,
    /// Global plugin directories, in search order (the first is where new
    /// plugins belong; later ones are kept so existing plugins keep loading).
    pub plugin_dirs: Vec<ResolvedPath>,
    /// hx home (`~/.hx`).
    pub hx_home: ResolvedPath,
    /// hx-managed GHC/Cabal/HLS installs.
    pub toolchains: ResolvedPath,
    /// Symlinks to the active hx-managed tools.
    pub toolchain_bin: ResolvedPath,
    /// BHC installs (`~/.bhc`, BHC's own convention).
    pub bhc_home: ResolvedPath,
}

impl HxPaths {
    /// Resolve every location from an environment lookup and platform base
    /// directories. Pure: does not read the process environment or touch disk.
    pub fn resolve(env: impl Fn(&str) -> Option<OsString>, base: &BaseLocations) -> Self {
        let cache_override = cache_override(&env);
        let cache_at = |sub: &str, legacy: LegacyRoot| -> ResolvedPath {
            match &cache_override {
                Some(root) if sub.is_empty() => root.clone(),
                Some(root) => root.join(sub),
                None => {
                    let root = match legacy {
                        LegacyRoot::Project => base.project_cache.clone(),
                        LegacyRoot::HomeDotCache => base.home.join(".cache").join("hx"),
                        LegacyRoot::Platform => base.platform_cache.join("hx"),
                    };
                    let path = if sub.is_empty() { root } else { root.join(sub) };
                    ResolvedPath::new(path, PathSource::Default)
                }
            }
        };

        let cache = cache_at("", LegacyRoot::Project);
        let cabal_store = match explicit(&env, EnvVars::HX_STORE_DIR) {
            Some(dir) => ResolvedPath::new(dir, PathSource::Env(EnvVars::HX_STORE_DIR)),
            None => cache.join("cabal").join("store"),
        };

        let config_dir = match xdg(&env, EnvVars::XDG_CONFIG_HOME) {
            Some(dir) => {
                ResolvedPath::new(dir.join("hx"), PathSource::Env(EnvVars::XDG_CONFIG_HOME))
            }
            None => ResolvedPath::new(base.project_config.clone(), PathSource::Default),
        };
        let config_file = match explicit(&env, EnvVars::HX_CONFIG_FILE) {
            Some(file) => ResolvedPath::new(file, PathSource::Env(EnvVars::HX_CONFIG_FILE)),
            None => config_dir.join("config.toml"),
        };

        // Plugins used to live in `<platform config>/hx/plugins`, which on
        // macOS and Windows is not hx's config directory. Keep searching it.
        let mut plugin_dirs = vec![config_dir.join("plugins")];
        let legacy_plugins = base.platform_config.join("hx").join("plugins");
        if legacy_plugins != plugin_dirs[0].path {
            plugin_dirs.push(ResolvedPath::new(legacy_plugins, PathSource::Default));
        }

        let hx_home = match explicit(&env, EnvVars::HX_HOME) {
            Some(dir) => ResolvedPath::new(dir, PathSource::Env(EnvVars::HX_HOME)),
            None => ResolvedPath::new(base.home.join(".hx"), PathSource::Default),
        };

        HxPaths {
            solver_cache: cache_at("solver", LegacyRoot::HomeDotCache),
            package_store: cache_at("packages", LegacyRoot::HomeDotCache),
            index_mirror: cache_at("index", LegacyRoot::HomeDotCache),
            stackage_cache: cache_at("stackage", LegacyRoot::Platform),
            bhc_platform_cache: cache_at("bhc-platform", LegacyRoot::Platform),
            script_cache: cache_at("scripts", LegacyRoot::Platform),
            cache,
            cabal_store,
            config_dir,
            config_file,
            plugin_dirs,
            toolchains: hx_home.join("toolchains"),
            toolchain_bin: hx_home.join("bin"),
            hx_home,
            bhc_home: ResolvedPath::new(base.home.join(".bhc"), PathSource::Default),
        }
    }

    /// Resolve every location for the current process: real environment,
    /// real platform directories, and the `--config-file` flag if the CLI
    /// registered one with [`set_config_file_override`].
    pub fn current() -> Result<Self> {
        let base = BaseLocations::detect()?;
        let mut paths = Self::resolve(|key| std::env::var_os(key), &base);
        if let Some(file) = CONFIG_FILE_OVERRIDE.get() {
            paths.config_file = ResolvedPath::new(file.clone(), PathSource::Flag("--config-file"));
        }
        Ok(paths)
    }

    /// Every location as `(label, path)` rows, in display order.
    pub fn entries(&self) -> Vec<(&'static str, &ResolvedPath)> {
        let mut rows = vec![
            ("config file", &self.config_file),
            ("config dir", &self.config_dir),
        ];
        for dir in &self.plugin_dirs {
            rows.push(("plugins", dir));
        }
        rows.extend([
            ("cache", &self.cache),
            ("cabal store", &self.cabal_store),
            ("solver cache", &self.solver_cache),
            ("package store", &self.package_store),
            ("index mirror", &self.index_mirror),
            ("stackage cache", &self.stackage_cache),
            ("bhc platform cache", &self.bhc_platform_cache),
            ("script cache", &self.script_cache),
            ("hx home", &self.hx_home),
            ("toolchains", &self.toolchains),
            ("toolchain bin", &self.toolchain_bin),
            ("bhc home", &self.bhc_home),
        ]);
        rows
    }

    /// Distinct cache directories hx writes to (the cache root plus any
    /// legacy-rooted cache that does not live under it).
    pub fn cache_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.cache.path.clone()];
        for loc in [
            &self.solver_cache,
            &self.package_store,
            &self.index_mirror,
            &self.stackage_cache,
            &self.bhc_platform_cache,
            &self.script_cache,
        ] {
            if !loc.path.starts_with(&self.cache.path) && !roots.contains(&loc.path) {
                roots.push(loc.path.clone());
            }
        }
        roots
    }
}

static CONFIG_FILE_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Register the config file given with `--config-file` so every later
/// [`global_config_file`] call in this process returns it. First call wins.
pub fn set_config_file_override(path: PathBuf) {
    let _ = CONFIG_FILE_OVERRIDE.set(path);
}

fn cache_override(env: &impl Fn(&str) -> Option<OsString>) -> Option<ResolvedPath> {
    if let Some(dir) = explicit(env, EnvVars::HX_CACHE_DIR) {
        return Some(ResolvedPath::new(
            dir,
            PathSource::Env(EnvVars::HX_CACHE_DIR),
        ));
    }
    xdg(env, EnvVars::XDG_CACHE_HOME)
        .map(|dir| ResolvedPath::new(dir.join("hx"), PathSource::Env(EnvVars::XDG_CACHE_HOME)))
}

/// An `HX_*` override: any non-empty value, used as given.
fn explicit(env: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<PathBuf> {
    env(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// An `XDG_*` base directory: only absolute values count (XDG spec).
fn xdg(env: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<PathBuf> {
    env(key).map(PathBuf::from).filter(|p| p.is_absolute())
}

fn current() -> Result<HxPaths> {
    HxPaths::current()
}

/// The user's home directory.
pub fn home_dir() -> Result<PathBuf> {
    Ok(BaseLocations::detect()?.home)
}

/// Get the global cache directory.
///
/// `$HX_CACHE_DIR`, else `$XDG_CACHE_HOME/hx`, else the platform default:
/// - Linux: `~/.cache/hx`
/// - macOS: `~/Library/Caches/io.raskell.hx`
/// - Windows: `%LOCALAPPDATA%\raskell\hx\cache`
pub fn global_cache_dir() -> Result<PathBuf> {
    Ok(current()?.cache.path)
}

/// Get the Cabal store directory.
///
/// `$HX_STORE_DIR`, else `<global cache>/cabal/store`.
pub fn cabal_store_dir() -> Result<PathBuf> {
    Ok(current()?.cabal_store.path)
}

/// Dependency-resolution cache (`<cache>/solver`; default `~/.cache/hx/solver`).
pub fn solver_cache_dir() -> Result<PathBuf> {
    Ok(current()?.solver_cache.path)
}

/// Package sources and native package store (`<cache>/packages`; default
/// `~/.cache/hx/packages`).
pub fn package_store_dir() -> Result<PathBuf> {
    Ok(current()?.package_store.path)
}

/// Mirrored Hackage index (`<cache>/index`; default `~/.cache/hx/index`).
pub fn index_mirror_dir() -> Result<PathBuf> {
    Ok(current()?.index_mirror.path)
}

/// Stackage snapshot cache (`<cache>/stackage`; default `<platform cache>/hx/stackage`).
pub fn stackage_cache_dir() -> Result<PathBuf> {
    Ok(current()?.stackage_cache.path)
}

/// BHC Platform cache (`<cache>/bhc-platform`; default `<platform cache>/hx/bhc-platform`).
pub fn bhc_platform_cache_dir() -> Result<PathBuf> {
    Ok(current()?.bhc_platform_cache.path)
}

/// `hx script` build cache (`<cache>/scripts`; default `<platform cache>/hx/scripts`).
pub fn script_cache_dir() -> Result<PathBuf> {
    Ok(current()?.script_cache.path)
}

/// Get the global config directory.
///
/// `$XDG_CONFIG_HOME/hx`, else the platform default:
/// - Linux: `~/.config/hx`
/// - macOS: `~/Library/Application Support/io.raskell.hx`
/// - Windows: `%APPDATA%\raskell\hx\config`
pub fn global_config_dir() -> Result<PathBuf> {
    Ok(current()?.config_dir.path)
}

/// Get the global config file path.
///
/// `--config-file`, else `$HX_CONFIG_FILE`, else `<config dir>/config.toml`.
pub fn global_config_file() -> Result<PathBuf> {
    Ok(current()?.config_file.path)
}

/// Global plugin directories in search order.
pub fn global_plugin_dirs() -> Result<Vec<PathBuf>> {
    Ok(current()?.plugin_dirs.into_iter().map(|d| d.path).collect())
}

/// hx home: `$HX_HOME`, else `~/.hx`.
pub fn hx_home_dir() -> Result<PathBuf> {
    Ok(current()?.hx_home.path)
}

/// Get the toolchain directory for hx-managed installations.
///
/// `<hx home>/toolchains`, i.e. `~/.hx/toolchains` unless `$HX_HOME` is set.
/// A home-relative path is used on every platform because GHC's build system
/// breaks on paths with spaces (macOS's data directory is
/// `~/Library/Application Support`).
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
    Ok(current()?.toolchains.path)
}

/// Get the bin directory for hx-managed tool symlinks (`<hx home>/bin`).
///
/// This directory can be added to PATH for direct access to hx-managed tools.
pub fn toolchain_bin_dir() -> Result<PathBuf> {
    Ok(current()?.toolchain_bin.path)
}

/// BHC installation root (`~/.bhc`).
pub fn bhc_home_dir() -> Result<PathBuf> {
    Ok(current()?.bhc_home.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// XDG roots used in tests. XDG values must be absolute *on the host*
    /// (relative ones are ignored), and `/x/...` is not absolute on Windows.
    const X_CACHE: &str = if cfg!(windows) {
        "C:/x/cache"
    } else {
        "/x/cache"
    };
    const X_CONFIG: &str = if cfg!(windows) {
        "C:/x/config"
    } else {
        "/x/config"
    };
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn no_env() -> impl Fn(&str) -> Option<OsString> {
        env(&[])
    }

    /// What `directories` 5/6 report on Linux with no XDG vars set.
    fn linux() -> BaseLocations {
        BaseLocations {
            home: "/home/u".into(),
            project_cache: "/home/u/.cache/hx".into(),
            project_config: "/home/u/.config/hx".into(),
            platform_cache: "/home/u/.cache".into(),
            platform_config: "/home/u/.config".into(),
        }
    }

    /// What `directories` 5/6 report on macOS.
    fn macos() -> BaseLocations {
        BaseLocations {
            home: "/Users/u".into(),
            project_cache: "/Users/u/Library/Caches/io.raskell.hx".into(),
            project_config: "/Users/u/Library/Application Support/io.raskell.hx".into(),
            platform_cache: "/Users/u/Library/Caches".into(),
            platform_config: "/Users/u/Library/Application Support".into(),
        }
    }

    /// What `directories` 5/6 report on Windows (forward slashes so the test
    /// runs on any host; only the structure matters).
    fn windows() -> BaseLocations {
        BaseLocations {
            home: "/C/Users/u".into(),
            project_cache: "/C/Users/u/AppData/Local/raskell/hx/cache".into(),
            project_config: "/C/Users/u/AppData/Roaming/raskell/hx/config".into(),
            platform_cache: "/C/Users/u/AppData/Local".into(),
            platform_config: "/C/Users/u/AppData/Roaming".into(),
        }
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn linux_defaults_are_unified_under_dot_cache() {
        let paths = HxPaths::resolve(no_env(), &linux());
        assert_eq!(paths.cache.path, p("/home/u/.cache/hx"));
        assert_eq!(paths.cabal_store.path, p("/home/u/.cache/hx/cabal/store"));
        assert_eq!(paths.solver_cache.path, p("/home/u/.cache/hx/solver"));
        assert_eq!(paths.package_store.path, p("/home/u/.cache/hx/packages"));
        assert_eq!(paths.index_mirror.path, p("/home/u/.cache/hx/index"));
        assert_eq!(paths.stackage_cache.path, p("/home/u/.cache/hx/stackage"));
        assert_eq!(
            paths.bhc_platform_cache.path,
            p("/home/u/.cache/hx/bhc-platform")
        );
        assert_eq!(paths.script_cache.path, p("/home/u/.cache/hx/scripts"));
        assert_eq!(paths.config_file.path, p("/home/u/.config/hx/config.toml"));
        assert_eq!(
            paths.plugin_dirs.len(),
            1,
            "legacy plugin dir is the same on Linux"
        );
        assert_eq!(paths.plugin_dirs[0].path, p("/home/u/.config/hx/plugins"));
        assert_eq!(paths.toolchains.path, p("/home/u/.hx/toolchains"));
        assert_eq!(paths.toolchain_bin.path, p("/home/u/.hx/bin"));
        assert_eq!(paths.bhc_home.path, p("/home/u/.bhc"));
        assert_eq!(paths.cache_roots(), vec![p("/home/u/.cache/hx")]);
        assert!(
            paths
                .entries()
                .iter()
                .all(|(_, r)| r.source == PathSource::Default)
        );
    }

    #[test]
    fn macos_defaults_match_previous_releases() {
        let paths = HxPaths::resolve(no_env(), &macos());
        assert_eq!(paths.cache.path, p("/Users/u/Library/Caches/io.raskell.hx"));
        assert_eq!(
            paths.cabal_store.path,
            p("/Users/u/Library/Caches/io.raskell.hx/cabal/store")
        );
        // hx-solver historically used ~/.cache/hx on every platform.
        assert_eq!(paths.solver_cache.path, p("/Users/u/.cache/hx/solver"));
        assert_eq!(paths.package_store.path, p("/Users/u/.cache/hx/packages"));
        assert_eq!(paths.index_mirror.path, p("/Users/u/.cache/hx/index"));
        // Stackage/BHC Platform/scripts historically used <platform cache>/hx.
        assert_eq!(
            paths.stackage_cache.path,
            p("/Users/u/Library/Caches/hx/stackage")
        );
        assert_eq!(
            paths.script_cache.path,
            p("/Users/u/Library/Caches/hx/scripts")
        );
        assert_eq!(
            paths.config_file.path,
            p("/Users/u/Library/Application Support/io.raskell.hx/config.toml")
        );
        assert_eq!(
            paths
                .plugin_dirs
                .iter()
                .map(|d| d.path.clone())
                .collect::<Vec<_>>(),
            vec![
                p("/Users/u/Library/Application Support/io.raskell.hx/plugins"),
                p("/Users/u/Library/Application Support/hx/plugins"),
            ]
        );
        assert_eq!(paths.toolchains.path, p("/Users/u/.hx/toolchains"));
        assert_eq!(
            paths.cache_roots(),
            vec![
                p("/Users/u/Library/Caches/io.raskell.hx"),
                p("/Users/u/.cache/hx/solver"),
                p("/Users/u/.cache/hx/packages"),
                p("/Users/u/.cache/hx/index"),
                p("/Users/u/Library/Caches/hx/stackage"),
                p("/Users/u/Library/Caches/hx/bhc-platform"),
                p("/Users/u/Library/Caches/hx/scripts"),
            ]
        );
    }

    #[test]
    fn windows_defaults_match_previous_releases() {
        let paths = HxPaths::resolve(no_env(), &windows());
        assert_eq!(
            paths.cache.path,
            p("/C/Users/u/AppData/Local/raskell/hx/cache")
        );
        assert_eq!(paths.solver_cache.path, p("/C/Users/u/.cache/hx/solver"));
        assert_eq!(
            paths.stackage_cache.path,
            p("/C/Users/u/AppData/Local/hx/stackage")
        );
        assert_eq!(
            paths.config_file.path,
            p("/C/Users/u/AppData/Roaming/raskell/hx/config/config.toml")
        );
        assert_eq!(
            paths.plugin_dirs[1].path,
            p("/C/Users/u/AppData/Roaming/hx/plugins")
        );
        assert_eq!(paths.toolchains.path, p("/C/Users/u/.hx/toolchains"));
    }

    #[test]
    fn xdg_cache_home_moves_every_cache_location() {
        for base in [linux(), macos(), windows()] {
            let paths = HxPaths::resolve(env(&[("XDG_CACHE_HOME", X_CACHE)]), &base);
            let src = PathSource::Env("XDG_CACHE_HOME");
            for (loc, expected) in [
                (&paths.cache, &format!("{X_CACHE}/hx")),
                (&paths.cabal_store, &format!("{X_CACHE}/hx/cabal/store")),
                (&paths.solver_cache, &format!("{X_CACHE}/hx/solver")),
                (&paths.package_store, &format!("{X_CACHE}/hx/packages")),
                (&paths.index_mirror, &format!("{X_CACHE}/hx/index")),
                (&paths.stackage_cache, &format!("{X_CACHE}/hx/stackage")),
                (
                    &paths.bhc_platform_cache,
                    &format!("{X_CACHE}/hx/bhc-platform"),
                ),
                (&paths.script_cache, &format!("{X_CACHE}/hx/scripts")),
            ] {
                assert_eq!(loc.path, p(expected));
                assert_eq!(loc.source, src);
            }
            assert_eq!(paths.cache_roots(), vec![p(&format!("{X_CACHE}/hx"))]);
            // Config and toolchains are untouched.
            assert_eq!(paths.config_dir.source, PathSource::Default);
            assert_eq!(paths.toolchains.path, base.home.join(".hx/toolchains"));
        }
    }

    #[test]
    fn hx_cache_dir_wins_over_xdg_and_is_used_as_is() {
        let paths = HxPaths::resolve(
            env(&[("HX_CACHE_DIR", "/hxc"), ("XDG_CACHE_HOME", X_CACHE)]),
            &macos(),
        );
        assert_eq!(paths.cache.path, p("/hxc"));
        assert_eq!(
            paths.cache.path.as_os_str(),
            "/hxc",
            "no trailing separator"
        );
        assert_eq!(paths.solver_cache.path, p("/hxc/solver"));
        assert_eq!(paths.cabal_store.path, p("/hxc/cabal/store"));
        assert_eq!(paths.cache.source, PathSource::Env("HX_CACHE_DIR"));
    }

    #[test]
    fn hx_store_dir_moves_only_the_cabal_store() {
        let paths = HxPaths::resolve(env(&[("HX_STORE_DIR", "/store")]), &linux());
        assert_eq!(paths.cabal_store.path, p("/store"));
        assert_eq!(paths.cache.path, p("/home/u/.cache/hx"));
    }

    #[test]
    fn xdg_config_home_moves_config_file_and_plugins() {
        let paths = HxPaths::resolve(env(&[("XDG_CONFIG_HOME", X_CONFIG)]), &macos());
        assert_eq!(paths.config_dir.path, p(&format!("{X_CONFIG}/hx")));
        assert_eq!(
            paths.config_file.path,
            p(&format!("{X_CONFIG}/hx/config.toml"))
        );
        assert_eq!(
            paths.plugin_dirs[0].path,
            p(&format!("{X_CONFIG}/hx/plugins"))
        );
        assert_eq!(paths.config_file.source, PathSource::Env("XDG_CONFIG_HOME"));
        // XDG_CONFIG_HOME must not leak into the cache.
        assert_eq!(paths.cache.path, p("/Users/u/Library/Caches/io.raskell.hx"));
    }

    #[test]
    fn xdg_cache_home_does_not_leak_into_config() {
        let paths = HxPaths::resolve(env(&[("XDG_CACHE_HOME", X_CACHE)]), &linux());
        assert_eq!(paths.config_dir.path, p("/home/u/.config/hx"));
        assert_ne!(paths.config_dir.path, paths.cache.path);
    }

    #[test]
    fn hx_config_file_overrides_only_the_file() {
        let paths = HxPaths::resolve(
            env(&[
                ("HX_CONFIG_FILE", "/etc/hx.toml"),
                ("XDG_CONFIG_HOME", X_CONFIG),
            ]),
            &linux(),
        );
        assert_eq!(paths.config_file.path, p("/etc/hx.toml"));
        assert_eq!(paths.config_dir.path, p(&format!("{X_CONFIG}/hx")));
    }

    #[test]
    fn hx_home_moves_toolchains_and_bin() {
        let paths = HxPaths::resolve(env(&[("HX_HOME", "/opt/hx")]), &linux());
        assert_eq!(paths.toolchains.path, p("/opt/hx/toolchains"));
        assert_eq!(paths.toolchain_bin.path, p("/opt/hx/bin"));
        assert_eq!(paths.toolchains.source, PathSource::Env("HX_HOME"));
    }

    #[test]
    fn xdg_data_home_does_not_move_toolchains() {
        let paths = HxPaths::resolve(env(&[("XDG_DATA_HOME", "/x/data")]), &linux());
        assert_eq!(paths.toolchains.path, p("/home/u/.hx/toolchains"));
    }

    #[test]
    fn empty_and_relative_xdg_values_are_ignored() {
        for value in ["", "relative/cache"] {
            let paths = HxPaths::resolve(
                env(&[("XDG_CACHE_HOME", value), ("XDG_CONFIG_HOME", value)]),
                &macos(),
            );
            assert_eq!(paths.cache.path, p("/Users/u/Library/Caches/io.raskell.hx"));
            assert_eq!(paths.solver_cache.path, p("/Users/u/.cache/hx/solver"));
            assert_eq!(paths.config_dir.source, PathSource::Default);
        }
    }

    #[test]
    fn empty_hx_values_are_ignored() {
        let paths = HxPaths::resolve(
            env(&[
                ("HX_CACHE_DIR", ""),
                ("HX_HOME", ""),
                ("HX_CONFIG_FILE", ""),
            ]),
            &linux(),
        );
        assert_eq!(paths, HxPaths::resolve(no_env(), &linux()));
    }

    #[test]
    fn detected_base_locations_match_directories_crate() {
        // Guards against a `directories` upgrade silently moving defaults:
        // the shape of what it reports must stay what the tests above assume.
        let Ok(base) = BaseLocations::detect() else {
            return;
        };
        assert!(base.project_cache.starts_with(&base.home) || cfg!(windows));
        if cfg!(target_os = "macos") {
            assert_eq!(
                base.project_cache,
                base.home.join("Library/Caches/io.raskell.hx")
            );
            assert_eq!(
                base.project_config,
                base.home.join("Library/Application Support/io.raskell.hx")
            );
            assert_eq!(base.platform_cache, base.home.join("Library/Caches"));
            assert_eq!(
                base.platform_config,
                base.home.join("Library/Application Support")
            );
        }
        if cfg!(windows) {
            assert!(base.project_cache.ends_with("raskell\\hx\\cache"));
            assert!(base.project_config.ends_with("raskell\\hx\\config"));
        }
        if cfg!(target_os = "linux") {
            assert_eq!(base.project_cache, base.platform_cache.join("hx"));
            assert_eq!(base.project_config, base.platform_config.join("hx"));
        }
    }
}
