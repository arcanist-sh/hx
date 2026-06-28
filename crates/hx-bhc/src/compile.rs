//! Single-module BHC compilation.
//!
//! Provides functionality to compile individual Haskell modules using the BHC
//! compiler, including argument construction, process execution, and diagnostic
//! parsing.

use crate::diagnostics::parse_bhc_output;
use crate::native::{BhcCompilerConfig, BhcNativeBuildOptions, BhcNativeError};
use hx_compiler::Diagnostic;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, warn};

/// Result of compiling a single module.
#[derive(Debug)]
pub struct ModuleCompileResult {
    /// Fully qualified module name (e.g. "Data.List").
    pub module_name: String,
    /// Path to the generated object file.
    pub object_file: PathBuf,
    /// Path to the generated interface file.
    pub interface_file: PathBuf,
    /// Whether compilation succeeded.
    pub success: bool,
    /// Warning diagnostics emitted during compilation.
    pub warnings: Vec<Diagnostic>,
    /// Error diagnostics emitted during compilation.
    pub errors: Vec<Diagnostic>,
}

/// Build the argument list for a single-module BHC compilation.
///
/// This is extracted as a standalone function to support unit testing of
/// argument construction without spawning a process.
pub fn build_compile_args(
    bhc: &BhcCompilerConfig,
    source_file: &Path,
    build_dir: &Path,
    options: &BhcNativeBuildOptions,
    dependency_ids: &[String],
) -> Vec<String> {
    let mut args = Vec::new();

    // Core compilation flag
    args.push("-c".to_string());
    args.push(source_file.to_string_lossy().to_string());

    // Output directories for interface and object files
    let hi_dir = build_dir.join("hi");
    let o_dir = build_dir.join("o");
    args.push("--hidir".to_string());
    args.push(hi_dir.to_string_lossy().to_string());
    args.push("--odir".to_string());
    args.push(o_dir.to_string_lossy().to_string());

    // Source directories as search paths
    for src_dir in &options.src_dirs {
        args.push("--import-path".to_string());
        args.push(src_dir.to_string_lossy().to_string());
    }

    // Package databases
    for db in &bhc.package_dbs {
        args.push("--package-db".to_string());
        args.push(db.to_string_lossy().to_string());
    }

    // Dependency package IDs (BHC spells this `--package-id`, matching its other
    // long flags like `--package-db`).
    for dep_id in dependency_ids {
        args.push("--package-id".to_string());
        args.push(dep_id.clone());
    }

    // Optimization level
    args.push("-O".to_string());
    args.push(options.optimization.to_string());

    // Profile
    args.push(format!("--profile={}", bhc.profile.as_str()));

    // Tensor fusion
    if bhc.tensor_fusion {
        args.push("--tensor-fusion".to_string());
    }

    // Language extensions
    for ext in &options.extensions {
        args.push(format!("-X{}", ext));
    }

    // Warnings
    if options.warnings {
        args.push("--Wall".to_string());
    }

    if options.werror {
        args.push("--Werror".to_string());
    }

    // Extra flags
    args.extend(options.extra_flags.iter().cloned());

    args
}

/// Compile a single Haskell module with BHC.
///
/// Invokes the BHC compiler on the given source file, captures stdout and
/// stderr, parses diagnostics, and returns a structured result.
pub async fn compile_module(
    bhc: &BhcCompilerConfig,
    module_name: &str,
    source_file: &Path,
    build_dir: &Path,
    options: &BhcNativeBuildOptions,
    dependency_ids: &[String],
) -> Result<ModuleCompileResult, BhcNativeError> {
    let args = build_compile_args(bhc, source_file, build_dir, options, dependency_ids);

    debug!(
        module = module_name,
        source = %source_file.display(),
        "Compiling module"
    );
    debug!("BHC args: {:?}", args);

    let output = Command::new(&bhc.bhc_path)
        .args(&args)
        .output()
        .await
        .map_err(|e| BhcNativeError::Io {
            message: format!("failed to execute BHC for module {}: {}", module_name, e),
            source: e,
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let success = output.status.success();

    if !stdout.is_empty() {
        debug!("BHC stdout for {}: {}", module_name, stdout.trim());
    }

    // Parse diagnostics from stderr
    let diagnostics = parse_bhc_output(&stderr);
    let (warnings, errors): (Vec<_>, Vec<_>) = diagnostics
        .into_iter()
        .partition(|d| d.severity == hx_compiler::DiagnosticSeverity::Warning);

    if !success {
        warn!(
            module = module_name,
            error_count = errors.len(),
            "Module compilation failed"
        );
    }

    // Compute expected output paths
    // Module "Data.List" => hi/Data/List.bhi and o/Data/List.o
    let module_path_component = module_name.replace('.', std::path::MAIN_SEPARATOR_STR);
    let hi_dir = build_dir.join("hi");
    let o_dir = build_dir.join("o");
    let interface_file = hi_dir.join(format!("{}.bhi", module_path_component));
    let object_file = o_dir.join(format!("{}.o", module_path_component));

    Ok(ModuleCompileResult {
        module_name: module_name.to_string(),
        object_file,
        interface_file,
        success,
        warnings,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_config::BhcProfile;

    /// Helper to create a test compiler config.
    fn test_config() -> BhcCompilerConfig {
        BhcCompilerConfig {
            bhc_path: PathBuf::from("/usr/local/bin/bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        }
    }

    #[test]
    fn test_compile_args_construction() {
        let bhc = BhcCompilerConfig {
            bhc_path: PathBuf::from("/usr/local/bin/bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::Numeric,
            package_dbs: vec![PathBuf::from("/tmp/pkg.db")],
            packages: Vec::new(),
            tensor_fusion: true,
            emit_kernel_report: false,
        };

        let options = BhcNativeBuildOptions {
            src_dirs: vec![PathBuf::from("src"), PathBuf::from("lib")],
            optimization: 2,
            warnings: true,
            werror: true,
            extensions: vec!["OverloadedStrings".to_string(), "GADTs".to_string()],
            extra_flags: vec!["-fforce-recomp".to_string()],
            ..Default::default()
        };

        let dep_ids = vec!["base-4.18.0-abc123".to_string()];

        let args = build_compile_args(
            &bhc,
            Path::new("src/Main.hs"),
            Path::new("dist/build"),
            &options,
            &dep_ids,
        );

        // Core flags
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"src/Main.hs".to_string()));

        // Output directories. Build the expected paths with `join` so the
        // separator matches the platform (the code under test uses join too),
        // otherwise this fails on Windows where join yields `dist/build\hi`.
        assert!(args.contains(&"--hidir".to_string()));
        assert!(
            args.contains(
                &Path::new("dist/build")
                    .join("hi")
                    .to_string_lossy()
                    .to_string()
            )
        );
        assert!(args.contains(&"--odir".to_string()));
        assert!(
            args.contains(
                &Path::new("dist/build")
                    .join("o")
                    .to_string_lossy()
                    .to_string()
            )
        );

        // Source dirs
        assert!(args.contains(&"--import-path".to_string()));
        assert!(args.contains(&"src".to_string()));
        assert!(args.contains(&"lib".to_string()));

        // Package DB
        assert!(args.contains(&"--package-db".to_string()));
        assert!(args.contains(&"/tmp/pkg.db".to_string()));

        // Dependency IDs
        assert!(args.contains(&"--package-id".to_string()));
        assert!(args.contains(&"base-4.18.0-abc123".to_string()));

        // Optimization
        assert!(args.contains(&"-O".to_string()));
        assert!(args.contains(&"2".to_string()));

        // Profile
        assert!(args.contains(&"--profile=numeric".to_string()));

        // Tensor fusion
        assert!(args.contains(&"--tensor-fusion".to_string()));

        // Extensions
        assert!(args.contains(&"-XOverloadedStrings".to_string()));
        assert!(args.contains(&"-XGADTs".to_string()));

        // Warnings
        assert!(args.contains(&"--Wall".to_string()));
        assert!(args.contains(&"--Werror".to_string()));

        // Extra flags
        assert!(args.contains(&"-fforce-recomp".to_string()));
    }

    #[test]
    fn test_compile_args_minimal() {
        let bhc = test_config();
        let options = BhcNativeBuildOptions::default();

        let args = build_compile_args(
            &bhc,
            Path::new("src/Main.hs"),
            Path::new("dist/build"),
            &options,
            &[],
        );

        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"src/Main.hs".to_string()));
        assert!(args.contains(&"-O".to_string()));
        assert!(args.contains(&"1".to_string()));
        assert!(args.contains(&"--profile=default".to_string()));
        assert!(args.contains(&"--Wall".to_string()));
        // No --Werror by default
        assert!(!args.contains(&"--Werror".to_string()));
        // No tensor fusion by default
        assert!(!args.contains(&"--tensor-fusion".to_string()));
    }

    #[test]
    fn test_compile_args_no_warnings() {
        let bhc = test_config();
        let options = BhcNativeBuildOptions {
            warnings: false,
            ..Default::default()
        };

        let args = build_compile_args(
            &bhc,
            Path::new("src/Main.hs"),
            Path::new("dist/build"),
            &options,
            &[],
        );

        assert!(!args.contains(&"--Wall".to_string()));
    }
}
