//! BHC native compiler configuration and build options.
//!
//! This module provides the low-level compiler configuration for invoking BHC
//! directly, including version detection, package database discovery, and
//! flag generation.

use directories::BaseDirs;
use hx_config::BhcProfile;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, info};

/// Error types for BHC native operations.
#[derive(Debug, thiserror::Error)]
pub enum BhcNativeError {
    /// BHC executable was not found in PATH.
    #[error("BHC not found: {0}")]
    BhcNotFound(String),

    /// Failed to detect BHC version from output.
    #[error("BHC version detection failed: {0}")]
    VersionDetectionFailed(String),

    /// Package database operation failed.
    #[error("package database error: {0}")]
    PackageDbError(String),

    /// Compilation step failed.
    #[error("compilation failed: {0}")]
    CompilationFailed(String),

    /// Linking step failed.
    #[error("linking failed: {0}")]
    LinkingFailed(String),

    /// Underlying IO error with context.
    #[error("IO error: {message}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
    },

    /// External command returned a non-zero exit code.
    #[error("command failed: {command}")]
    CommandFailed {
        command: String,
        stderr: String,
        exit_code: Option<i32>,
    },
}

/// BHC compiler configuration for native builds.
///
/// Holds all the information needed to invoke BHC for compilation,
/// including paths, profile, package databases, and feature flags.
#[derive(Clone)]
pub struct BhcCompilerConfig {
    /// Path to the BHC executable.
    pub bhc_path: PathBuf,
    /// Detected BHC version string (e.g. "2026.2.0").
    pub version: String,
    /// Build profile controlling optimization strategy.
    pub profile: BhcProfile,
    /// Paths to package databases to pass to BHC.
    pub package_dbs: Vec<PathBuf>,
    /// Explicit packages to expose during compilation.
    pub packages: Vec<String>,
    /// Enable tensor fusion optimization pass.
    pub tensor_fusion: bool,
    /// Emit a kernel report after compilation.
    pub emit_kernel_report: bool,
}

impl BhcCompilerConfig {
    /// Detect BHC on the system PATH.
    ///
    /// Runs `bhc --numeric-version` to obtain the version string, then
    /// discovers default package databases.
    pub async fn detect() -> Result<Self, BhcNativeError> {
        Self::detect_with_path(Path::new("bhc")).await
    }

    /// Detect BHC using an explicit executable path.
    ///
    /// Runs `<bhc_path> --numeric-version` to obtain the version string,
    /// then discovers default package databases.
    pub async fn detect_with_path(bhc_path: &Path) -> Result<Self, BhcNativeError> {
        info!("Detecting BHC at {}", bhc_path.display());

        let output = Command::new(bhc_path)
            .arg("--numeric-version")
            .output()
            .await
            .map_err(|e| BhcNativeError::BhcNotFound(format!("{}: {}", bhc_path.display(), e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BhcNativeError::VersionDetectionFailed(format!(
                "bhc --numeric-version exited with {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if version.is_empty() {
            return Err(BhcNativeError::VersionDetectionFailed(
                "bhc --numeric-version returned empty output".to_string(),
            ));
        }

        debug!("Detected BHC version {}", version);

        let package_dbs = Self::detect_package_dbs(&version);

        Ok(Self {
            bhc_path: bhc_path.to_path_buf(),
            version,
            profile: BhcProfile::default(),
            package_dbs,
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        })
    }

    /// Discover default package database paths for a given BHC version.
    ///
    /// Looks in:
    /// - `~/.cache/hx/bhc-<version>/package.db`
    /// - `~/.bhc/package.db`
    fn detect_package_dbs(version: &str) -> Vec<PathBuf> {
        let mut dbs = Vec::new();

        if let Some(base_dirs) = BaseDirs::new() {
            let home = base_dirs.home_dir();

            // Per-version cache DB under the platform cache directory
            let cache_dir = base_dirs.cache_dir();
            let cache_db = cache_dir
                .join("hx")
                .join(format!("bhc-{}", version))
                .join("package.db");
            dbs.push(cache_db);

            // Global BHC DB
            let global_db = home.join(".bhc").join("package.db");
            dbs.push(global_db);
        }

        debug!("Detected package DBs: {:?}", dbs);
        dbs
    }

    /// Set the packages to expose during compilation.
    pub fn with_packages(mut self, packages: Vec<String>) -> Self {
        self.packages = packages;
        self
    }

    /// Set the build profile.
    pub fn with_profile(mut self, profile: BhcProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Enable or disable tensor fusion optimization.
    pub fn with_tensor_fusion(mut self, enabled: bool) -> Self {
        self.tensor_fusion = enabled;
        self
    }

    /// Enable or disable kernel report emission.
    pub fn with_emit_kernel_report(mut self, enabled: bool) -> Self {
        self.emit_kernel_report = enabled;
        self
    }

    /// Generate the common BHC flags from this configuration.
    ///
    /// Includes profile, tensor-fusion, kernel-report, package-db, and
    /// package flags as appropriate.
    pub fn bhc_flags(&self) -> Vec<String> {
        let mut flags = Vec::new();

        // Profile flag (global option; must precede any subcommand/files).
        flags.push(format!("--profile={}", self.profile.as_str()));

        // Kernel report. BHC 0.2.3 spells this `--kernel-report` (there is no
        // `--emit-kernel-report`, and no `--tensor-fusion` flag at all — tensor
        // optimisation is implied by `--profile numeric`).
        if self.emit_kernel_report {
            flags.push("--kernel-report".to_string());
        }

        // Package databases
        for db in &self.package_dbs {
            flags.push("--package-db".to_string());
            flags.push(db.to_string_lossy().to_string());
        }

        // Explicit packages
        for pkg in &self.packages {
            flags.push("--package".to_string());
            flags.push(pkg.clone());
        }

        flags
    }
}

/// Build options for a native BHC compilation.
#[derive(Clone)]
pub struct BhcNativeBuildOptions {
    /// Source directories to compile.
    pub src_dirs: Vec<PathBuf>,
    /// Directory for build artifacts.
    pub output_dir: PathBuf,
    /// Optimization level (0-3).
    pub optimization: u8,
    /// Enable warnings.
    pub warnings: bool,
    /// Treat warnings as errors.
    pub werror: bool,
    /// Extra flags to pass to BHC.
    pub extra_flags: Vec<String>,
    /// Number of parallel compilation jobs.
    pub jobs: usize,
    /// Enable verbose output.
    pub verbose: bool,
    /// Main module name (e.g. "Main").
    pub main_module: Option<String>,
    /// Output executable path.
    pub output_exe: Option<PathBuf>,
    /// Output library path.
    pub output_lib: Option<PathBuf>,
    /// Cross-compilation target triple.
    pub target: Option<String>,
    /// Language extensions to enable.
    pub extensions: Vec<String>,
}

impl Default for BhcNativeBuildOptions {
    fn default() -> Self {
        Self {
            src_dirs: vec![PathBuf::from("src")],
            output_dir: PathBuf::from("dist-newstyle"),
            optimization: 1,
            warnings: true,
            werror: false,
            extra_flags: Vec::new(),
            jobs: 4,
            verbose: false,
            main_module: None,
            output_exe: None,
            output_lib: None,
            target: None,
            extensions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bhc_compiler_config_default() {
        let config = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        };

        assert_eq!(config.bhc_path, PathBuf::from("bhc"));
        assert_eq!(config.version, "2026.2.0");
        assert!(!config.tensor_fusion);
        assert!(!config.emit_kernel_report);
        assert!(config.packages.is_empty());
        assert!(config.package_dbs.is_empty());
    }

    #[test]
    fn test_bhc_compiler_config_with_profile() {
        let config = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        }
        .with_profile(BhcProfile::Numeric)
        .with_tensor_fusion(true)
        .with_emit_kernel_report(true)
        .with_packages(vec!["base".to_string(), "text".to_string()]);

        assert_eq!(config.profile.as_str(), "numeric");
        assert!(config.tensor_fusion);
        assert!(config.emit_kernel_report);
        assert_eq!(config.packages, vec!["base", "text"]);
    }

    #[test]
    fn test_detect_package_dbs() {
        let dbs = BhcCompilerConfig::detect_package_dbs("2026.2.0");

        // Should produce paths if home directory exists
        if BaseDirs::new().is_some() {
            assert!(!dbs.is_empty());

            // First path should contain the version-specific cache dir
            let first = dbs[0].to_string_lossy();
            assert!(
                first.contains("bhc-2026.2.0"),
                "expected version in path, got: {}",
                first
            );
            assert!(
                first.ends_with("package.db"),
                "expected package.db suffix, got: {}",
                first
            );

            // Second path should be the global ~/.bhc/package.db
            let second = dbs[1].to_string_lossy();
            assert!(
                second.contains(".bhc"),
                "expected .bhc in global path, got: {}",
                second
            );
            assert!(
                second.ends_with("package.db"),
                "expected package.db suffix, got: {}",
                second
            );
        }
    }

    #[test]
    fn test_bhc_flags() {
        let config = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::Server,
            package_dbs: vec![PathBuf::from("/tmp/test/package.db")],
            packages: vec!["base".to_string()],
            tensor_fusion: true,
            emit_kernel_report: true,
        };

        let flags = config.bhc_flags();

        assert!(flags.contains(&"--profile=server".to_string()));
        // BHC 0.2.3 has no `--tensor-fusion`; kernel report is `--kernel-report`.
        assert!(!flags.iter().any(|f| f == "--tensor-fusion"));
        assert!(flags.contains(&"--kernel-report".to_string()));
        assert!(!flags.iter().any(|f| f == "--emit-kernel-report"));
        assert!(flags.contains(&"--package-db".to_string()));
        assert!(flags.contains(&"/tmp/test/package.db".to_string()));
        assert!(flags.contains(&"--package".to_string()));
        assert!(flags.contains(&"base".to_string()));
    }

    #[test]
    fn test_bhc_flags_minimal() {
        let config = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        };

        let flags = config.bhc_flags();

        assert_eq!(flags, vec!["--profile=default"]);
    }

    #[test]
    fn test_bhc_native_build_options_default() {
        let opts = BhcNativeBuildOptions::default();

        assert_eq!(opts.src_dirs, vec![PathBuf::from("src")]);
        assert_eq!(opts.output_dir, PathBuf::from("dist-newstyle"));
        assert_eq!(opts.optimization, 1);
        assert!(opts.warnings);
        assert!(!opts.werror);
        assert!(opts.extra_flags.is_empty());
        assert_eq!(opts.jobs, 4);
        assert!(!opts.verbose);
        assert!(opts.main_module.is_none());
        assert!(opts.output_exe.is_none());
        assert!(opts.output_lib.is_none());
        assert!(opts.target.is_none());
        assert!(opts.extensions.is_empty());
    }
}
