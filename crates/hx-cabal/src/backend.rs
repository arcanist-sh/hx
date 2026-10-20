//! GHC backend implementation for the compiler abstraction.
//!
//! This module wraps the existing hx-cabal build functionality to implement
//! the `CompilerBackend` trait, allowing GHC to be used alongside alternative
//! compilers like BHC.

use async_trait::async_trait;
use hx_compiler::{
    BuildOptions as CompilerBuildOptions, BuildResult as CompilerBuildResult,
    CheckOptions as CompilerCheckOptions, CheckResult as CompilerCheckResult, CompilerBackend,
    CompilerError, CompilerStatus, Diagnostic, Result as CompilerResult,
    RunOptions as CompilerRunOptions, RunResult as CompilerRunResult,
};
use hx_ui::Output;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tracing::debug;

use crate::build::{self as cabal_build, BuildOptions, BuildResult};

/// GHC compiler backend.
///
/// This backend uses Cabal to orchestrate GHC compilation.
pub struct GhcBackend {
    /// Path to the GHC executable.
    ghc_path: Option<PathBuf>,
    /// Path to the Cabal executable.
    cabal_path: Option<PathBuf>,
    /// Additional toolchain bin directories.
    toolchain_bin_dirs: Vec<PathBuf>,
    /// Build directory for Cabal.
    build_dir: Option<PathBuf>,
}

impl GhcBackend {
    /// Create a new GHC backend.
    pub fn new() -> Self {
        Self {
            ghc_path: None,
            cabal_path: None,
            toolchain_bin_dirs: Vec::new(),
            build_dir: None,
        }
    }

    /// Set the path to the GHC executable.
    pub fn with_ghc_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.ghc_path = Some(path.into());
        self
    }

    /// Set the path to the Cabal executable.
    pub fn with_cabal_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.cabal_path = Some(path.into());
        self
    }

    /// Add toolchain bin directories for PATH.
    pub fn with_toolchain_bin_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.toolchain_bin_dirs = dirs;
        self
    }

    /// Set the build directory.
    pub fn with_build_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.build_dir = Some(dir.into());
        self
    }

    /// Detect GHC version by running `ghc --version`.
    async fn detect_ghc_version(&self) -> CompilerResult<String> {
        let ghc_cmd = self
            .ghc_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "ghc".to_string());

        let output = Command::new(&ghc_cmd).arg("--version").output()?;

        if !output.status.success() {
            return Err(CompilerError::NotFound {
                name: "ghc".to_string(),
            });
        }

        let version_str = String::from_utf8_lossy(&output.stdout);
        // Parse version from "The Glorious Glasgow Haskell Compilation System, version X.Y.Z"
        if let Some(version) = version_str.split("version").nth(1) {
            Ok(version.trim().to_string())
        } else {
            // Try to find version number
            for word in version_str.split_whitespace() {
                if word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Ok(word.to_string());
                }
            }
            Ok(version_str.trim().to_string())
        }
    }

    /// Convert internal BuildResult to CompilerBuildResult.
    fn convert_build_result(&self, result: &BuildResult, start: Instant) -> CompilerBuildResult {
        let warnings: Vec<Diagnostic> = result
            .warnings
            .iter()
            .map(|w| Diagnostic::warning(w.clone()))
            .collect();

        let errors: Vec<Diagnostic> = result
            .errors
            .iter()
            .map(|e| Diagnostic::error(e.clone()))
            .collect();

        CompilerBuildResult {
            success: result.success,
            duration: start.elapsed(),
            modules_compiled: 0, // Cabal doesn't report this directly
            modules_skipped: 0,
            executable: None, // Would need to parse cabal output to find this
            library: None,
            warnings,
            errors,
        }
    }
}

impl Default for GhcBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CompilerBackend for GhcBackend {
    fn name(&self) -> &str {
        "ghc"
    }

    fn description(&self) -> &str {
        "GHC (Glasgow Haskell Compiler) via Cabal"
    }

    async fn detect(&self) -> CompilerResult<CompilerStatus> {
        match self.detect_ghc_version().await {
            Ok(version) => {
                let path = self
                    .ghc_path
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("ghc"));
                Ok(CompilerStatus::Available { version, path })
            }
            Err(_) => Ok(CompilerStatus::NotInstalled),
        }
    }

    async fn version(&self) -> CompilerResult<String> {
        self.detect_ghc_version().await
    }

    async fn build(
        &self,
        project_root: &Path,
        options: &CompilerBuildOptions,
        output: &Output,
    ) -> CompilerResult<CompilerBuildResult> {
        let build_dir = self
            .build_dir
            .clone()
            .unwrap_or_else(|| project_root.join("dist-newstyle"));

        let cabal_options = BuildOptions {
            release: options.release,
            jobs: options.jobs,
            target: options.target.clone(),
            targets: options.package.clone().into_iter().collect(),
            verbose: options.verbose,
            fingerprint: None,
            ghc_version: None,
            package_count: None,
            project_name: None,
            toolchain_bin_dirs: self.toolchain_bin_dirs.clone(),
            extra_args: Vec::new(),
        };

        let start = Instant::now();
        let project_root = project_root.to_path_buf();

        match cabal_build::build(&project_root, &build_dir, &cabal_options, output).await {
            Ok(result) => Ok(self.convert_build_result(&result, start)),
            Err(e) => {
                debug!("GHC build failed: {}", e);
                Err(CompilerError::BuildFailed {
                    error_count: 1,
                    errors: vec![e.to_string()],
                })
            }
        }
    }

    async fn check(
        &self,
        project_root: &Path,
        options: &CompilerCheckOptions,
        output: &Output,
    ) -> CompilerResult<CompilerCheckResult> {
        // For GHC, we use `cabal build` but could use `ghc -fno-code` for pure type-checking
        // For now, delegate to build with minimal options
        let build_options = CompilerBuildOptions {
            release: false,
            optimization: Some(0),
            jobs: None,
            target: None,
            package: options.package.clone(),
            verbose: options.verbose,
            extra_flags: options.extra_flags.clone(),
            src_dirs: Vec::new(),
            werror: false,
        };

        let start = Instant::now();
        let build_result = self.build(project_root, &build_options, output).await?;

        Ok(CompilerCheckResult {
            success: build_result.success,
            duration: start.elapsed(),
            modules_checked: build_result.modules_compiled,
            warnings: build_result.warnings,
            errors: build_result.errors,
        })
    }

    async fn run(
        &self,
        project_root: &Path,
        options: &CompilerRunOptions,
        output: &Output,
    ) -> CompilerResult<CompilerRunResult> {
        let build_dir = self
            .build_dir
            .clone()
            .unwrap_or_else(|| project_root.join("dist-newstyle"));

        let start = Instant::now();

        match cabal_build::run(
            project_root,
            &build_dir,
            &options.args,
            options.package.as_deref(),
            options.target.as_deref(),
            &self.toolchain_bin_dirs,
            &[],
            output,
        )
        .await
        {
            Ok(exit_code) => Ok(CompilerRunResult {
                exit_code,
                duration: start.elapsed(),
            }),
            Err(e) => Err(CompilerError::RunFailed {
                message: e.to_string(),
            }),
        }
    }

    fn parse_diagnostics(&self, raw_output: &str) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        for line in raw_output.lines() {
            // Parse GHC error format: file:line:col: error: message
            if let Some((location, rest)) = line.split_once(": error:") {
                let mut diag = Diagnostic::error(rest.trim());
                if let Some((file, line_col)) = parse_location(location) {
                    if let Some((line_num, col_num)) = line_col {
                        diag = diag.with_location(file, line_num, col_num);
                    } else {
                        diag.file = Some(file);
                    }
                }
                diagnostics.push(diag);
            } else if let Some((location, rest)) = line.split_once(": warning:") {
                let mut diag = Diagnostic::warning(rest.trim());
                if let Some((file, line_col)) = parse_location(location) {
                    if let Some((line_num, col_num)) = line_col {
                        diag = diag.with_location(file, line_num, col_num);
                    } else {
                        diag.file = Some(file);
                    }
                }
                diagnostics.push(diag);
            }
        }

        diagnostics
    }

    fn supported_targets(&self) -> Vec<&str> {
        vec![
            "x86_64-linux-gnu",
            "aarch64-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-w64-mingw32",
        ]
    }
}

/// Parse a GHC location string like "src/Foo.hs:10:5" or "src/Foo.hs".
fn parse_location(location: &str) -> Option<(PathBuf, Option<(u32, u32)>)> {
    let parts: Vec<&str> = location.split(':').collect();
    match parts.len() {
        1 => Some((PathBuf::from(parts[0]), None)),
        2 => {
            let line = parts[1].parse().ok()?;
            Some((PathBuf::from(parts[0]), Some((line, 1))))
        }
        _ => {
            let line = parts[1].parse().ok()?;
            let col = parts[2].parse().ok()?;
            Some((PathBuf::from(parts[0]), Some((line, col))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_compiler::DiagnosticSeverity;

    #[test]
    fn test_parse_location() {
        let (path, pos) = parse_location("src/Foo.hs:10:5").unwrap();
        assert_eq!(path, PathBuf::from("src/Foo.hs"));
        assert_eq!(pos, Some((10, 5)));

        let (path, pos) = parse_location("src/Foo.hs:10").unwrap();
        assert_eq!(path, PathBuf::from("src/Foo.hs"));
        assert_eq!(pos, Some((10, 1)));

        let (path, pos) = parse_location("src/Foo.hs").unwrap();
        assert_eq!(path, PathBuf::from("src/Foo.hs"));
        assert_eq!(pos, None);
    }

    #[test]
    fn test_parse_diagnostics() {
        let backend = GhcBackend::new();

        let output = r#"
src/Main.hs:15:10: error: Variable not in scope: undefined
src/Lib.hs:23:5: warning: Unused variable 'x'
"#;

        let diagnostics = backend.parse_diagnostics(output);
        assert_eq!(diagnostics.len(), 2);

        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].file, Some(PathBuf::from("src/Main.hs")));
        assert_eq!(diagnostics[0].line, Some(15));
        assert_eq!(diagnostics[0].column, Some(10));

        assert_eq!(diagnostics[1].severity, DiagnosticSeverity::Warning);
        assert_eq!(diagnostics[1].file, Some(PathBuf::from("src/Lib.hs")));
        assert_eq!(diagnostics[1].line, Some(23));
        assert_eq!(diagnostics[1].column, Some(5));
    }
}
