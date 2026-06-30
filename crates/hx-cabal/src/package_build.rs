//! Native package compilation from source.
//!
//! This module handles building a single Haskell dependency package from source
//! using direct GHC invocation, without relying on Cabal.

use crate::native::{CCompileOptions, CCompilerConfig, GhcConfig, compile_c_sources};
use hx_core::{CommandRunner, Error, Fix, Result};
use hx_solver::{ExtractedPackage, LibraryConfig};
use hx_ui::Output;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Configuration for building a single package.
#[derive(Debug, Clone)]
pub struct PackageBuildConfig {
    /// GHC configuration
    pub ghc: GhcConfig,
    /// C compiler configuration (None to auto-detect when needed)
    pub cc: Option<CCompilerConfig>,
    /// Build output directory
    pub build_dir: PathBuf,
    /// Installation directory (for final artifacts)
    pub install_dir: PathBuf,
    /// Already-built dependencies (package IDs), exposed via `-package-id`.
    pub dependency_ids: Vec<String>,
    /// Pre-installed boot packages to expose by name via `-package <name>`
    /// (GHC keeps boot packages like `containers`/`text` hidden by default).
    pub expose_packages: Vec<String>,
    /// Number of parallel jobs
    pub jobs: usize,
    /// Optimization level (0, 1, or 2)
    pub optimization: u8,
    /// Enable verbose output
    pub verbose: bool,
    /// Name → version of every package available to this build (source-built
    /// dependencies plus GHC boot packages), used to generate the
    /// `cabal_macros.h` CPP version macros (`MIN_VERSION_<pkg>` / `VERSION_<pkg>`).
    pub package_versions: Vec<(String, String)>,
}

impl Default for PackageBuildConfig {
    fn default() -> Self {
        Self {
            ghc: GhcConfig::default(),
            cc: None,
            build_dir: PathBuf::from(".hx/pkg-build"),
            install_dir: PathBuf::from(".hx/pkg-install"),
            dependency_ids: Vec::new(),
            expose_packages: Vec::new(),
            jobs: num_cpus::get(),
            optimization: 1,
            verbose: false,
            package_versions: Vec::new(),
        }
    }
}

/// Result of building a package.
#[derive(Debug, Clone)]
pub struct PackageBuildResult {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Generated package ID (e.g., "text-2.1.1-abc123")
    pub package_id: String,
    /// Path to the installed library archive
    pub library_path: Option<PathBuf>,
    /// Path to the package registration file
    pub registration_file: PathBuf,
    /// Whether the build succeeded
    pub success: bool,
    /// Build duration
    pub duration: Duration,
    /// Number of Haskell modules compiled
    pub modules_compiled: usize,
    /// Number of C source files compiled
    pub c_sources_compiled: usize,
    /// Warnings during build
    pub warnings: Vec<String>,
    /// Errors during build
    pub errors: Vec<String>,
}

/// Build a single package from source.
pub async fn build_package(
    package: &ExtractedPackage,
    config: &PackageBuildConfig,
    output: &Output,
) -> Result<PackageBuildResult> {
    let start = Instant::now();
    let name = &package.name;
    let version = &package.version.to_string();

    info!("Building package: {}-{}", name, version);

    // Check if this package can be built natively
    if !package.can_build_native() {
        let reason = package
            .skip_reason()
            .unwrap_or_else(|| "unknown".to_string());
        return Err(Error::BuildFailed {
            errors: vec![format!("Cannot build {} natively: {}", name, reason)],
            fixes: vec![Fix::new("Use cabal to build this package instead")],
        });
    }

    let Some(lib_config) = &package.build_info.library else {
        return Err(Error::BuildFailed {
            errors: vec![format!("Package {} has no library component", name)],
            fixes: vec![],
        });
    };

    // Generate package ID
    let package_id = compute_package_id(name, version, &config.ghc.version, &config.dependency_ids);

    // Set up build directories
    let pkg_build_dir = config.build_dir.join(format!("{}-{}", name, version));
    let hi_dir = pkg_build_dir.join("hi");
    let o_dir = pkg_build_dir.join("o");

    std::fs::create_dir_all(&hi_dir).map_err(|e| Error::Io {
        message: "Failed to create build directory".to_string(),
        path: Some(hi_dir.clone()),
        source: e,
    })?;
    std::fs::create_dir_all(&o_dir).map_err(|e| Error::Io {
        message: "Failed to create build directory".to_string(),
        path: Some(o_dir.clone()),
        source: e,
    })?;

    // Set up install directories
    let pkg_install_dir = config.install_dir.join(&package_id);
    let lib_install_dir = pkg_install_dir.join("lib");
    let hi_install_dir = pkg_install_dir.join("hi");

    std::fs::create_dir_all(&lib_install_dir).map_err(|e| Error::Io {
        message: "Failed to create install directory".to_string(),
        path: Some(lib_install_dir.clone()),
        source: e,
    })?;
    std::fs::create_dir_all(&hi_install_dir).map_err(|e| Error::Io {
        message: "Failed to create install directory".to_string(),
        path: Some(hi_install_dir.clone()),
        source: e,
    })?;

    // Initialize collections for warnings and errors
    let mut all_warnings = Vec::new();
    let mut all_errors = Vec::new();

    // Compile C sources if present
    let c_sources: Vec<PathBuf> = lib_config.c_sources.iter().map(PathBuf::from).collect();

    let mut c_object_files = Vec::new();
    let c_sources_compiled = c_sources.len();

    if !c_sources.is_empty() {
        if config.verbose {
            output.info(&format!(
                "Compiling {} C source files for {}",
                c_sources.len(),
                name
            ));
        }

        // Get or detect C compiler
        let cc = match &config.cc {
            Some(cc) => cc.clone(),
            None => CCompilerConfig::detect().await?,
        };

        let c_output_dir = o_dir.join("cbits");

        // The package's own include dirs, plus GHC's RTS include dirs so that
        // C sources can `#include <HsFFI.h>`.
        let mut include_dirs: Vec<PathBuf> = lib_config
            .include_dirs
            .iter()
            .map(|s| package.source_dir.join(s))
            .collect();
        include_dirs.extend(config.ghc.rts_include_dirs.iter().cloned());

        let c_options = CCompileOptions {
            include_dirs,
            defines: Vec::new(),
            optimization: config.optimization.to_string(),
            extra_flags: lib_config.cc_options.clone(),
            pic: true,
            output_dir: c_output_dir.clone(),
            verbose: config.verbose,
        };

        let c_result = compile_c_sources(&cc, &c_sources, &c_options, &package.source_dir).await?;

        if !c_result.success {
            return Ok(PackageBuildResult {
                name: name.clone(),
                version: version.clone(),
                package_id,
                library_path: None,
                registration_file: PathBuf::new(),
                success: false,
                duration: start.elapsed(),
                modules_compiled: 0,
                c_sources_compiled,
                warnings: c_result.warnings,
                errors: c_result.errors,
            });
        }

        c_object_files = c_result.object_files;

        if config.verbose && !c_result.warnings.is_empty() {
            for warning in &c_result.warnings {
                output.warn(warning);
            }
        }
    }

    // Run preprocessors if needed
    let src_dirs: Vec<PathBuf> = if lib_config.hs_source_dirs.is_empty() {
        vec![package.source_dir.clone()]
    } else {
        lib_config
            .hs_source_dirs
            .iter()
            .map(|d| package.source_dir.join(d))
            .collect()
    };

    let preproc_sources = crate::preprocessor::detect_preprocessors(&src_dirs);
    let mut generated_dir: Option<PathBuf> = None;

    if !preproc_sources.is_empty() {
        if config.verbose {
            output.info(&format!(
                "Preprocessing {} files for {}",
                preproc_sources.total_files(),
                name
            ));
        }

        // Check preprocessor availability (fail gracefully if missing)
        match crate::preprocessor::check_availability(&preproc_sources, Some(&config.ghc.ghc_path))
            .await
        {
            Ok(available) => {
                let gen_dir = pkg_build_dir.join("generated");
                let preproc_config = crate::preprocessor::PreprocessorConfig {
                    include_dirs: lib_config
                        .include_dirs
                        .iter()
                        .map(|s| package.source_dir.join(s))
                        .collect(),
                    cpp_defines: Vec::new(),
                    output_dir: gen_dir.clone(),
                    verbose: config.verbose,
                    cc_options: lib_config.cpp_options.clone(),
                    ld_options: Vec::new(),
                };

                match crate::preprocessor::preprocess_all(
                    &preproc_sources,
                    &preproc_config,
                    &config.ghc.ghc_path,
                    &available,
                )
                .await
                {
                    Ok(results) => {
                        for result in &results {
                            all_warnings.extend(result.warnings.clone());
                        }
                        if gen_dir.exists() {
                            generated_dir = Some(gen_dir);
                        }
                    }
                    Err(e) => {
                        return Ok(PackageBuildResult {
                            name: name.clone(),
                            version: version.clone(),
                            package_id,
                            library_path: None,
                            registration_file: PathBuf::new(),
                            success: false,
                            duration: start.elapsed(),
                            modules_compiled: 0,
                            c_sources_compiled,
                            warnings: all_warnings,
                            errors: vec![format!("Preprocessing failed: {}", e)],
                        });
                    }
                }
            }
            Err(e) => {
                // Log warning but continue - some packages list build-tools they don't actually need
                if config.verbose {
                    output.warn(&format!("Preprocessors not available for {}: {}", name, e));
                }
            }
        }
    }

    // Get all modules to compile, in dependency (topological) order. A package's
    // exposed module commonly imports its own internal modules (e.g. split's
    // `Data.List.Split` imports `Data.List.Split.Internals`); compiling in cabal
    // declaration order then fails because the sibling `.hi` does not exist yet.
    let extra_src_dirs: Vec<PathBuf> = generated_dir.iter().cloned().collect();
    let mut graph_dirs = src_dirs.clone();
    graph_dirs.extend(extra_src_dirs.iter().cloned());
    // The graph gives a correct compile ORDER, but scanning a package whose
    // `hs-source-dirs` is `.` also sweeps in `test/`, `benchmarks/`, `Setup.hs`,
    // etc. Restrict the order to the library's declared modules so stray
    // non-library modules (e.g. `test.Main`) are not compiled as part of the lib.
    let declared = collect_modules(lib_config);
    let declared_set: std::collections::HashSet<&str> =
        declared.iter().map(|s| s.as_str()).collect();
    let modules =
        match hx_solver::build_module_graph(&graph_dirs).and_then(|g| g.topological_order()) {
            Ok(order) if !order.is_empty() => {
                if declared_set.is_empty() {
                    order
                } else {
                    let filtered: Vec<String> = order
                        .into_iter()
                        .filter(|m| declared_set.contains(m.as_str()))
                        .collect();
                    if filtered.is_empty() {
                        declared
                    } else {
                        filtered
                    }
                }
            }
            _ => declared,
        };
    let module_count = modules.len();

    if config.verbose {
        output.info(&format!("Compiling {} modules for {}", module_count, name));
    }

    // Generate cabal_macros.h so version-conditional CPP (`#if MIN_VERSION_foo`)
    // selects the same branch cabal would. Best-effort: a write failure just
    // means no macros file (and packages not using such CPP are unaffected).
    let macros_path = pkg_build_dir.join("cabal_macros.h");
    let macros_file = {
        let content = generate_cabal_macros(&config.package_versions, name, version);
        match std::fs::write(&macros_path, content) {
            Ok(()) => Some(macros_path.as_path()),
            Err(_) => None,
        }
    };

    // Compile each module
    let mut object_files = Vec::new();

    for module in &modules {
        let result = compile_package_module(
            module,
            package,
            lib_config,
            config,
            &package_id,
            &hi_dir,
            &o_dir,
            &extra_src_dirs,
            macros_file,
        )
        .await;

        all_warnings.extend(result.warnings);

        if !result.success {
            all_errors.extend(result.errors);
            return Ok(PackageBuildResult {
                name: name.clone(),
                version: version.clone(),
                package_id,
                library_path: None,
                registration_file: PathBuf::new(),
                success: false,
                duration: start.elapsed(),
                modules_compiled: object_files.len(),
                c_sources_compiled,
                warnings: all_warnings,
                errors: all_errors,
            });
        }

        if let Some(obj) = result.object_file {
            object_files.push(obj);
        }
    }

    // Create static library (including C object files)
    let lib_name = format!("libHS{}-{}.a", name, version);
    let lib_path = lib_install_dir.join(&lib_name);

    // Combine Haskell and C object files
    let mut all_object_files = object_files.clone();
    all_object_files.extend(c_object_files);

    if !all_object_files.is_empty() {
        create_library(&lib_path, &all_object_files, config.verbose).await?;
    }

    // Copy interface files to install directory
    copy_interface_files(&hi_dir, &hi_install_dir)?;

    // Generate package registration file
    let reg_file = pkg_install_dir.join(format!("{}.conf", package_id));
    generate_registration_file(
        &reg_file,
        name,
        version,
        &package_id,
        lib_config,
        &lib_install_dir,
        &hi_install_dir,
        &config.dependency_ids,
    )?;

    output.status("Built", &format!("{}-{}", name, version));

    Ok(PackageBuildResult {
        name: name.clone(),
        version: version.clone(),
        package_id,
        library_path: Some(lib_path),
        registration_file: reg_file,
        success: true,
        duration: start.elapsed(),
        modules_compiled: module_count,
        c_sources_compiled,
        warnings: all_warnings,
        errors: all_errors,
    })
}

/// Compute a unique package ID based on inputs.
fn compute_package_id(
    name: &str,
    version: &str,
    ghc_version: &str,
    dependency_ids: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("name:{}", name));
    hasher.update(format!("version:{}", version));
    hasher.update(format!("ghc:{}", ghc_version));

    let mut sorted_deps = dependency_ids.to_vec();
    sorted_deps.sort();
    for dep in &sorted_deps {
        hasher.update(format!("dep:{}", dep));
    }

    let hash = hasher.finalize();
    let hash_prefix = &format!("{:x}", hash)[..12];

    format!("{}-{}-{}", name, version, hash_prefix)
}

/// Collect all modules to compile from library config.
fn collect_modules(lib: &LibraryConfig) -> Vec<String> {
    let mut modules = Vec::new();
    modules.extend(lib.exposed_modules.clone());
    modules.extend(lib.other_modules.clone());
    modules
}

/// Generate the contents of cabal's `cabal_macros.h` for a package build.
///
/// For each available package it emits `VERSION_<pkg>` (the full version string)
/// and `MIN_VERSION_<pkg>(major1,major2,minor)` (a comparison against the first
/// three version components), plus `CURRENT_PACKAGE_VERSION` for the package
/// being built. The macro name is the package name with every non-alphanumeric
/// character replaced by `_`, matching Cabal. This lets dependencies that use
/// `#if MIN_VERSION_base(4,18,0)` style CPP compile the same branch they would
/// under cabal.
fn generate_cabal_macros(
    package_versions: &[(String, String)],
    own_name: &str,
    own_version: &str,
) -> String {
    let mut out = String::from("/* DO NOT EDIT: generated by hx for native build */\n");

    let mut seen = std::collections::HashSet::new();
    for (name, version) in package_versions {
        let macro_name = sanitize_macro_name(name);
        if !seen.insert(macro_name.clone()) {
            continue;
        }
        let (v0, v1, v2) = version_triple(version);
        out.push_str(&format!("/* package {name}-{version} */\n"));
        out.push_str(&format!("#ifndef VERSION_{macro_name}\n"));
        out.push_str(&format!("#define VERSION_{macro_name} \"{version}\"\n"));
        out.push_str(&format!("#endif /* VERSION_{macro_name} */\n"));
        out.push_str(&format!("#ifndef MIN_VERSION_{macro_name}\n"));
        out.push_str(&format!(
            "#define MIN_VERSION_{macro_name}(major1,major2,minor) (\\\n"
        ));
        out.push_str(&format!("  (major1) <  {v0} || \\\n"));
        out.push_str(&format!("  (major1) == {v0} && (major2) <  {v1} || \\\n"));
        out.push_str(&format!(
            "  (major1) == {v0} && (major2) == {v1} && (minor) <= {v2})\n"
        ));
        out.push_str(&format!("#endif /* MIN_VERSION_{macro_name} */\n"));
    }

    out.push_str("#ifndef CURRENT_PACKAGE_VERSION\n");
    out.push_str(&format!(
        "#define CURRENT_PACKAGE_VERSION \"{own_version}\"\n"
    ));
    out.push_str("#endif /* CURRENT_PACKAGE_VERSION */\n");
    let _ = own_name;
    out
}

/// Replace every non-alphanumeric character with `_` (Cabal's macro-name rule).
fn sanitize_macro_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// First three numeric components of a version (`4.19.1.0` → `(4, 19, 1)`),
/// padding missing components with 0 and ignoring non-numeric junk.
fn version_triple(version: &str) -> (u64, u64, u64) {
    let mut it = version.split('.').map(|c| c.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Result of compiling a single module.
struct ModuleCompileResult {
    success: bool,
    object_file: Option<PathBuf>,
    warnings: Vec<String>,
    errors: Vec<String>,
}

/// Compile a single module in a package.
#[allow(clippy::too_many_arguments)]
async fn compile_package_module(
    module_name: &str,
    package: &ExtractedPackage,
    lib_config: &LibraryConfig,
    config: &PackageBuildConfig,
    package_id: &str,
    hi_dir: &Path,
    o_dir: &Path,
    extra_src_dirs: &[PathBuf],
    macros_file: Option<&Path>,
) -> ModuleCompileResult {
    debug!("Compiling module: {} in {}", module_name, package.name);

    // Convert module name to path
    let module_path = module_name.replace('.', "/");
    let source_file = find_module_source(package, lib_config, &module_path, extra_src_dirs);

    let Some(source_file) = source_file else {
        return ModuleCompileResult {
            success: false,
            object_file: None,
            warnings: vec![],
            errors: vec![format!("Cannot find source for module: {}", module_name)],
        };
    };

    // Build GHC args. `-this-unit-id` stamps the package's interface files with
    // its real unit id; without it GHC stamps them as the default `main`
    // package, and any module importing the registered package then fails with
    // "requested module <pkg>:M differs from name found in the interface file
    // main:M".
    let mut args: Vec<String> = vec![
        "-c".to_string(), // Compile only
        "-this-unit-id".to_string(),
        package_id.to_string(),
        format!("-hidir={}", hi_dir.display()),
        format!("-odir={}", o_dir.display()),
    ];

    // Add source directories
    let src_dirs = if lib_config.hs_source_dirs.is_empty() {
        vec![".".to_string()]
    } else {
        lib_config.hs_source_dirs.clone()
    };

    for src_dir in &src_dirs {
        let full_path = package.source_dir.join(src_dir);
        args.push(format!("-i{}", full_path.display()));
    }

    // Add extra source directories (e.g., generated preprocessor output)
    for extra_dir in extra_src_dirs {
        args.push(format!("-i{}", extra_dir.display()));
    }

    // Add package databases
    for db in &config.ghc.package_dbs {
        args.push(format!("-package-db={}", db.display()));
    }

    // Add dependency packages (built from source) by id.
    for dep_id in &config.dependency_ids {
        args.push("-package-id".to_string());
        args.push(dep_id.clone());
    }

    // Expose pre-installed boot packages by name (GHC hides them by default).
    for pkg in &config.expose_packages {
        args.push("-package".to_string());
        args.push(pkg.clone());
    }

    // Add extensions. Only `default-extensions` are enabled for the whole
    // component; `other-extensions` merely *declares* extensions that individual
    // modules turn on via their own `{-# LANGUAGE #-}` pragmas, so enabling them
    // globally is wrong — e.g. a declared `Safe` would force Safe Haskell on
    // every module and break those importing unsafe internals (os-string).
    for ext in &lib_config.default_extensions {
        args.push(format!("-X{}", ext));
    }

    // Add default language
    if let Some(lang) = &lib_config.default_language {
        args.push(format!("-X{}", lang));
    }

    // Add include directories
    for inc_dir in &lib_config.include_dirs {
        let full_path = package.source_dir.join(inc_dir);
        args.push(format!("-I{}", full_path.display()));
    }

    // Include the generated cabal_macros.h ahead of every source so version
    // CPP (`#if MIN_VERSION_foo(...)`) resolves. Passed through to the C
    // preprocessor via `-optP`, exactly as cabal does.
    if let Some(macros) = macros_file {
        args.push("-optP-include".to_string());
        args.push(format!("-optP{}", macros.display()));
    }

    // Add CPP options
    args.extend(lib_config.cpp_options.clone());

    // Add GHC options from cabal file, but never treat warnings as errors when
    // building a dependency from source. Packages enable `-Werror` (often behind
    // a flag our flat parser does not evaluate); a dependency's warnings are not
    // ours to fix, and cabal likewise does not `-Werror` dependencies. Dropping
    // it keeps otherwise-fine packages (e.g. dlist) from failing to build.
    args.extend(
        lib_config
            .ghc_options
            .iter()
            .filter(|o| *o != "-Werror" && !o.starts_with("-Werror="))
            .cloned(),
    );

    // Add optimization
    if config.optimization > 0 {
        args.push(format!("-O{}", config.optimization));
    }

    // Add source file
    args.push(source_file.display().to_string());

    // Run GHC
    let runner = CommandRunner::new().with_working_dir(&package.source_dir);
    let ghc_path = config.ghc.ghc_path.display().to_string();

    let output = match runner.run(&ghc_path, &args).await {
        Ok(o) => o,
        Err(e) => {
            return ModuleCompileResult {
                success: false,
                object_file: None,
                warnings: vec![],
                errors: vec![format!("Failed to run GHC: {}", e)],
            };
        }
    };

    // Parse output
    let (warnings, errors) = parse_ghc_output(&output.stdout, &output.stderr);

    let success = output.success();
    let object_file = if success {
        Some(o_dir.join(format!("{}.o", module_path)))
    } else {
        None
    };

    ModuleCompileResult {
        success,
        object_file,
        warnings,
        errors,
    }
}

/// Find the source file for a module.
fn find_module_source(
    package: &ExtractedPackage,
    lib_config: &LibraryConfig,
    module_path: &str,
    extra_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let src_dirs = if lib_config.hs_source_dirs.is_empty() {
        vec![".".to_string()]
    } else {
        lib_config.hs_source_dirs.clone()
    };

    let extensions = ["hs", "lhs"];

    // Search in package source directories
    for src_dir in &src_dirs {
        for ext in &extensions {
            let path = package
                .source_dir
                .join(src_dir)
                .join(format!("{}.{}", module_path, ext));
            if path.exists() {
                return Some(path);
            }
        }
    }

    // Search in extra directories (e.g., generated preprocessor output)
    for extra_dir in extra_dirs {
        for ext in &extensions {
            let path = extra_dir.join(format!("{}.{}", module_path, ext));
            if path.exists() {
                return Some(path);
            }
        }
    }

    None
}

/// Parse GHC output into warnings and errors.
fn parse_ghc_output(stdout: &str, stderr: &str) -> (Vec<String>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    let combined = format!("{}\n{}", stdout, stderr);

    for line in combined.lines() {
        let trimmed = line.trim();
        if trimmed.contains(": error:") || trimmed.contains(": Error:") {
            errors.push(line.to_string());
        } else if trimmed.contains(": warning:") || trimmed.contains(": Warning:") {
            warnings.push(line.to_string());
        }
    }

    (warnings, errors)
}

/// Create a static library from object files.
async fn create_library(lib_path: &Path, object_files: &[PathBuf], verbose: bool) -> Result<()> {
    if object_files.is_empty() {
        return Ok(());
    }

    // Use libtool on macOS, ar on Linux
    let (archiver, args) = if cfg!(target_os = "macos") {
        (
            "libtool",
            vec![
                "-static".to_string(),
                "-o".to_string(),
                lib_path.display().to_string(),
            ],
        )
    } else {
        (
            "ar",
            vec!["rcs".to_string(), lib_path.display().to_string()],
        )
    };

    let mut full_args = args;
    for obj in object_files {
        full_args.push(obj.display().to_string());
    }

    if verbose {
        info!("Archive command: {} {}", archiver, full_args.join(" "));
    }

    let runner = CommandRunner::new();
    let output = runner
        .run(archiver, full_args.iter().map(|s| s.as_str()))
        .await?;

    if !output.success() {
        return Err(Error::BuildFailed {
            errors: vec!["Failed to create library archive".to_string()],
            fixes: vec![],
        });
    }

    Ok(())
}

/// Copy interface files to install directory.
fn copy_interface_files(from: &Path, to: &Path) -> Result<()> {
    copy_dir_recursive(from, to).map_err(|e| Error::Io {
        message: "Failed to copy interface files".to_string(),
        path: Some(from.to_path_buf()),
        source: e,
    })?;
    Ok(())
}

/// Recursively copy a directory.
fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;

    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest = to.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }

    Ok(())
}

/// Generate a package registration file for ghc-pkg.
#[allow(clippy::too_many_arguments)]
fn generate_registration_file(
    path: &Path,
    name: &str,
    version: &str,
    package_id: &str,
    lib_config: &LibraryConfig,
    lib_dir: &Path,
    import_dir: &Path,
    dependency_ids: &[String],
) -> Result<()> {
    let hs_lib = format!("HS{}-{}", name, version);

    let mut content = String::new();

    content.push_str(&format!("name: {}\n", name));
    content.push_str(&format!("version: {}\n", version));
    content.push_str(&format!("id: {}\n", package_id));
    content.push_str(&format!("key: {}\n", package_id));

    // Exposed modules
    if !lib_config.exposed_modules.is_empty() {
        content.push_str(&format!(
            "exposed-modules: {}\n",
            lib_config.exposed_modules.join(" ")
        ));
    }

    // Hidden modules
    if !lib_config.other_modules.is_empty() {
        content.push_str(&format!(
            "hidden-modules: {}\n",
            lib_config.other_modules.join(" ")
        ));
    }

    content.push_str("exposed: True\n");

    // Library directories and libraries
    content.push_str(&format!("library-dirs: {}\n", lib_dir.display()));
    content.push_str(&format!("hs-libraries: {}\n", hs_lib));

    // Import directories
    content.push_str(&format!("import-dirs: {}\n", import_dir.display()));

    // Dependencies
    if !dependency_ids.is_empty() {
        content.push_str(&format!("depends: {}\n", dependency_ids.join(" ")));
    }

    // Extra libraries
    if !lib_config.extra_libraries.is_empty() {
        content.push_str(&format!(
            "extra-libraries: {}\n",
            lib_config.extra_libraries.join(" ")
        ));
    }

    // Extra library dirs
    if !lib_config.extra_lib_dirs.is_empty() {
        content.push_str(&format!(
            "extra-lib-dirs: {}\n",
            lib_config.extra_lib_dirs.join(" ")
        ));
    }

    // Include dirs
    if !lib_config.include_dirs.is_empty() {
        content.push_str(&format!(
            "include-dirs: {}\n",
            lib_config.include_dirs.join(" ")
        ));
    }

    std::fs::write(path, content).map_err(|e| Error::Io {
        message: "Failed to write package registration file".to_string(),
        path: Some(path.to_path_buf()),
        source: e,
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_package_id() {
        let id = compute_package_id("text", "2.1.1", "9.8.2", &["base-4.18.0".to_string()]);
        assert!(id.starts_with("text-2.1.1-"));
        assert_eq!(id.len(), "text-2.1.1-".len() + 12);
    }

    #[test]
    fn test_version_triple() {
        assert_eq!(version_triple("4.19.1.0"), (4, 19, 1));
        assert_eq!(version_triple("0.1"), (0, 1, 0));
        assert_eq!(version_triple("2"), (2, 0, 0));
        assert_eq!(version_triple(""), (0, 0, 0));
    }

    #[test]
    fn test_sanitize_macro_name() {
        assert_eq!(sanitize_macro_name("bytestring"), "bytestring");
        assert_eq!(sanitize_macro_name("vector-stream"), "vector_stream");
    }

    #[test]
    fn test_generate_cabal_macros() {
        let macros = generate_cabal_macros(
            &[("base".to_string(), "4.19.1.0".to_string())],
            "split",
            "0.2.5",
        );
        // Matches Cabal's cabal_macros.h format byte-for-byte for the macro body.
        assert!(macros.contains("#define VERSION_base \"4.19.1.0\""));
        assert!(macros.contains("#define MIN_VERSION_base(major1,major2,minor) (\\"));
        assert!(macros.contains("  (major1) <  4 || \\"));
        assert!(macros.contains("  (major1) == 4 && (major2) <  19 || \\"));
        assert!(macros.contains("  (major1) == 4 && (major2) == 19 && (minor) <= 1)"));
        assert!(macros.contains("#define CURRENT_PACKAGE_VERSION \"0.2.5\""));
    }

    #[test]
    fn test_generate_cabal_macros_dedupes() {
        let macros = generate_cabal_macros(
            &[
                ("base".to_string(), "4.19.1.0".to_string()),
                ("base".to_string(), "4.19.1.0".to_string()),
            ],
            "p",
            "1.0",
        );
        assert_eq!(macros.matches("#define VERSION_base").count(), 1);
    }

    #[test]
    fn test_package_id_stable() {
        let id1 = compute_package_id("text", "2.1.1", "9.8.2", &["base-4.18.0".to_string()]);
        let id2 = compute_package_id("text", "2.1.1", "9.8.2", &["base-4.18.0".to_string()]);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_package_id_changes_with_deps() {
        let id1 = compute_package_id("text", "2.1.1", "9.8.2", &["base-4.18.0".to_string()]);
        let id2 = compute_package_id(
            "text",
            "2.1.1",
            "9.8.2",
            &["base-4.18.0".to_string(), "bytestring-0.11.5".to_string()],
        );
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_collect_modules() {
        let lib = LibraryConfig {
            exposed_modules: vec!["Data.Text".to_string(), "Data.Text.Lazy".to_string()],
            other_modules: vec!["Data.Text.Internal".to_string()],
            ..Default::default()
        };

        let modules = collect_modules(&lib);
        assert_eq!(modules.len(), 3);
        assert!(modules.contains(&"Data.Text".to_string()));
        assert!(modules.contains(&"Data.Text.Internal".to_string()));
    }
}
