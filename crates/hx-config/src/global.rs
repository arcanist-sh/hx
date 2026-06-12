//! Global configuration.
//!
//! Global config is stored at `~/.config/hx/config.toml` (or platform equivalent)
//! and provides defaults that can be overridden by project-local `hx.toml`.

use crate::{BuildConfig, FormatConfig, LintConfig, PluginConfig, ToolchainConfig};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
use tracing::debug;

/// Error type for global config operations.
#[derive(Debug, Error)]
pub enum GlobalConfigError {
    #[error("failed to read global config: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("failed to parse global config: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("failed to serialize global config: {0}")]
    SerializeError(#[from] toml::ser::Error),
}

/// Global configuration.
///
/// This is similar to [`Manifest`](crate::Manifest) but without project-specific
/// fields like `project.name`. Settings here provide defaults that can be
/// overridden by project-local `hx.toml`.
///
/// # Example
///
/// ```toml
/// # ~/.config/hx/config.toml
///
/// [toolchain]
/// ghc = "9.8.2"
/// cabal = "3.12.1.0"
///
/// [build]
/// optimization = 2
/// warnings = true
///
/// [format]
/// formatter = "fourmolu"
///
/// [lint]
/// hlint = true
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Projects allowed to run their local `.hx/plugins` scripts.
    ///
    /// Local plugins execute arbitrary code, so trust must be granted by the
    /// user (via `hx plugins trust`), never by the project itself.
    /// Note: must stay the first field so TOML serializes it before tables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_projects: Vec<std::path::PathBuf>,

    /// Toolchain configuration (GHC, Cabal, HLS versions)
    #[serde(default)]
    pub toolchain: ToolchainConfig,

    /// Build configuration
    #[serde(default)]
    pub build: BuildConfig,

    /// Format configuration
    #[serde(default)]
    pub format: FormatConfig,

    /// Lint configuration
    #[serde(default)]
    pub lint: LintConfig,

    /// Plugin configuration
    #[serde(default)]
    pub plugins: PluginConfig,
}

impl GlobalConfig {
    /// Parse global config from a TOML string.
    pub fn parse(s: &str) -> Result<Self, GlobalConfigError> {
        Ok(toml::from_str(s)?)
    }

    /// Parse global config from a file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, GlobalConfigError> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Serialize the global config to a TOML string.
    pub fn to_string(&self) -> Result<String, GlobalConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Write the global config to a file.
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), GlobalConfigError> {
        let path = path.as_ref();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = self.to_string()?;
        hx_core::atomic_write(path, content)?;
        Ok(())
    }
}

/// Load the global configuration.
///
/// Returns `Ok(None)` if the global config file doesn't exist.
/// Returns `Err` if the file exists but can't be parsed.
pub fn load_global_config() -> Result<Option<GlobalConfig>, GlobalConfigError> {
    let config_file = match hx_cache::global_config_file() {
        Ok(path) => path,
        Err(_) => {
            debug!("Could not determine global config path");
            return Ok(None);
        }
    };

    if !config_file.exists() {
        debug!(
            "Global config file does not exist: {}",
            config_file.display()
        );
        return Ok(None);
    }

    debug!("Loading global config from: {}", config_file.display());
    let config = GlobalConfig::from_file(&config_file)?;
    Ok(Some(config))
}

/// Get the path to the global config file.
///
/// Returns `None` if the home directory cannot be determined.
pub fn global_config_path() -> Option<std::path::PathBuf> {
    hx_cache::global_config_file().ok()
}

/// Save the global configuration.
pub fn save_global_config(config: &GlobalConfig) -> Result<(), GlobalConfigError> {
    let config_file = hx_cache::global_config_file().map_err(|e| {
        GlobalConfigError::ReadError(std::io::Error::other(format!(
            "could not determine global config path: {e}"
        )))
    })?;
    config.to_file(config_file)
}

/// Canonical form of a project root used for trust comparisons.
fn canonical_project_root(root: &Path) -> std::path::PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// Check whether a project is trusted to run its local plugins.
pub fn is_project_trusted(project_root: &Path) -> bool {
    let root = canonical_project_root(project_root);
    match load_global_config() {
        Ok(Some(config)) => config
            .trusted_projects
            .iter()
            .any(|p| canonical_project_root(p) == root),
        _ => false,
    }
}

/// Add a project to the trusted list. Returns false if it was already trusted.
pub fn trust_project(project_root: &Path) -> Result<bool, GlobalConfigError> {
    let root = canonical_project_root(project_root);
    let mut config = load_global_config()?.unwrap_or_default();
    if config
        .trusted_projects
        .iter()
        .any(|p| canonical_project_root(p) == root)
    {
        return Ok(false);
    }
    config.trusted_projects.push(root);
    save_global_config(&config)?;
    Ok(true)
}

/// Remove a project from the trusted list. Returns false if it wasn't trusted.
pub fn untrust_project(project_root: &Path) -> Result<bool, GlobalConfigError> {
    let root = canonical_project_root(project_root);
    let mut config = load_global_config()?.unwrap_or_default();
    let before = config.trusted_projects.len();
    config
        .trusted_projects
        .retain(|p| canonical_project_root(p) != root);
    if config.trusted_projects.len() == before {
        return Ok(false);
    }
    save_global_config(&config)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_parse_empty() {
        let config = GlobalConfig::parse("").unwrap();
        assert!(config.toolchain.ghc.is_none());
        assert!(config.toolchain.cabal.is_none());
    }

    #[test]
    fn test_parse_toolchain() {
        let toml = r#"
[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"
"#;
        let config = GlobalConfig::parse(toml).unwrap();
        assert_eq!(config.toolchain.ghc, Some("9.8.2".to_string()));
        assert_eq!(config.toolchain.cabal, Some("3.12.1.0".to_string()));
    }

    #[test]
    fn test_parse_build() {
        let toml = r#"
[build]
optimization = 2
warnings = true
werror = true
"#;
        let config = GlobalConfig::parse(toml).unwrap();
        assert_eq!(config.build.optimization, 2);
        assert!(config.build.warnings);
        assert!(config.build.werror);
    }

    #[test]
    fn test_parse_format_lint() {
        let toml = r#"
[format]
formatter = "ormolu"

[lint]
hlint = false
"#;
        let config = GlobalConfig::parse(toml).unwrap();
        assert_eq!(config.format.formatter, "ormolu");
        assert!(!config.lint.hlint);
    }

    #[test]
    fn test_roundtrip() {
        let original = GlobalConfig {
            trusted_projects: vec![],
            toolchain: ToolchainConfig {
                ghc: Some("9.8.2".to_string()),
                cabal: Some("3.12.1.0".to_string()),
                hls: None,
            },
            build: BuildConfig::default(),
            format: FormatConfig::default(),
            lint: LintConfig::default(),
            plugins: PluginConfig::default(),
        };

        let toml = original.to_string().unwrap();
        let parsed = GlobalConfig::parse(&toml).unwrap();

        assert_eq!(parsed.toolchain.ghc, original.toolchain.ghc);
        assert_eq!(parsed.toolchain.cabal, original.toolchain.cabal);
    }

    #[test]
    fn test_write_to_file() {
        let dir = tempdir().unwrap();
        let config_file = dir.path().join("subdir").join("config.toml");

        let config = GlobalConfig {
            toolchain: ToolchainConfig {
                ghc: Some("9.8.2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        config.to_file(&config_file).unwrap();
        assert!(config_file.exists());

        let loaded = GlobalConfig::from_file(&config_file).unwrap();
        assert_eq!(loaded.toolchain.ghc, Some("9.8.2".to_string()));
    }
}
