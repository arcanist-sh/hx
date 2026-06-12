//! hx.toml manifest parsing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Error type for manifest operations.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("failed to read manifest: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("failed to parse manifest: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("failed to serialize manifest: {0}")]
    SerializeError(#[from] toml::ser::Error),
}

/// The project kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    /// A binary/executable project
    #[default]
    Bin,
    /// A library project
    Lib,
}

impl ProjectKind {
    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProjectKind::Bin => "bin",
            ProjectKind::Lib => "lib",
        }
    }
}

/// Project configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Project name
    pub name: String,
    /// Project kind (bin or lib)
    #[serde(default)]
    pub kind: ProjectKind,
    /// Resolver to use (currently only "cabal")
    #[serde(default = "default_resolver")]
    pub resolver: String,
}

fn default_resolver() -> String {
    "cabal".to_string()
}

/// Toolchain configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolchainConfig {
    /// GHC version
    pub ghc: Option<String>,
    /// Cabal version
    pub cabal: Option<String>,
    /// HLS version
    pub hls: Option<String>,
}

/// The compiler backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompilerBackend {
    /// GHC (Glasgow Haskell Compiler) - the standard Haskell compiler.
    #[default]
    Ghc,
    /// BHC (Basel Haskell Compiler) - alternative compiler with tensor optimizations.
    Bhc,
}

impl CompilerBackend {
    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            CompilerBackend::Ghc => "ghc",
            CompilerBackend::Bhc => "bhc",
        }
    }
}

impl std::fmt::Display for CompilerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for CompilerBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ghc" => Ok(CompilerBackend::Ghc),
            "bhc" => Ok(CompilerBackend::Bhc),
            _ => Err(format!("unknown compiler backend: {}", s)),
        }
    }
}

/// BHC profile for optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BhcProfile {
    /// Default profile - balanced optimizations.
    #[default]
    Default,
    /// Server profile - optimized for server workloads.
    Server,
    /// Numeric profile - optimized for numeric/scientific computing.
    Numeric,
    /// Edge profile - optimized for edge/embedded deployments.
    Edge,
}

impl BhcProfile {
    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            BhcProfile::Default => "default",
            BhcProfile::Server => "server",
            BhcProfile::Numeric => "numeric",
            BhcProfile::Edge => "edge",
        }
    }
}

/// BHC-specific configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BhcConfig {
    /// Optimization profile.
    #[serde(default)]
    pub profile: BhcProfile,
    /// Emit kernel performance report.
    #[serde(default)]
    pub emit_kernel_report: bool,
    /// Cross-compilation target.
    pub target: Option<String>,
    /// Enable tensor fusion optimizations.
    #[serde(default)]
    pub tensor_fusion: bool,
}

/// GHC-specific configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GhcConfig {
    /// GHC version to use (moved from `[toolchain]` section).
    pub version: Option<String>,
    /// Enable profiling.
    #[serde(default)]
    pub profiling: bool,
    /// Enable split sections for smaller binaries.
    #[serde(default)]
    pub split_sections: bool,
}

/// Compiler configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilerConfig {
    /// Which compiler backend to use.
    #[serde(default)]
    pub backend: CompilerBackend,
    /// Compiler version (applies to selected backend).
    pub version: Option<String>,
    /// BHC-specific configuration.
    #[serde(default)]
    pub bhc: BhcConfig,
    /// GHC-specific configuration.
    #[serde(default)]
    pub ghc: GhcConfig,
}

impl Default for CompilerConfig {
    fn default() -> Self {
        Self {
            backend: CompilerBackend::Ghc,
            version: None,
            bhc: BhcConfig::default(),
            ghc: GhcConfig::default(),
        }
    }
}

/// Stackage snapshot configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StackageConfig {
    /// Stackage snapshot to use (e.g., "lts-22.28", "nightly-2024-01-15")
    pub snapshot: Option<String>,
    /// Whether to allow packages not in the snapshot (from Hackage)
    #[serde(default)]
    pub allow_newer: bool,
    /// Extra packages to add with specific versions (overrides snapshot)
    #[serde(default)]
    pub extra_deps: HashMap<String, String>,
}

/// BHC Platform curated snapshot configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BhcPlatformConfig {
    /// BHC Platform snapshot to use (e.g., "bhc-platform-2026.1")
    pub snapshot: Option<String>,
    /// Whether to allow packages not in the platform set
    #[serde(default)]
    pub allow_newer: bool,
    /// Extra packages to add with specific versions (overrides platform)
    #[serde(default)]
    pub extra_deps: HashMap<String, String>,
    /// Paths to overlay TOML files (relative to project root)
    #[serde(default)]
    pub overlays: Vec<String>,
}

/// Format configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatConfig {
    /// Formatter to use (fourmolu or ormolu)
    #[serde(default = "default_formatter")]
    pub formatter: String,
}

fn default_formatter() -> String {
    "fourmolu".to_string()
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            formatter: default_formatter(),
        }
    }
}

/// Lint configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LintConfig {
    /// Whether to enable hlint
    #[serde(default = "default_true")]
    pub hlint: bool,
}

fn default_true() -> bool {
    true
}

/// Plugin hook configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginHookConfig {
    /// Scripts to run before build.
    pub pre_build: Vec<String>,

    /// Scripts to run after build.
    pub post_build: Vec<String>,

    /// Scripts to run before tests.
    pub pre_test: Vec<String>,

    /// Scripts to run after tests.
    pub post_test: Vec<String>,

    /// Scripts to run before run command.
    pub pre_run: Vec<String>,

    /// Scripts to run after run completes.
    pub post_run: Vec<String>,

    /// Scripts to run before clean.
    pub pre_clean: Vec<String>,

    /// Scripts to run after clean.
    pub post_clean: Vec<String>,

    /// Scripts to run before lock generation.
    pub pre_lock: Vec<String>,

    /// Scripts to run after lock completes.
    pub post_lock: Vec<String>,

    /// Scripts to run on project initialization.
    pub init: Vec<String>,
}

/// Plugin configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Whether plugins are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Timeout for hook execution in milliseconds.
    #[serde(default = "default_hook_timeout")]
    pub hook_timeout_ms: u64,

    /// Additional paths to search for plugins.
    #[serde(default)]
    pub paths: Vec<String>,

    /// Whether to continue on hook failure.
    #[serde(default)]
    pub continue_on_error: bool,

    /// Hook configuration.
    #[serde(default)]
    pub hooks: PluginHookConfig,
}

fn default_hook_timeout() -> u64 {
    5000
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hook_timeout_ms: default_hook_timeout(),
            paths: Vec::new(),
            continue_on_error: false,
            hooks: PluginHookConfig::default(),
        }
    }
}

/// Build configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// GHC optimization level (0, 1, or 2)
    #[serde(default = "default_optimization")]
    pub optimization: u8,

    /// Enable all warnings (-Wall)
    #[serde(default = "default_true")]
    pub warnings: bool,

    /// Treat warnings as errors (-Werror)
    #[serde(default)]
    pub werror: bool,

    /// Additional GHC flags
    #[serde(default)]
    pub ghc_flags: Vec<String>,

    /// Source directories (defaults to ["src"])
    #[serde(default = "default_src_dirs")]
    pub src_dirs: Vec<String>,

    /// Use native build (experimental)
    #[serde(default)]
    pub native: bool,
}

fn default_optimization() -> u8 {
    1
}

fn default_src_dirs() -> Vec<String> {
    vec!["src".to_string()]
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            optimization: default_optimization(),
            warnings: true,
            werror: false,
            ghc_flags: Vec::new(),
            src_dirs: default_src_dirs(),
            native: false,
        }
    }
}

/// The hx.toml manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Project configuration
    pub project: ProjectConfig,

    /// Toolchain configuration
    #[serde(default)]
    pub toolchain: ToolchainConfig,

    /// Compiler backend configuration
    #[serde(default)]
    pub compiler: CompilerConfig,

    /// Stackage snapshot configuration
    #[serde(default)]
    pub stackage: StackageConfig,

    /// BHC Platform curated snapshot configuration
    #[serde(default, rename = "bhc-platform")]
    pub bhc_platform: BhcPlatformConfig,

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

    /// Dependencies (optional, can rely on .cabal)
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

impl Manifest {
    /// Parse a manifest from a TOML string.
    pub fn parse(s: &str) -> Result<Self, ManifestError> {
        Ok(toml::from_str(s)?)
    }

    /// Parse a manifest from a file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Serialize the manifest to a TOML string.
    pub fn to_string(&self) -> Result<String, ManifestError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Write the manifest to a file (atomically, to survive interruption).
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), ManifestError> {
        let content = self.to_string()?;
        hx_core::atomic_write(path.as_ref(), content)?;
        Ok(())
    }

    /// Create a new manifest for a project.
    pub fn new(name: impl Into<String>, kind: ProjectKind) -> Self {
        Self {
            project: ProjectConfig {
                name: name.into(),
                kind,
                resolver: default_resolver(),
            },
            toolchain: ToolchainConfig::default(),
            compiler: CompilerConfig::default(),
            stackage: StackageConfig::default(),
            bhc_platform: BhcPlatformConfig::default(),
            build: BuildConfig::default(),
            format: FormatConfig::default(),
            lint: LintConfig::default(),
            plugins: PluginConfig::default(),
            dependencies: HashMap::new(),
        }
    }

    /// Apply global config defaults to this manifest.
    ///
    /// Project-local settings take precedence over global settings.
    /// This uses the [`Combine`](crate::Combine) trait where `self` (project)
    /// has higher precedence than `global` config.
    pub fn with_global_defaults(self, global: &crate::GlobalConfig) -> Self {
        use crate::Combine;

        Self {
            // Project config is always from project manifest
            project: self.project,
            // Merge toolchain (project takes precedence)
            toolchain: self.toolchain.combine(global.toolchain.clone()),
            // Compiler config is project-specific (no global merging)
            compiler: self.compiler,
            // Stackage config is project-specific only
            stackage: self.stackage,
            // BHC Platform config is project-specific only
            bhc_platform: self.bhc_platform,
            // Merge build (project takes precedence)
            build: self.build.combine(global.build.clone()),
            // Merge format (project takes precedence)
            format: self.format.combine(global.format.clone()),
            // Merge lint (project takes precedence)
            lint: self.lint.combine(global.lint.clone()),
            // Merge plugins (project takes precedence)
            plugins: self.plugins.combine(global.plugins.clone()),
            // Dependencies are project-specific only
            dependencies: self.dependencies,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal() {
        let toml = r#"
[project]
name = "myapp"
"#;
        let manifest = Manifest::parse(toml).unwrap();
        assert_eq!(manifest.project.name, "myapp");
        assert_eq!(manifest.project.kind, ProjectKind::Bin);
    }

    #[test]
    fn test_parse_full() {
        let toml = r#"
[project]
name = "myapp"
kind = "bin"
resolver = "cabal"

[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"
hls = "2.9.0.0"

[format]
formatter = "fourmolu"

[lint]
hlint = true
"#;
        let manifest = Manifest::parse(toml).unwrap();
        assert_eq!(manifest.project.name, "myapp");
        assert_eq!(manifest.toolchain.ghc, Some("9.8.2".to_string()));
        assert_eq!(manifest.format.formatter, "fourmolu");
        assert!(manifest.lint.hlint);
    }

    #[test]
    fn test_roundtrip() {
        let manifest = Manifest::new("test", ProjectKind::Lib);
        let toml = manifest.to_string().unwrap();
        let parsed = Manifest::parse(&toml).unwrap();
        assert_eq!(parsed.project.name, "test");
        assert_eq!(parsed.project.kind, ProjectKind::Lib);
    }

    #[test]
    fn test_parse_compiler_ghc() {
        let toml = r#"
[project]
name = "myapp"

[compiler]
backend = "ghc"
version = "9.8.2"

[compiler.ghc]
profiling = true
"#;
        let manifest = Manifest::parse(toml).unwrap();
        assert_eq!(manifest.compiler.backend, CompilerBackend::Ghc);
        assert_eq!(manifest.compiler.version, Some("9.8.2".to_string()));
        assert!(manifest.compiler.ghc.profiling);
    }

    #[test]
    fn test_parse_compiler_bhc() {
        let toml = r#"
[project]
name = "myapp"

[compiler]
backend = "bhc"
version = "0.1.0"

[compiler.bhc]
profile = "numeric"
emit_kernel_report = true
tensor_fusion = true
"#;
        let manifest = Manifest::parse(toml).unwrap();
        assert_eq!(manifest.compiler.backend, CompilerBackend::Bhc);
        assert_eq!(manifest.compiler.version, Some("0.1.0".to_string()));
        assert_eq!(manifest.compiler.bhc.profile, BhcProfile::Numeric);
        assert!(manifest.compiler.bhc.emit_kernel_report);
        assert!(manifest.compiler.bhc.tensor_fusion);
    }

    #[test]
    fn test_compiler_backend_default() {
        let toml = r#"
[project]
name = "myapp"
"#;
        let manifest = Manifest::parse(toml).unwrap();
        // Default should be GHC
        assert_eq!(manifest.compiler.backend, CompilerBackend::Ghc);
    }

    #[test]
    fn test_compiler_backend_from_str() {
        assert_eq!(
            "ghc".parse::<CompilerBackend>().unwrap(),
            CompilerBackend::Ghc
        );
        assert_eq!(
            "GHC".parse::<CompilerBackend>().unwrap(),
            CompilerBackend::Ghc
        );
        assert_eq!(
            "bhc".parse::<CompilerBackend>().unwrap(),
            CompilerBackend::Bhc
        );
        assert_eq!(
            "BHC".parse::<CompilerBackend>().unwrap(),
            CompilerBackend::Bhc
        );
        assert!("unknown".parse::<CompilerBackend>().is_err());
    }

    #[test]
    fn test_bhc_profile_parsing() {
        let toml = r#"
[project]
name = "myapp"

[compiler]
backend = "bhc"

[compiler.bhc]
profile = "server"
"#;
        let manifest = Manifest::parse(toml).unwrap();
        assert_eq!(manifest.compiler.bhc.profile, BhcProfile::Server);

        let toml_edge = r#"
[project]
name = "myapp"

[compiler.bhc]
profile = "edge"
"#;
        let manifest_edge = Manifest::parse(toml_edge).unwrap();
        assert_eq!(manifest_edge.compiler.bhc.profile, BhcProfile::Edge);
    }
}
