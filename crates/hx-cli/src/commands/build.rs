//! Build command implementation.

use anyhow::Result;
use hx_cabal::build::{self as cabal_build, BuildOptions};
use hx_cabal::native::{
    GhcConfig, NativeBuildOptions, NativeBuilder, compute_flags_hash, native_build_up_to_date,
    packages_from_lockfile,
};
use hx_cabal::{FullNativeBuildOptions, FullNativeBuilder, pre_installed_packages};
use hx_cache::{BuildState, StoreIndex, compute_source_fingerprint, save_source_fingerprint};
use hx_config::{CompilerBackend, Project, find_project_root};
use hx_lock::Lockfile;
use hx_plugins::HookEvent;
use hx_solver::{
    CabalContext, Dependency, FetchOptions, PlanOptions, compute_deps_fingerprint,
    default_package_cache_dir, fetch_packages, generate_build_plan, load_cached_resolution,
    parse_cabal_ctx,
};
use hx_toolchain::{AutoInstallPolicy, Toolchain, ensure_toolchain};
use hx_ui::Output;
use std::path::PathBuf;
use std::time::Instant;

use crate::plugins::PluginHooks;

/// Run the build command.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    release: bool,
    jobs: Option<usize>,
    target: Option<String>,
    package: Option<String>,
    native: bool,
    backend_override: Option<CompilerBackend>,
    policy: AutoInstallPolicy,
    output: &Output,
) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Determine compiler backend (CLI override > config > default)
    let backend = backend_override.unwrap_or(project.manifest.compiler.backend);

    // If BHC is requested, use the BHC backend
    if backend == CompilerBackend::Bhc {
        return run_bhc_build(&project, release, jobs, target, native, output).await;
    }

    // Check toolchain requirements and get toolchain info
    let mut toolchain = Toolchain::detect().await;
    if let Err(e) = ensure_toolchain(
        &toolchain,
        project.manifest.toolchain.ghc.as_deref(),
        project.manifest.toolchain.cabal.as_deref(),
        policy,
    )
    .await
    {
        output.print_error(&e);
        return Ok(4); // Toolchain error exit code
    }

    // Re-detect toolchain after potential installation to get updated paths
    if !toolchain.ghc.status.is_found() || !toolchain.cabal.status.is_found() {
        toolchain = Toolchain::detect().await;
    }

    // Validate package selection for workspaces
    if let Some(ref pkg_name) = package {
        if project.is_workspace() {
            if project.get_package(pkg_name).is_none() {
                output.error(&format!("Package '{}' not found in workspace", pkg_name));
                output.info(&format!(
                    "Available packages: {}",
                    project.package_names().join(", ")
                ));
                return Ok(1);
            }
        } else {
            output.warn("--package flag ignored (not a workspace project)");
        }
    }

    let build_target = match &package {
        Some(pkg) if project.is_workspace() => pkg.as_str(),
        _ => project.name(),
    };

    // Use native build if requested.
    if native {
        // For a project with external dependencies, build them from source
        // (native dependency build). If that is not viable — no lockfile yet, or
        // a dependency that cannot be built natively — fall through to the cabal
        // build so the user always gets a correct result.
        if project_has_external_deps(&project) {
            match run_full_native_build(&project, release, jobs, &toolchain, output).await {
                Ok(Some(code)) => return Ok(code),
                Ok(None) => {
                    output.info("Falling back to cabal build");
                }
                Err(e) => {
                    output.error(&format!("Native build failed: {e:#}"));
                    return Ok(5);
                }
            }
        } else {
            return run_native_build(&project, release, jobs, target, &toolchain, output).await;
        }
    }

    // Get GHC version early for plugin context
    let ghc_version = toolchain.ghc.status.version().map(|v| v.to_string());

    // Initialize plugin hooks
    let mut hooks = PluginHooks::from_project(&project, ghc_version.clone());
    if let Some(ref mut h) = hooks {
        if let Err(e) = h.initialize() {
            output.verbose(&format!("Plugin initialization warning: {}", e));
        }
    }

    // Run pre-build hooks
    if let Some(ref mut h) = hooks {
        if !h.run_pre_hook(HookEvent::PreBuild, output) {
            output.error("Pre-build hook failed");
            return Ok(6); // Hook failure exit code
        }
    }

    // Check lockfile for fingerprint
    let (lock_fingerprint, package_count) = get_build_fingerprint(&project);

    // Compute source fingerprint for incremental build detection
    let source_fingerprint = match compute_source_fingerprint(&project.root) {
        Ok(fp) => Some(fp),
        Err(e) => {
            output.verbose(&format!("Could not compute source fingerprint: {}", e));
            None
        }
    };

    // Check if we can skip the build entirely
    if let Some(ref source_fp) = source_fingerprint {
        let build_state = BuildState::load(&project.root).unwrap_or_default();
        if build_state.is_fresh(&source_fp.hash, lock_fingerprint.as_deref()) {
            output.status("Fresh", build_target);
            output.info("No changes detected, skipping build");
            return Ok(0);
        }
    }

    // Check if we have cached dependencies
    if let Some(ref fp) = lock_fingerprint
        && let Ok(store) = StoreIndex::load()
        && store.has_cached_build(fp)
    {
        output.verbose("Dependencies cached, build should be fast");
    }

    if project.is_workspace() {
        output.status(
            "Building",
            &format!(
                "{} ({}/{} packages)",
                build_target,
                if package.is_some() {
                    1
                } else {
                    project.package_count()
                },
                project.package_count()
            ),
        );
    } else {
        output.status("Building", build_target);
    }

    // Collect toolchain bin directories for PATH
    let mut toolchain_bin_dirs = Vec::new();
    if let Some(ghc_path) = toolchain.ghc.status.path() {
        if let Some(parent) = ghc_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }
    if let Some(cabal_path) = toolchain.cabal.status.path() {
        if let Some(parent) = cabal_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }

    let options = BuildOptions {
        release,
        jobs,
        target,
        // A workspace has no package in the root directory, so `cabal build`
        // with no target fails (Cabal-7134). Build every member with the `all`
        // target unless a specific package was requested.
        package: package
            .clone()
            .or_else(|| project.is_workspace().then(|| "all".to_string())),
        verbose: output.is_verbose(),
        fingerprint: lock_fingerprint.clone(),
        ghc_version: ghc_version.clone(),
        package_count: Some(package_count),
        project_name: Some(project.name().to_string()),
        toolchain_bin_dirs,
    };

    let build_dir = project.cabal_build_dir();
    let build_start = Instant::now();

    match cabal_build::build(&project.root, &build_dir, &options, output).await {
        Ok(_result) => {
            let build_duration = build_start.elapsed();

            // Update build state on success
            if let Some(ref source_fp) = source_fingerprint {
                let mut build_state = BuildState::load(&project.root).unwrap_or_default();
                build_state.update_fingerprints(
                    &source_fp.hash,
                    lock_fingerprint.as_deref(),
                    ghc_version.as_deref(),
                );
                build_state.mark_success(
                    build_target,
                    &source_fp.hash,
                    build_duration.as_secs_f64(),
                );
                if let Err(e) = build_state.save(&project.root) {
                    output.verbose(&format!("Could not save build state: {}", e));
                }
                if let Err(e) = save_source_fingerprint(&project.root, source_fp) {
                    output.verbose(&format!("Could not save source fingerprint: {}", e));
                }
            }

            // Run post-build hooks (success)
            if let Some(ref mut h) = hooks {
                h.run_post_build_hook(true, build_duration, vec![], vec![], output);
            }

            Ok(0)
        }
        Err(e) => {
            let build_duration = build_start.elapsed();

            // Update build state on failure
            if source_fingerprint.is_some() {
                let mut build_state = BuildState::load(&project.root).unwrap_or_default();
                build_state.mark_failed(build_target, &e.to_string());
                let _ = build_state.save(&project.root);
            }

            // Run post-build hooks (failure)
            if let Some(ref mut h) = hooks {
                h.run_post_build_hook(false, build_duration, vec![], vec![e.to_string()], output);
            }

            output.print_error(&e);
            Ok(5) // Build error exit code
        }
    }
}

/// Get build fingerprint from lockfile if it exists.
fn get_build_fingerprint(project: &Project) -> (Option<String>, usize) {
    let lockfile_path = project.lockfile_path();
    if !lockfile_path.exists() {
        return (None, 0);
    }

    match Lockfile::from_file(&lockfile_path) {
        Ok(lock) => {
            let fingerprint = lock.plan.hash.clone();
            let package_count = lock.packages.len();
            (fingerprint, package_count)
        }
        Err(_) => (None, 0),
    }
}

/// Run the test command.
pub async fn test(
    pattern: Option<String>,
    package: Option<String>,
    target: Option<String>,
    backend_override: Option<CompilerBackend>,
    policy: AutoInstallPolicy,
    output: &Output,
) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Determine compiler backend (CLI override > config > default)
    let backend = backend_override.unwrap_or(project.manifest.compiler.backend);

    // Use BHC backend for testing
    if backend == CompilerBackend::Bhc {
        return run_bhc_test(&project, pattern, package, target, output).await;
    }

    // Check toolchain requirements
    let mut toolchain = Toolchain::detect().await;
    if let Err(e) = ensure_toolchain(
        &toolchain,
        project.manifest.toolchain.ghc.as_deref(),
        project.manifest.toolchain.cabal.as_deref(),
        policy,
    )
    .await
    {
        output.print_error(&e);
        return Ok(4); // Toolchain error exit code
    }

    // Re-detect toolchain after potential installation to get updated paths
    if !toolchain.ghc.status.is_found() || !toolchain.cabal.status.is_found() {
        toolchain = Toolchain::detect().await;
    }

    // Validate package selection for workspaces
    if let Some(ref pkg_name) = package {
        if project.is_workspace() {
            if project.get_package(pkg_name).is_none() {
                output.error(&format!("Package '{}' not found in workspace", pkg_name));
                output.info(&format!(
                    "Available packages: {}",
                    project.package_names().join(", ")
                ));
                return Ok(1);
            }
        } else {
            output.warn("--package flag ignored (not a workspace project)");
        }
    }

    let test_target = match &package {
        Some(pkg) if project.is_workspace() => pkg.as_str(),
        _ => project.name(),
    };

    // Get GHC version for plugin context
    let ghc_version = toolchain.ghc.status.version().map(|v| v.to_string());

    // Initialize plugin hooks
    let mut hooks = PluginHooks::from_project(&project, ghc_version);
    if let Some(ref mut h) = hooks {
        if let Err(e) = h.initialize() {
            output.verbose(&format!("Plugin initialization warning: {}", e));
        }
    }

    // Run pre-test hooks
    if let Some(ref mut h) = hooks {
        if !h.run_pre_hook(HookEvent::PreTest, output) {
            output.error("Pre-test hook failed");
            return Ok(6); // Hook failure exit code
        }
    }

    output.status("Testing", test_target);

    // Collect toolchain bin directories for PATH
    let mut toolchain_bin_dirs = Vec::new();
    if let Some(ghc_path) = toolchain.ghc.status.path() {
        if let Some(parent) = ghc_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }
    if let Some(cabal_path) = toolchain.cabal.status.path() {
        if let Some(parent) = cabal_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }

    let build_dir = project.cabal_build_dir();

    // A workspace has no package in the root, so an untargeted `cabal test`
    // fails. Default to the `all` target (every test suite) unless a specific
    // package was requested.
    let test_package = package
        .clone()
        .or_else(|| project.is_workspace().then(|| "all".to_string()));

    match cabal_build::test(
        &project.root,
        &build_dir,
        pattern.as_deref(),
        test_package.as_deref(),
        target.as_deref(),
        &toolchain_bin_dirs,
        output,
    )
    .await
    {
        Ok(_result) => {
            // Run post-test hooks (success)
            // Note: We don't have detailed test counts from cabal output yet
            if let Some(ref mut h) = hooks {
                h.run_post_test_hook(true, 0, 0, 0, output);
            }
            Ok(0)
        }
        Err(e) => {
            // Run post-test hooks (failure)
            if let Some(ref mut h) = hooks {
                h.run_post_test_hook(false, 0, 0, 0, output);
            }
            output.print_error(&e);
            Ok(5) // Test error exit code
        }
    }
}

/// Run native GHC build (experimental).
async fn run_native_build(
    project: &Project,
    release: bool,
    jobs: Option<usize>,
    target: Option<String>,
    toolchain: &Toolchain,
    output: &Output,
) -> Result<i32> {
    output.warn("Native build is experimental - use --native only for testing");

    // Get GHC version
    let ghc_version = toolchain
        .ghc
        .status
        .version()
        .map(|v| v.to_string())
        .unwrap_or_default();

    // Get GHC path from detected toolchain
    let ghc_path = toolchain
        .ghc
        .status
        .path()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("ghc"));

    // Build the native options first — they don't depend on toolchain
    // resolution, so we can use them for a cheap up-to-date check.
    let build_config = &project.manifest.build;

    // Determine optimization level
    let optimization = if release {
        2
    } else {
        build_config.optimization
    };

    // Get source directories from config
    let src_dirs: Vec<PathBuf> = build_config.src_dirs.iter().map(PathBuf::from).collect();

    // Combine extra flags from config
    let mut extra_flags = build_config.ghc_flags.clone();

    // Add any language extensions commonly needed
    // These can be overridden by the user's ghc_flags
    if extra_flags.is_empty() {
        extra_flags.push("-XOverloadedStrings".to_string());
    }

    // Configure native build options
    let native_options = NativeBuildOptions {
        src_dirs,
        output_dir: project.root.join(".hx/native-build"),
        optimization,
        warnings: build_config.warnings,
        werror: build_config.werror,
        extra_flags,
        jobs: jobs.unwrap_or_else(num_cpus::get),
        verbose: output.is_verbose(),
        main_module: Some("Main".to_string()),
        output_exe: Some(project.root.join(".hx/native-build").join(project.name())),
        output_lib: None,     // Only set for library projects
        native_linking: true, // Enable native linking with resolved packages
        target,               // Cross-compilation target
    };

    // Fast path: if nothing has changed, skip toolchain resolution entirely.
    // Detecting GHC and querying ghc-pkg spawns several subprocesses that
    // dominate the cost of an otherwise no-op build.
    let abs_src_dirs: Vec<PathBuf> = native_options
        .src_dirs
        .iter()
        .map(|d| project.root.join(d))
        .filter(|p| p.exists())
        .collect();
    let output_dir = project.root.join(&native_options.output_dir);
    let flags_hash = compute_flags_hash(&native_options);
    if native_build_up_to_date(
        &project.root,
        &abs_src_dirs,
        &output_dir,
        &ghc_version,
        &flags_hash,
    ) {
        output.status("Building", &format!("{} (native)", project.name()));
        output.status("Finished", "up to date");
        return Ok(0);
    }

    // Something changed — resolve the toolchain and package databases.
    let mut ghc_config = match GhcConfig::detect_with_path(&ghc_path).await {
        Ok(config) => config,
        Err(_) => GhcConfig {
            ghc_path,
            version: ghc_version.clone(),
            package_dbs: vec![],
            packages: vec![],
            resolved_packages: vec![],
            rts_include_dirs: vec![],
        },
    };

    // Get packages from lockfile if it exists
    let lockfile_path = project.lockfile_path();
    let packages = packages_from_lockfile(&lockfile_path);
    ghc_config = ghc_config.with_packages(packages);

    // Create builder and run
    let builder = NativeBuilder::new(ghc_config);

    // Show build info
    output.status("Building", &format!("{} (native)", project.name()));
    if output.is_verbose() {
        output.info(&format!("  GHC version: {}", ghc_version));
        output.info(&format!("  Optimization: -O{}", optimization));
        output.info(&format!("  Parallelism: {} jobs", native_options.jobs));
    }

    match builder.build(&project.root, &native_options, output).await {
        Ok(result) => {
            if result.success {
                // Show detailed results
                let duration_str = format_build_duration(result.duration);

                if result.modules_compiled > 0 || result.modules_skipped > 0 {
                    let status = if result.modules_skipped > 0 {
                        format!(
                            "{} compiled, {} up-to-date in {}",
                            result.modules_compiled, result.modules_skipped, duration_str
                        )
                    } else {
                        format!("{} modules in {}", result.modules_compiled, duration_str)
                    };
                    output.status("Finished", &status);
                }

                // Show warnings if any
                if !result.warnings.is_empty() {
                    output.warn(&format!("{} warning(s)", result.warnings.len()));
                    if output.is_verbose() {
                        for warning in &result.warnings {
                            output.info(warning);
                        }
                    }
                }

                if let Some(exe) = result.executable {
                    output.info(&format!("Executable: {}", exe.display()));
                }
                Ok(0)
            } else {
                output.error(&format!(
                    "Build failed with {} error(s)",
                    result.errors.len()
                ));
                for error in &result.errors {
                    eprintln!("{}", error);
                }
                Ok(5)
            }
        }
        Err(e) => {
            output.print_error(&e);
            Ok(5)
        }
    }
}

/// Build the `.cabal` evaluation context (target GHC + host) the same way the
/// lock command does, so the dependency fingerprint matches the cached plan.
fn build_cabal_context(project: &Project, toolchain: &Toolchain) -> CabalContext {
    let ghc = project
        .manifest
        .toolchain
        .ghc
        .as_deref()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            toolchain
                .ghc
                .status
                .version()
                .and_then(|v| v.to_string().parse().ok())
        })
        .unwrap_or_else(|| "9.8.2".parse().expect("valid default GHC version"));
    CabalContext::host(ghc)
}

/// Collect the project's external dependencies (skipping workspace members),
/// matching the lock command's collection so fingerprints line up.
fn collect_project_deps(project: &Project, ctx: &CabalContext) -> Vec<Dependency> {
    let workspace: Vec<String> = project
        .package_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut deps: Vec<Dependency> = Vec::new();
    for cabal_file in project.cabal_files() {
        if let Ok(content) = std::fs::read_to_string(&cabal_file) {
            for dep in parse_cabal_ctx(&content, ctx).all_dependencies() {
                if !workspace.contains(&dep.name) && !deps.iter().any(|d| d.name == dep.name) {
                    deps.push(dep);
                }
            }
        }
    }
    deps
}

/// Whether the project declares any dependency that must be fetched and built
/// (i.e. not a GHC-provided boot package and not a workspace member).
fn project_has_external_deps(project: &Project) -> bool {
    let ctx = CabalContext::host("9.8.2".parse().expect("valid default GHC version"));
    collect_project_deps(project, &ctx)
        .iter()
        .any(|d| !pre_installed_packages().contains(d.name.as_str()))
}

/// Build a project and its external dependencies natively from source, reusing
/// the lockfile's resolution.
///
/// Returns `Ok(Some(code))` when the native build handled the request, or
/// `Ok(None)` when it is not viable (no cached resolution, or a dependency that
/// cannot be built natively) and the caller should fall back to cabal.
async fn run_full_native_build(
    project: &Project,
    release: bool,
    jobs: Option<usize>,
    toolchain: &Toolchain,
    output: &Output,
) -> Result<Option<i32>> {
    output.warn("Native dependency builds are experimental");

    // Stackage-snapshot projects resolve differently; defer to cabal for now.
    if project.manifest.stackage.snapshot.is_some() {
        return Ok(None);
    }

    let ctx = build_cabal_context(project, toolchain);
    let all_deps = collect_project_deps(project, &ctx);

    // Reuse the lockfile's cached resolution, keyed by the dependency fingerprint.
    let pairs: Vec<(String, String)> = all_deps
        .iter()
        .map(|d| (d.name.clone(), d.constraint.to_string()))
        .collect();
    let fingerprint = compute_deps_fingerprint(&pairs);
    let plan = match load_cached_resolution(&fingerprint) {
        Ok(p) => p,
        Err(_) => {
            output.info("Native build: no cached resolution — run `hx lock` first");
            return Ok(None);
        }
    };

    // Fetch dependency sources from Hackage.
    let fetched = match fetch_packages(&plan, &FetchOptions::default()).await {
        Ok(f) => f,
        Err(e) => {
            output.warn(&format!("Could not fetch dependencies: {e}"));
            return Ok(None);
        }
    };

    // Topologically ordered build plan.
    let ghc_version = toolchain
        .ghc
        .status
        .version()
        .map(|v| v.to_string())
        .unwrap_or_default();
    let plan_opts = PlanOptions {
        compiler_id: format!("ghc-{ghc_version}"),
        platform: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        pre_installed: pre_installed_packages()
            .into_iter()
            .map(str::to_string)
            .collect(),
        cached_hashes: Default::default(),
    };
    let build_plan = match generate_build_plan(&plan, &plan_opts) {
        Ok(b) => b,
        Err(e) => {
            output.warn(&format!("Could not plan the dependency build: {e}"));
            return Ok(None);
        }
    };

    // Resolve GHC and the package store.
    let ghc_path = toolchain
        .ghc
        .status
        .path()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("ghc"));
    let cache_dir =
        default_package_cache_dir().unwrap_or_else(|| project.root.join(".hx").join("native-store"));
    // Memoised on disk: GHC/ghc-pkg introspection is ~150ms of subprocess time
    // and its result is fixed for a given GHC install.
    let ghc_config =
        GhcConfig::detect_with_path_cached(&ghc_path, &cache_dir, &ghc_version).await;

    let mut builder = FullNativeBuilder::new(ghc_config, cache_dir).await?;
    let opts = FullNativeBuildOptions {
        jobs: jobs.unwrap_or(4),
        optimization: if release {
            2
        } else {
            project.manifest.build.optimization
        },
        verbose: output.is_verbose(),
        force: false,
        profiling: false,
        // Fall back to cabal if any dependency cannot be built natively, rather
        // than silently producing a partial build.
        skip_unsupported: false,
    };

    // The local project's own build options (source dirs, main module, output
    // paths). build_project adds the dependency package flags on top of these.
    let build_config = &project.manifest.build;
    let local_options = NativeBuildOptions {
        src_dirs: build_config.src_dirs.iter().map(PathBuf::from).collect(),
        output_dir: project.root.join(".hx").join("native-build"),
        optimization: opts.optimization,
        warnings: build_config.warnings,
        werror: build_config.werror,
        extra_flags: build_config.ghc_flags.clone(),
        jobs: opts.jobs,
        verbose: output.is_verbose(),
        main_module: Some("Main".to_string()),
        output_exe: Some(project.root.join(".hx").join("native-build").join(project.name())),
        output_lib: None,
        native_linking: true,
        target: None,
    };

    output.status(
        "Building",
        &format!("{} (native, with dependencies)", project.name()),
    );

    let result = builder
        .build_project(&project.root, &build_plan, &fetched, &opts, &local_options, output)
        .await?;

    let project_ok = result
        .project_result
        .as_ref()
        .map(|r| r.success)
        .unwrap_or(false);

    if result.success && project_ok {
        output.status(
            "Finished",
            &format!("native build ({} dependencies built)", result.packages_built),
        );
        Ok(Some(0))
    } else {
        output.info("Native dependency build incomplete; using cabal instead");
        Ok(None)
    }
}

/// Format build duration for display.
fn format_build_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", duration.as_millis())
    } else if secs < 60.0 {
        format!("{:.2}s", secs)
    } else {
        let mins = (secs / 60.0).floor();
        let remaining_secs = secs - (mins * 60.0);
        format!("{}m {:.1}s", mins as u64, remaining_secs)
    }
}

/// Run a build using the BHC backend.
async fn run_bhc_build(
    project: &Project,
    release: bool,
    jobs: Option<usize>,
    target: Option<String>,
    native: bool,
    output: &Output,
) -> Result<i32> {
    if native {
        return run_bhc_native_build(project, release, jobs, target, output).await;
    }

    use hx_bhc::{BhcBackend, generate_bhc_manifest};
    use hx_compiler::{BuildOptions as CompilerBuildOptions, CompilerBackend as Backend};

    output.status("Building", &format!("{} (BHC)", project.name()));

    // Generate bhc.toml from hx.toml
    match generate_bhc_manifest(&project.root, &project.manifest) {
        Ok(path) => {
            output.verbose(&format!("Generated BHC manifest at {}", path.display()));
        }
        Err(e) => {
            output.error(&format!("Failed to generate BHC manifest: {}", e));
            return Ok(3); // Config error
        }
    }

    // Create BHC backend with project configuration
    let bhc_config = &project.manifest.compiler.bhc;
    let backend = BhcBackend::new().with_config(bhc_config.clone());

    // Check if BHC is available
    let status = backend.detect().await;
    match status {
        Ok(hx_compiler::CompilerStatus::Available { version, .. }) => {
            output.verbose(&format!("BHC version: {}", version));
        }
        Ok(hx_compiler::CompilerStatus::NotInstalled) => {
            output.error("BHC is not installed");
            output.info("Install BHC with: hx toolchain install --bhc latest");
            return Ok(4); // Toolchain error
        }
        Ok(hx_compiler::CompilerStatus::VersionMismatch {
            required,
            installed,
        }) => {
            output.warn(&format!(
                "BHC version mismatch: required {}, installed {}",
                required, installed
            ));
        }
        Err(e) => {
            output.error(&format!("Failed to detect BHC: {}", e));
            return Ok(4);
        }
    }

    // Build options
    let build_options = CompilerBuildOptions {
        release,
        optimization: if release {
            Some(2)
        } else {
            Some(project.manifest.build.optimization)
        },
        jobs,
        target,
        package: None,
        verbose: output.is_verbose(),
        extra_flags: project.manifest.build.ghc_flags.clone(),
        src_dirs: project
            .manifest
            .build
            .src_dirs
            .iter()
            .map(PathBuf::from)
            .collect(),
        werror: project.manifest.build.werror,
    };

    let start = std::time::Instant::now();

    match backend.build(&project.root, &build_options, output).await {
        Ok(result) => {
            let duration = start.elapsed();
            if result.success {
                output.status(
                    "Finished",
                    &format!("BHC build in {}", format_build_duration(duration)),
                );

                // Show warnings if any
                if !result.warnings.is_empty() {
                    output.warn(&format!("{} warning(s)", result.warnings.len()));
                    if output.is_verbose() {
                        for warning in &result.warnings {
                            output.info(&warning.to_string());
                        }
                    }
                }

                Ok(0)
            } else {
                output.error(&format!(
                    "Build failed with {} error(s)",
                    result.errors.len()
                ));
                for error in &result.errors {
                    eprintln!("{}", error);
                }
                Ok(5)
            }
        }
        Err(e) => {
            output.error(&format!("BHC build failed: {}", e));
            Ok(5)
        }
    }
}

/// Run a native BHC build (hx owns the build graph).
async fn run_bhc_native_build(
    project: &Project,
    release: bool,
    jobs: Option<usize>,
    target: Option<String>,
    output: &Output,
) -> Result<i32> {
    use hx_bhc::native::{BhcCompilerConfig, BhcNativeBuildOptions};

    output.warn("BHC native build is experimental");
    output.status("Building", &format!("{} (BHC native)", project.name()));

    // Detect BHC
    let bhc = match BhcCompilerConfig::detect().await {
        Ok(config) => config
            .with_profile(project.manifest.compiler.bhc.profile)
            .with_tensor_fusion(project.manifest.compiler.bhc.tensor_fusion)
            .with_emit_kernel_report(project.manifest.compiler.bhc.emit_kernel_report),
        Err(e) => {
            output.error(&format!("BHC not available: {}", e));
            output.info("Install BHC with: hx toolchain install --bhc latest");
            return Ok(4);
        }
    };

    if output.is_verbose() {
        output.info(&format!("  BHC version: {}", bhc.version));
        output.info(&format!("  Profile: {}", bhc.profile.as_str()));
    }

    let build_config = &project.manifest.build;
    let optimization = if release {
        2
    } else {
        build_config.optimization
    };
    let src_dirs: Vec<PathBuf> = build_config.src_dirs.iter().map(PathBuf::from).collect();

    let options = BhcNativeBuildOptions {
        src_dirs,
        output_dir: project.root.join(".hx/bhc-native-build"),
        optimization,
        warnings: build_config.warnings,
        werror: build_config.werror,
        extra_flags: build_config.ghc_flags.clone(),
        jobs: jobs.unwrap_or(4),
        verbose: output.is_verbose(),
        main_module: Some("Main".to_string()),
        output_exe: Some(
            project
                .root
                .join(".hx/bhc-native-build")
                .join(project.name()),
        ),
        output_lib: None,
        target,
        extensions: Vec::new(),
    };

    // For now, invoke BHC directly with native build flags
    let mut args = bhc.bhc_flags();
    args.push("-O".to_string());
    args.push(options.optimization.to_string());
    args.push("--odir".to_string());
    args.push(options.output_dir.to_string_lossy().to_string());

    if options.warnings {
        args.push("--Wall".to_string());
    }
    if options.werror {
        args.push("--Werror".to_string());
    }

    for dir in &options.src_dirs {
        args.push("--import-path".to_string());
        args.push(dir.to_string_lossy().to_string());
    }

    for ext in &options.extensions {
        args.push(format!("-X{}", ext));
    }

    args.extend(options.extra_flags.iter().cloned());

    if let Some(ref main_module) = options.main_module {
        args.push(main_module.clone());
    }

    if let Some(ref exe_path) = options.output_exe {
        args.push("-o".to_string());
        args.push(exe_path.to_string_lossy().to_string());
        // Ensure output directory exists
        if let Some(parent) = exe_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    if options.verbose {
        args.push("-v".to_string());
    }

    let start = Instant::now();

    let cmd_output = std::process::Command::new(&bhc.bhc_path)
        .args(&args)
        .current_dir(&project.root)
        .output()?;

    let duration = start.elapsed();
    let success = cmd_output.status.success();

    let stdout = String::from_utf8_lossy(&cmd_output.stdout);
    let stderr = String::from_utf8_lossy(&cmd_output.stderr);

    if output.is_verbose() || !success {
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
        let duration_str = format!("{:.2}s", duration.as_secs_f64());
        output.status("Finished", &format!("BHC native build in {}", duration_str));
        if let Some(exe) = options.output_exe {
            output.info(&format!("Executable: {}", exe.display()));
        }
        Ok(0)
    } else {
        output.error("BHC native build failed");
        Ok(5)
    }
}

/// Run tests using the BHC backend.
async fn run_bhc_test(
    project: &Project,
    pattern: Option<String>,
    package: Option<String>,
    target: Option<String>,
    output: &Output,
) -> Result<i32> {
    use hx_bhc::{BhcBackend, generate_bhc_manifest};
    use hx_compiler::CompilerBackend as Backend;

    output.status("Testing", &format!("{} (BHC)", project.name()));

    // Generate bhc.toml from hx.toml
    match generate_bhc_manifest(&project.root, &project.manifest) {
        Ok(path) => {
            output.verbose(&format!("Generated BHC manifest at {}", path.display()));
        }
        Err(e) => {
            output.error(&format!("Failed to generate BHC manifest: {}", e));
            return Ok(3);
        }
    }

    // Create BHC backend with project configuration
    let bhc_config = &project.manifest.compiler.bhc;
    let backend = BhcBackend::new().with_config(bhc_config.clone());

    // Check if BHC is available
    let status = backend.detect().await;
    match status {
        Ok(hx_compiler::CompilerStatus::Available { version, .. }) => {
            output.verbose(&format!("BHC version: {}", version));
        }
        Ok(hx_compiler::CompilerStatus::NotInstalled) => {
            output.error("BHC is not installed");
            output.info("Install BHC with: hx toolchain install --bhc latest");
            return Ok(4);
        }
        Ok(hx_compiler::CompilerStatus::VersionMismatch {
            required,
            installed,
        }) => {
            output.warn(&format!(
                "BHC version mismatch: required {}, installed {}",
                required, installed
            ));
        }
        Err(e) => {
            output.error(&format!("Failed to detect BHC: {}", e));
            return Ok(4);
        }
    }

    // Build test arguments using the backend's test_args helper
    let bhc_cmd = backend.bhc_cmd();
    let args = backend.test_args(pattern.as_deref(), package.as_deref(), target.as_deref());

    let start = std::time::Instant::now();

    let cmd_output = std::process::Command::new(&bhc_cmd)
        .args(&args)
        .current_dir(&project.root)
        .output()?;

    let duration = start.elapsed();
    let success = cmd_output.status.success();

    let stdout = String::from_utf8_lossy(&cmd_output.stdout);
    let stderr = String::from_utf8_lossy(&cmd_output.stderr);

    if output.is_verbose() || !success {
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
            &format!("BHC tests in {}", format_build_duration(duration)),
        );
        Ok(0)
    } else {
        output.error("BHC tests failed");
        Ok(5)
    }
}
