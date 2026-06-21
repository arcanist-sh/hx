//! BHC (Basel Haskell Compiler) backend for hx.
//!
//! This crate provides:
//! - BHC compiler backend implementation
//! - BHC-specific build options and diagnostics
//! - bhc.toml manifest generation

pub mod build;
pub mod builtin_packages;
pub mod compile;
pub mod diagnostics;
pub mod full_native;
pub mod manifest;
pub mod native;
pub mod native_builder;
pub mod package_build;
pub mod package_db;
pub mod repl;

use async_trait::async_trait;
use hx_compiler::{
    BuildOptions as CompilerBuildOptions, BuildResult as CompilerBuildResult,
    CheckOptions as CompilerCheckOptions, CheckResult as CompilerCheckResult, CompilerBackend,
    CompilerError, CompilerStatus, Diagnostic, Result as CompilerResult,
    RunOptions as CompilerRunOptions, RunResult as CompilerRunResult,
};
use hx_config::BhcConfig;
use hx_ui::Output;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tracing::{debug, info};

pub use build::BhcBuildOptions;
pub use diagnostics::parse_bhc_output;
pub use manifest::{BhcManifest, generate_bhc_manifest};

/// BHC compiler backend.
///
/// This backend invokes the BHC compiler directly for builds.
pub struct BhcBackend {
    /// Path to the BHC executable.
    bhc_path: Option<PathBuf>,
    /// BHC-specific configuration.
    config: BhcConfig,
}

impl BhcBackend {
    /// Create a new BHC backend.
    pub fn new() -> Self {
        Self {
            bhc_path: None,
            config: BhcConfig::default(),
        }
    }

    /// Set the path to the BHC executable.
    pub fn with_bhc_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.bhc_path = Some(path.into());
        self
    }

    /// Set the BHC configuration.
    pub fn with_config(mut self, config: BhcConfig) -> Self {
        self.config = config;
        self
    }

    /// Get the BHC executable path.
    pub fn bhc_cmd(&self) -> String {
        self.bhc_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "bhc".to_string())
    }

    /// Detect BHC version by running `bhc --version`.
    async fn detect_bhc_version(&self) -> CompilerResult<String> {
        let bhc_cmd = self.bhc_cmd();

        let output = Command::new(&bhc_cmd).arg("--version").output()?;

        if !output.status.success() {
            return Err(CompilerError::NotFound {
                name: "bhc".to_string(),
            });
        }

        let version_str = String::from_utf8_lossy(&output.stdout);
        // Parse version from "BHC version X.Y.Z" or similar
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

    /// Build command-line arguments for BHC test.
    pub fn test_args(
        &self,
        pattern: Option<&str>,
        package: Option<&str>,
        target: Option<&str>,
    ) -> Vec<String> {
        let mut args = vec!["test".to_string()];

        if let Some(pkg) = package {
            args.push(pkg.to_string());
        }

        if let Some(pat) = pattern {
            args.push(format!("--pattern={}", pat));
        }

        if let Some(tgt) = target {
            args.push(format!("--target={}", tgt));
        } else if let Some(ref tgt) = self.config.target {
            args.push(format!("--target={}", tgt));
        }

        args
    }

    /// Build the command-line arguments for a BHC compilation.
    ///
    /// BHC 0.2.3's CLI is `bhc [GLOBAL OPTIONS] [FILES]... -o <out>` — there is
    /// no manifest-driven `build` subcommand (it is a stub), and every option
    /// (`--profile`, `-O`, `-j`, `--target`, `-I`) is global and must precede
    /// the source files. We therefore enumerate the project's `.hs` files and
    /// invoke the top-level form, rather than `bhc build`.
    fn build_args(
        &self,
        options: &CompilerBuildOptions,
        sources: &[PathBuf],
        output_exe: &Path,
    ) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        // Profile (global).
        args.push("--profile".to_string());
        args.push(self.config.profile.as_str().to_string());

        // Optimization level (`-O <n>`, global).
        let opt_level = if options.release {
            2
        } else {
            options.optimization.unwrap_or(1)
        };
        args.push("-O".to_string());
        args.push(opt_level.to_string());

        // Cross-compilation target.
        if let Some(target) = options
            .target
            .as_ref()
            .or(self.config.target.as_ref())
        {
            args.push("--target".to_string());
            args.push(target.clone());
        }

        // Parallelism.
        if let Some(jobs) = options.jobs {
            args.push("-j".to_string());
            args.push(jobs.to_string());
        }

        // Kernel performance report (BHC 0.2.3: `--kernel-report`).
        if self.config.emit_kernel_report {
            args.push("--kernel-report".to_string());
        }

        // Import paths for module resolution, one `-I` per source dir.
        for dir in &options.src_dirs {
            args.push("-I".to_string());
            args.push(dir.to_string_lossy().to_string());
        }

        // Caller-supplied extra flags (verbatim).
        args.extend(options.extra_flags.iter().cloned());

        // Source files to compile.
        for src in sources {
            args.push(src.to_string_lossy().to_string());
        }

        // Output executable.
        args.push("-o".to_string());
        args.push(output_exe.to_string_lossy().to_string());

        args
    }

    /// The directory holding BHC's runtime archives (`libbhc_rts.a`, …).
    ///
    /// BHC's linker step references `-lbhc_rts` without adding a `-L`, so the
    /// caller must put that directory on `LIBRARY_PATH`. The release tarball
    /// ships the libs next to the binary (`<dir>/lib`); an hx-managed install
    /// places the binary in `bin/` with `lib/` a level up. Probe both.
    fn lib_dir(&self) -> Option<PathBuf> {
        let bhc = which::which(self.bhc_cmd()).ok()?;
        let bin_dir = bhc.parent()?;
        [bin_dir.join("lib"), bin_dir.parent()?.join("lib")]
            .into_iter()
            .find(|cand| cand.join("libbhc_rts.a").exists())
    }
}

/// Recursively collect `.hs` source files under the given directories.
fn collect_hs_sources(src_dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "hs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for dir in src_dirs {
        walk(dir, &mut files);
    }
    // Stable order so the command line is deterministic.
    files.sort();
    files
}

/// Whether BHC output indicates a failure even when the process exits 0.
///
/// BHC 0.2.3 frequently exits 0 while printing compile/link errors, so a build
/// cannot be judged by exit status alone. Treat any `error:`-prefixed line, or
/// the known terminal phrases, as failure.
fn output_indicates_failure(combined: &str) -> bool {
    combined.lines().any(|line| {
        let l = line.trim_start();
        l.starts_with("error:")
            || l.contains("type checking failed")
            || l.contains("linking failed")
            || l.contains("code generation failed")
            || l.contains("lowering failed")
    })
}

impl Default for BhcBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CompilerBackend for BhcBackend {
    fn name(&self) -> &str {
        "bhc"
    }

    fn description(&self) -> &str {
        "BHC (Basel Haskell Compiler) - alternative compiler with tensor optimizations"
    }

    async fn detect(&self) -> CompilerResult<CompilerStatus> {
        match self.detect_bhc_version().await {
            Ok(version) => {
                let path = self
                    .bhc_path
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("bhc"));
                Ok(CompilerStatus::Available { version, path })
            }
            Err(_) => Ok(CompilerStatus::NotInstalled),
        }
    }

    async fn version(&self) -> CompilerResult<String> {
        self.detect_bhc_version().await
    }

    async fn build(
        &self,
        project_root: &Path,
        options: &CompilerBuildOptions,
        output: &Output,
    ) -> CompilerResult<CompilerBuildResult> {
        let bhc_cmd = self.bhc_cmd();

        // Resolve the source dirs relative to the project root, defaulting to
        // `src` and `app` (the standard hx layout) when none are configured.
        let src_dirs: Vec<PathBuf> = if options.src_dirs.is_empty() {
            ["src", "app"]
                .iter()
                .map(|d| project_root.join(d))
                .filter(|p| p.exists())
                .collect()
        } else {
            options
                .src_dirs
                .iter()
                .map(|d| {
                    if d.is_absolute() {
                        d.clone()
                    } else {
                        project_root.join(d)
                    }
                })
                .filter(|p| p.exists())
                .collect()
        };

        let sources = collect_hs_sources(&src_dirs);
        if sources.is_empty() {
            return Err(CompilerError::BuildFailed {
                error_count: 1,
                errors: vec![format!(
                    "no Haskell source files (.hs) found under {}",
                    src_dirs
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )],
            });
        }

        // Output executable under .hx so it does not pollute the project.
        let out_dir = project_root.join(".hx").join("bhc-build");
        let _ = std::fs::create_dir_all(&out_dir);
        let exe_name = project_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "main".to_string());
        let output_exe = out_dir.join(exe_name);

        // BHC compiles with absolute `-I` dirs; pass sources relative to the
        // project root for a tidy command line.
        let options_abs = CompilerBuildOptions {
            src_dirs: src_dirs.clone(),
            ..options.clone()
        };
        let args = self.build_args(&options_abs, &sources, &output_exe);

        info!("Running BHC build in {}", project_root.display());
        debug!("Args: {:?}", args);

        output.status(
            "Building",
            &format!("with BHC ({})", self.config.profile.as_str()),
        );

        let start = Instant::now();

        // BHC's linker references `-lbhc_rts` without a `-L`; its runtime libs
        // must be discoverable via LIBRARY_PATH or linking fails.
        let mut command = Command::new(&bhc_cmd);
        command.args(&args).current_dir(project_root);
        if let Some(lib_dir) = self.lib_dir() {
            let prev = std::env::var("LIBRARY_PATH").unwrap_or_default();
            let combined = if prev.is_empty() {
                lib_dir.to_string_lossy().to_string()
            } else {
                format!("{}:{}", lib_dir.display(), prev)
            };
            command.env("LIBRARY_PATH", combined);
        }

        let cmd_output = command.output()?;

        let duration = start.elapsed();

        let stdout = String::from_utf8_lossy(&cmd_output.stdout);
        let stderr = String::from_utf8_lossy(&cmd_output.stderr);

        // BHC 0.2.3 may exit 0 while printing errors, so judge success on both
        // the exit status and the output.
        let success = cmd_output.status.success()
            && !output_indicates_failure(&stderr)
            && !output_indicates_failure(&stdout);

        // Parse diagnostics from output
        let diagnostics = diagnostics::parse_bhc_output(&stderr);
        let (warnings, errors): (Vec<_>, Vec<_>) = diagnostics
            .into_iter()
            .partition(|d| d.severity == hx_compiler::DiagnosticSeverity::Warning);

        if options.verbose || !success {
            if !stdout.is_empty() {
                output.verbose(&stdout);
            }
            if !stderr.is_empty() {
                if success {
                    output.verbose(&stderr);
                } else {
                    eprintln!("{}", stderr);
                }
            }
        }

        if success {
            output.status(
                "Finished",
                &format!("BHC build in {:.2}s", duration.as_secs_f64()),
            );
        } else {
            // If the diagnostics parser found nothing structured (BHC's error
            // format varies), fall back to the raw stderr so the failure is
            // never silent.
            let messages: Vec<String> = if errors.is_empty() {
                let tail: String = stderr
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                vec![if tail.is_empty() {
                    "BHC reported a build failure".to_string()
                } else {
                    tail
                }]
            } else {
                errors.iter().map(|e| e.message.clone()).collect()
            };
            return Err(CompilerError::BuildFailed {
                error_count: messages.len(),
                errors: messages,
            });
        }

        Ok(CompilerBuildResult {
            success,
            duration,
            modules_compiled: 0, // Would need to parse BHC output
            modules_skipped: 0,
            executable: None,
            library: None,
            warnings,
            errors,
        })
    }

    async fn check(
        &self,
        project_root: &Path,
        options: &CompilerCheckOptions,
        output: &Output,
    ) -> CompilerResult<CompilerCheckResult> {
        let bhc_cmd = self.bhc_cmd();
        let mut args = vec!["check".to_string()];

        if let Some(ref pkg) = options.package {
            args.push(pkg.clone());
        }

        args.extend(options.extra_flags.iter().cloned());

        info!("Running BHC check in {}", project_root.display());
        debug!("Args: {:?}", args);

        output.status("Checking", "with BHC");

        let start = Instant::now();

        let cmd_output = Command::new(&bhc_cmd)
            .args(&args)
            .current_dir(project_root)
            .output()?;

        let duration = start.elapsed();
        let success = cmd_output.status.success();

        let stderr = String::from_utf8_lossy(&cmd_output.stderr);

        // Parse diagnostics from output
        let diagnostics = diagnostics::parse_bhc_output(&stderr);
        let (warnings, errors): (Vec<_>, Vec<_>) = diagnostics
            .into_iter()
            .partition(|d| d.severity == hx_compiler::DiagnosticSeverity::Warning);

        if !success {
            return Err(CompilerError::CheckFailed {
                error_count: errors.len(),
                errors: errors.iter().map(|e| e.message.clone()).collect(),
            });
        }

        Ok(CompilerCheckResult {
            success,
            duration,
            modules_checked: 0,
            warnings,
            errors,
        })
    }

    async fn run(
        &self,
        project_root: &Path,
        options: &CompilerRunOptions,
        output: &Output,
    ) -> CompilerResult<CompilerRunResult> {
        let bhc_cmd = self.bhc_cmd();
        let mut args = vec!["run".to_string()];

        if let Some(ref pkg) = options.package {
            args.push(pkg.clone());
        }

        // Add program arguments after --
        if !options.args.is_empty() {
            args.push("--".to_string());
            args.extend(options.args.iter().cloned());
        }

        info!("Running BHC run in {}", project_root.display());
        debug!("Args: {:?}", args);

        output.status("Running", "with BHC");

        let start = Instant::now();

        let status = Command::new(&bhc_cmd)
            .args(&args)
            .current_dir(project_root)
            .status()?;

        let duration = start.elapsed();
        let exit_code = status.code().unwrap_or(1);

        if exit_code != 0 {
            return Err(CompilerError::RunFailed {
                message: format!("BHC run exited with code {}", exit_code),
            });
        }

        Ok(CompilerRunResult {
            exit_code,
            duration,
        })
    }

    fn parse_diagnostics(&self, raw_output: &str) -> Vec<Diagnostic> {
        diagnostics::parse_bhc_output(raw_output)
    }

    fn supported_targets(&self) -> Vec<&str> {
        vec![
            "x86_64-linux-gnu",
            "aarch64-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            // BHC-specific targets
            "wasm32-wasi",
            "riscv64-linux-gnu",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_config::BhcProfile;

    #[test]
    fn test_build_args_default() {
        let backend = BhcBackend::new();
        let options = CompilerBuildOptions::default();
        let sources = vec![PathBuf::from("src/Main.hs")];
        let out = PathBuf::from(".hx/bhc-build/app");
        let args = backend.build_args(&options, &sources, &out);

        // No `build` subcommand; `--profile` and `-O` are global (separate
        // tokens), and the source file + `-o <out>` come last.
        assert!(!args.contains(&"build".to_string()));
        assert!(args.windows(2).any(|w| w == ["--profile", "default"]));
        assert!(args.windows(2).any(|w| w == ["-O", "1"]));
        assert!(args.contains(&"src/Main.hs".to_string()));
        let o = args.iter().position(|a| a == "-o").unwrap();
        assert_eq!(args[o + 1], ".hx/bhc-build/app");
    }

    #[test]
    fn test_build_args_release() {
        let backend = BhcBackend::new();
        let options = CompilerBuildOptions {
            release: true,
            ..Default::default()
        };
        let args = backend.build_args(&options, &[], &PathBuf::from("out"));

        assert!(args.windows(2).any(|w| w == ["-O", "2"]));
    }

    #[test]
    fn test_test_args_default() {
        let backend = BhcBackend::new();
        let args = backend.test_args(None, None, None);

        assert_eq!(args, vec!["test".to_string()]);
    }

    #[test]
    fn test_test_args_with_pattern() {
        let backend = BhcBackend::new();
        let args = backend.test_args(Some("MyModule"), Some("my-pkg"), None);

        assert!(args.contains(&"test".to_string()));
        assert!(args.contains(&"my-pkg".to_string()));
        assert!(args.contains(&"--pattern=MyModule".to_string()));
    }

    #[test]
    fn test_build_args_with_config() {
        let config = BhcConfig {
            profile: BhcProfile::Numeric,
            emit_kernel_report: true,
            tensor_fusion: true,
            target: Some("aarch64-linux-gnu".to_string()),
        };
        let backend = BhcBackend::new().with_config(config);
        let options = CompilerBuildOptions::default();
        let args = backend.build_args(&options, &[], &PathBuf::from("out"));

        // Global flags as separate tokens; the kernel report is `--kernel-report`
        // and there is no `--tensor-fusion` in BHC 0.2.3.
        assert!(args.windows(2).any(|w| w == ["--profile", "numeric"]));
        assert!(args.contains(&"--kernel-report".to_string()));
        assert!(!args.iter().any(|a| a == "--tensor-fusion"));
        assert!(!args.iter().any(|a| a == "--emit-kernel-report"));
        assert!(args.windows(2).any(|w| w == ["--target", "aarch64-linux-gnu"]));
    }

    #[test]
    fn test_output_indicates_failure() {
        // BHC 0.2.3 exits 0 while printing these; we must catch them.
        assert!(output_indicates_failure("error: type checking failed: 2 errors"));
        assert!(output_indicates_failure("  Compute FAILED: lowering failed: 1 error"));
        assert!(output_indicates_failure(
            "ld: library 'bhc_rts' not found\nlinking failed"
        ));
        // Clean output is not a failure.
        assert!(!output_indicates_failure("Compiled 3 modules\nLinked ./app"));
        assert!(!output_indicates_failure(""));
    }

    #[test]
    fn test_collect_hs_sources_recurses_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/Sub")).unwrap();
        std::fs::write(root.join("src/B.hs"), "module B where").unwrap();
        std::fs::write(root.join("src/Sub/A.hs"), "module Sub.A where").unwrap();
        std::fs::write(root.join("src/notes.txt"), "ignore me").unwrap();

        let found = collect_hs_sources(&[root.join("src")]);
        assert_eq!(found.len(), 2);
        // Only `.hs` files, in a deterministic (component-wise sorted) order:
        // `src/B.hs` precedes `src/Sub/A.hs` because "B.hs" < "Sub".
        assert!(found[0].ends_with("src/B.hs"));
        assert!(found[1].ends_with("src/Sub/A.hs"));
    }
}
