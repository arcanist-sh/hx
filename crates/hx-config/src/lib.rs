//! Configuration parsing and project detection for hx.
//!
//! This crate handles:
//! - Parsing `hx.toml` project manifests
//! - Global configuration (`~/.config/hx/config.toml`)
//! - Detecting project roots
//! - Locating `.cabal` files
//! - Merging configuration defaults
//! - IDE/HLS configuration (hie.yaml generation)

pub mod combine;
pub mod global;
pub mod ide;
pub mod manifest;
pub mod project;

pub use combine::Combine;
pub use global::{
    GlobalConfig, GlobalConfigError, global_config_path, is_project_trusted, load_global_config,
    save_global_config, trust_project, untrust_project,
};
pub use ide::{
    HieYamlStatus, check_hie_yaml, generate_hie_yaml, is_hls_compatible, recommended_hls_for_ghc,
    write_hie_yaml,
};
pub use manifest::{
    BhcConfig, BhcPlatformConfig, BhcProfile, BuildConfig, CompilerBackend, CompilerConfig,
    FormatConfig, GhcConfig, LintConfig, Manifest, PluginConfig, PluginHookConfig, ProjectConfig,
    ProjectKind, StackageConfig, ToolchainConfig,
};
pub use project::{Project, WorkspacePackage, find_project_root};

/// The manifest filename.
pub const MANIFEST_FILENAME: &str = "hx.toml";

/// The lockfile filename.
pub const LOCKFILE_FILENAME: &str = "hx.lock";

/// The local cache directory name.
pub const CACHE_DIR_NAME: &str = ".hx";
