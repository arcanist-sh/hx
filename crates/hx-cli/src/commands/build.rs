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

use crate::commands::workspace::{self, Selection};
use crate::plugins::PluginHooks;

/// Workspace member selection flags shared by build, test and check.
#[derive(Debug, Clone, Default)]
pub struct PackageArgs {
    /// `--package` values (empty: decide from `changed` / the current directory).
    pub package: Vec<String>,
    /// `--changed[=BASE]`: select members affected by changes since BASE.
    pub changed: Option<String>,
}

/// Run the build command.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    release: bool,
    jobs: Option<usize>,
    target: Option<String>,
    packages: PackageArgs,
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

    // Decide which workspace members to build (all, `--package`, `--changed`,
    // or the member containing the current directory).
    let selection = match workspace::select(
        &project,
        &packages.package,
        packages.changed.as_deref(),
        output,
    )
    .await
    {
        Ok(Selection::Nothing) => return Ok(0),
        Ok(selection) => selection,
        Err(code) => return Ok(code),
    };

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

    let build_target = project.name();

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

    // Compute source fingerprint for incremental build detection. The
    // fingerprint covers the whole project, so it is only consulted (and
    // recorded) when the whole project is built: a partial workspace build
    // must neither be skipped nor mark unbuilt members as fresh.
    let source_fingerprint = if !selection.is_everything() {
        None
    } else {
        match compute_source_fingerprint(&project.root) {
            Ok(fp) => Some(fp),
            Err(e) => {
                output.verbose(&format!("Could not compute source fingerprint: {}", e));
                None
            }
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

    output.status("Building", &selection.label(&project));

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
        // target unless specific members were selected.
        targets: selection.targets(),
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
    packages: PackageArgs,
    target: Option<String>,
    _native: bool,
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
    // BHC has no `test` subcommand; hx builds and runs each test suite. A BHC
    // compile is always native, so `--native` does not change the BHC path.
    if backend == CompilerBackend::Bhc {
        let package = packages.package.first().cloned();
        return run_bhc_test(&project, pattern, package, target, output).await;
    }

    // Decide which members to test, then drop those without a test suite:
    // `cabal test <pkg>` fails for a package that has none.
    let selection = match workspace::select(
        &project,
        &packages.package,
        packages.changed.as_deref(),
        output,
    )
    .await
    {
        Ok(selection) => workspace::members_with_tests(&project, selection, output),
        Err(code) => return Ok(code),
    };
    if selection == Selection::Nothing {
        if packages.changed.is_none() {
            output.info("No test suites in the selected workspace members");
        }
        return Ok(0);
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

    output.status("Testing", &selection.label(&project));

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
    // fails. Default to the `all` target (every test suite) unless specific
    // members were selected.
    let test_targets = selection.targets();

    match cabal_build::test(
        &project.root,
        &build_dir,
        pattern.as_deref(),
        &test_targets,
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
            boot_packages: vec![],
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
    let cache_dir = default_package_cache_dir()
        .unwrap_or_else(|| project.root.join(".hx").join("native-store"));
    // Memoised on disk: GHC/ghc-pkg introspection is ~150ms of subprocess time
    // and its result is fixed for a given GHC install.
    let ghc_config = GhcConfig::detect_with_path_cached(&ghc_path, &cache_dir, &ghc_version).await;

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
        output_exe: Some(
            project
                .root
                .join(".hx")
                .join("native-build")
                .join(project.name()),
        ),
        output_lib: None,
        native_linking: true,
        target: None,
    };

    output.status(
        "Building",
        &format!("{} (native, with dependencies)", project.name()),
    );

    let result = builder
        .build_project(
            &project.root,
            &build_plan,
            &fetched,
            &opts,
            &local_options,
            output,
        )
        .await?;

    let project_ok = result
        .project_result
        .as_ref()
        .map(|r| r.success)
        .unwrap_or(false);

    if result.success && project_ok {
        output.status(
            "Finished",
            &format!(
                "native build ({} dependencies built)",
                result.packages_built
            ),
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

/// Attempt a full BHC native build that compiles transitive dependencies from
/// source into a package database, then builds the local project against it.
///
/// Returns `Ok(Some(exit_code))` when the dependency-aware build ran (whether it
/// succeeded or failed), or `Ok(None)` when it is not applicable — no external
/// dependencies, or no cached resolution (the caller then does a local-only
/// build). The dependency path requires `hx lock` to have produced a cached
/// resolution and network access to fetch sources from Hackage.
async fn try_bhc_full_native_build(
    project: &Project,
    bhc: &hx_bhc::native::BhcCompilerConfig,
    release: bool,
    jobs: Option<usize>,
    output: &Output,
) -> Result<Option<i32>> {
    use hx_bhc::full_native::BhcFullNativeBuilder;
    use hx_bhc::native::BhcNativeBuildOptions;
    use hx_bhc::package_db::BhcPackageDb;
    use std::collections::HashSet;

    // Stackage-snapshot projects resolve differently; defer for now.
    if project.manifest.stackage.snapshot.is_some() {
        return Ok(None);
    }

    // Cabal conditionals resolve against a GHC version (BHC targets GHC
    // compatibility); take it from the manifest, falling back to a default. No
    // GHC toolchain installation is required.
    let ghc_str = project
        .manifest
        .toolchain
        .ghc
        .clone()
        .unwrap_or_else(|| "9.8.2".to_string());
    let ghc = ghc_str
        .parse()
        .unwrap_or_else(|_| "9.8.2".parse().expect("valid default GHC version"));
    let ctx = CabalContext::host(ghc);

    let all_deps = collect_project_deps(project, &ctx);
    if all_deps.is_empty() {
        // No external dependencies — the local-only build is sufficient.
        return Ok(None);
    }

    // Reuse the lockfile's cached resolution, keyed by the dependency fingerprint.
    let pairs: Vec<(String, String)> = all_deps
        .iter()
        .map(|d| (d.name.clone(), d.constraint.to_string()))
        .collect();
    let fingerprint = compute_deps_fingerprint(&pairs);
    let plan = match load_cached_resolution(&fingerprint) {
        Ok(p) => p,
        Err(_) => {
            output.info("BHC native build: no cached resolution — run `hx lock` first");
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

    // Topologically ordered build plan. BHC's builtin packages are treated as
    // pre-installed so they are neither fetched nor built from source.
    let pre_installed: HashSet<String> = hx_bhc::builtin_packages::builtin_packages()
        .keys()
        .map(|s| s.to_string())
        .collect();
    let plan_opts = PlanOptions {
        compiler_id: format!("ghc-{ghc_str}"),
        platform: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        pre_installed,
        cached_hashes: Default::default(),
    };
    let build_plan = match generate_build_plan(&plan, &plan_opts) {
        Ok(b) => b,
        Err(e) => {
            output.warn(&format!("Could not plan the dependency build: {e}"));
            return Ok(None);
        }
    };

    let cache_dir = default_package_cache_dir()
        .unwrap_or_else(|| project.root.join(".hx").join("native-store"));
    let package_db = BhcPackageDb::open(&bhc.version, &cache_dir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to open BHC package DB: {e}"))?;

    let build_config = &project.manifest.build;
    let optimization = if release {
        2
    } else {
        build_config.optimization
    };
    let options = BhcNativeBuildOptions {
        src_dirs: build_config.src_dirs.iter().map(PathBuf::from).collect(),
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
        target: None,
        extensions: Vec::new(),
    };

    let mut builder = BhcFullNativeBuilder::new(bhc.clone(), package_db, cache_dir);
    let result = builder
        .build_project(&project.root, &build_plan, &fetched, &options, output)
        .await
        .map_err(|e| anyhow::anyhow!("BHC native build failed: {e}"))?;

    if result.success {
        output.status(
            "Finished",
            &format!(
                "BHC native build ({} built, {} skipped)",
                result.packages_built, result.packages_skipped
            ),
        );
        Ok(Some(0))
    } else {
        for e in &result.errors {
            eprintln!("{e}");
        }
        Ok(Some(5))
    }
}

/// Resolve a main module name to the source file BHC should compile.
///
/// BHC compiles a file, not a module name, so `Main` becomes `<src>/Main.hs`
/// (and `Foo.Bar` becomes `<src>/Foo/Bar.hs`). Returns the first candidate that
/// exists under `src_dirs` relative to `project_root`, or the bare relative path
/// (`Main.hs`) when none match — bhc then reports a clear missing-file error.
fn resolve_main_source(
    main_module: &str,
    src_dirs: &[PathBuf],
    project_root: &std::path::Path,
) -> String {
    let rel = format!("{}.hs", main_module.replace('.', "/"));
    src_dirs
        .iter()
        .map(|dir| dir.join(&rel))
        .find(|candidate| project_root.join(candidate).exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(rel)
}

/// Run a native BHC build (hx owns the build graph).
pub(crate) async fn run_bhc_native_build(
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

    // If the project has external dependencies and a cached resolution, build
    // them from source into a package database and link the project against it.
    match try_bhc_full_native_build(project, &bhc, release, jobs, output).await {
        Ok(Some(code)) => return Ok(code),
        Ok(None) => { /* no deps / no lockfile — fall through to local-only build */ }
        Err(e) => {
            output.warn(&format!("Native dependency build failed: {e}"));
            output.info("Falling back to a local-only BHC build");
        }
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
        // BHC compiles a source file, not a module name.
        args.push(resolve_main_source(
            main_module,
            &options.src_dirs,
            &project.root,
        ));
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

/// A package whose test suites a BHC test run covers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BhcTestPackage {
    /// Package name.
    name: String,
    /// Package root directory.
    root: PathBuf,
    /// The package's `.cabal` file, if it has one.
    cabal_file: Option<PathBuf>,
}

/// Which packages a BHC test run covers, and which suite (if any) it is
/// narrowed to.
///
/// `--package` names a workspace member, the project itself, or a single test
/// suite. A name that is neither a member nor the project is treated as a
/// suite name; it is validated once the suites are discovered.
fn select_bhc_test_packages(
    project: &Project,
    package: Option<&str>,
) -> (Vec<BhcTestPackage>, Option<String>) {
    if project.is_workspace() {
        let member = |p: &hx_config::WorkspacePackage| BhcTestPackage {
            name: p.name.clone(),
            root: project.root.join(&p.path),
            cabal_file: Some(project.root.join(&p.cabal_file)),
        };
        if let Some(pkg) = package.and_then(|name| project.get_package(name)) {
            return (vec![member(pkg)], None);
        }
        let all = project.workspace_packages.iter().map(member).collect();
        return (all, package.map(str::to_string));
    }

    let this = BhcTestPackage {
        name: project.name().to_string(),
        root: project.root.clone(),
        cabal_file: project.cabal_file.clone(),
    };
    let suite_filter = package
        .filter(|name| *name != project.name())
        .map(str::to_string);
    (vec![this], suite_filter)
}

/// Run tests using the BHC backend.
///
/// BHC has no `test` subcommand, so hx drives the run: it discovers the
/// package's `exitcode-stdio-1.0` test suites (or a conventional `test/Main.hs`),
/// compiles each suite's main module with BHC exactly like `hx build` does, then
/// runs the executable. A suite passes when it exits 0.
async fn run_bhc_test(
    project: &Project,
    pattern: Option<String>,
    package: Option<String>,
    target: Option<String>,
    output: &Output,
) -> Result<i32> {
    use hx_bhc::{BhcBackend, discover_test_suites, generate_bhc_manifest};
    use hx_compiler::{BuildOptions as CompilerBuildOptions, CompilerBackend as Backend};
    use hx_core::Fix;

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

    // Discover the test suites to run.
    let (packages, suite_filter) = select_bhc_test_packages(project, package.as_deref());
    let mut suites = Vec::new();
    for pkg in &packages {
        let found = match discover_test_suites(
            &pkg.root,
            &pkg.name,
            pkg.cabal_file.as_deref(),
            &project.manifest.build.src_dirs,
        ) {
            Ok(found) => found,
            Err(e) => {
                let cabal = pkg.cabal_file.as_deref().unwrap_or(&pkg.root);
                output.error(&format!("Failed to read {}: {}", cabal.display(), e));
                return Ok(3);
            }
        };
        let wanted = |name: &str| suite_filter.as_deref().is_none_or(|f| f == name);
        for skipped in found.skipped.iter().filter(|s| wanted(&s.name)) {
            output.warn(&format!(
                "skipping test suite `{}` ({}): {}",
                skipped.name, pkg.name, skipped.reason
            ));
        }
        suites.extend(found.suites.into_iter().map(|s| (pkg.clone(), s)));
    }

    if let Some(ref name) = suite_filter {
        let available: Vec<String> = suites.iter().map(|(_, s)| s.name.clone()).collect();
        suites.retain(|(_, s)| &s.name == name);
        if suites.is_empty() {
            output.error(&format!("No package or test suite named '{}'", name));
            let mut names: Vec<String> = project
                .package_names()
                .iter()
                .map(|n| n.to_string())
                .collect();
            names.extend(available);
            output.info(&format!("Available: {}", names.join(", ")));
            return Ok(1);
        }
    }

    if suites.is_empty() {
        output.error("No test suites found for BHC");
        output.info(
            "  hx looks for `test-suite` stanzas (type: exitcode-stdio-1.0) in the .cabal file, \
             then for test/Main.hs, test/Spec.hs, tests/Main.hs or tests/Spec.hs",
        );
        output.print_fix(&Fix::new(
            "Add a `test-suite` stanza with `type: exitcode-stdio-1.0` and `main-is`, \
             or create test/Main.hs",
        ));
        return Ok(3);
    }

    // Initialize plugin hooks (no GHC version: this is a BHC run).
    let mut hooks = PluginHooks::from_project(project, None);
    if let Some(ref mut h) = hooks {
        if let Err(e) = h.initialize() {
            output.verbose(&format!("Plugin initialization warning: {}", e));
        }
        if !h.run_pre_hook(HookEvent::PreTest, output) {
            output.error("Pre-test hook failed");
            return Ok(6); // Hook failure exit code
        }
    }

    output.status("Testing", &format!("{} (BHC)", project.name()));

    let build_config = &project.manifest.build;
    let build_options = CompilerBuildOptions {
        release: false,
        optimization: Some(build_config.optimization),
        jobs: None,
        target,
        package: None,
        verbose: output.is_verbose(),
        extra_flags: build_config.ghc_flags.clone(),
        src_dirs: Vec::new(), // per suite
        werror: build_config.werror,
    };

    let start = Instant::now();
    let mut passed: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut build_failed = false;

    for (pkg, suite) in &suites {
        let exe = pkg.root.join(".hx/bhc-test").join(&suite.name);

        output.status("Building", &format!("{} (BHC)", suite.name));
        if let Err(e) = backend
            .build_test_suite(&pkg.root, suite, &build_options, &exe, output)
            .await
        {
            // BHC's own output has already been printed by the backend.
            output.error(&format!(
                "Test suite `{}` failed to build: {}",
                suite.name, e
            ));
            failed.push(suite.name.clone());
            build_failed = true;
            continue;
        }

        let args = pattern
            .as_deref()
            .map(|p| suite.framework.pattern_args(p))
            .unwrap_or_default();
        output.status("Running", &suite.name);
        output.verbose(format!("  {} {}", exe.display(), args.join(" ")).trim_end());

        match std::process::Command::new(&exe)
            .args(&args)
            .current_dir(&pkg.root)
            .status()
        {
            Ok(status) if status.success() => passed.push(suite.name.clone()),
            Ok(status) => {
                let how = match status.code() {
                    Some(code) => format!("exit code {code}"),
                    None => "a signal".to_string(),
                };
                output.error(&format!("Test suite `{}` failed ({})", suite.name, how));
                failed.push(suite.name.clone());
            }
            Err(e) => {
                output.error(&format!(
                    "Failed to run test suite `{}` ({}): {}",
                    suite.name,
                    exe.display(),
                    e
                ));
                failed.push(suite.name.clone());
            }
        }
    }

    let duration = start.elapsed();
    let success = failed.is_empty();
    if let Some(ref mut h) = hooks {
        h.run_post_test_hook(success, passed.len(), failed.len(), 0, output);
    }

    if success {
        output.status(
            "Finished",
            &format!(
                "{} BHC test suite(s) passed in {}",
                passed.len(),
                format_build_duration(duration)
            ),
        );
        Ok(0)
    } else {
        output.error(&format!(
            "{} of {} BHC test suite(s) failed: {}",
            failed.len(),
            suites.len(),
            failed.join(", ")
        ));
        if build_failed {
            output.info("hint: Run `hx test --verbose` for full compiler output");
        }
        Ok(5)
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_main_source;
    use std::path::PathBuf;

    #[test]
    fn collect_project_deps_spans_members_and_skips_local_packages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("hx.toml"), "[project]\nname = \"ws\"\n").unwrap();
        std::fs::write(
            root.join("cabal.project"),
            "packages: ./lib-a ./lib-b ./app\n",
        )
        .unwrap();
        for (name, deps) in [
            ("lib-a", "base, text"),
            ("lib-b", "base, lib-a, containers"),
            ("app", "base, lib-b, lib-a, aeson"),
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(
                root.join(name).join(format!("{name}.cabal")),
                format!("name: {name}\nversion: 0.1\n\nlibrary\n  build-depends: {deps}\n"),
            )
            .unwrap();
        }
        let project = hx_config::Project::load(root).unwrap();
        let ctx = hx_solver::CabalContext::host("9.8.2".parse().unwrap());
        let mut names: Vec<String> = super::collect_project_deps(&project, &ctx)
            .into_iter()
            .map(|d| d.name)
            .collect();
        names.sort();
        // Every member contributes; members themselves are never sent to the
        // Hackage solver.
        assert_eq!(names, vec!["aeson", "base", "containers", "text"]);
    }

    #[test]
    fn resolve_main_source_finds_file_under_src_dir() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/Main.hs"), "module Main where\n").unwrap();

        let got = resolve_main_source("Main", &[PathBuf::from("src")], root.path());
        // Normalize the OS path separator (`\` on Windows) before comparing.
        assert_eq!(got.replace('\\', "/"), "src/Main.hs");
    }

    #[test]
    fn resolve_main_source_maps_dotted_module_to_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("lib/Foo")).unwrap();
        std::fs::write(root.path().join("lib/Foo/Bar.hs"), "module Foo.Bar where\n").unwrap();

        let got = resolve_main_source("Foo.Bar", &[PathBuf::from("lib")], root.path());
        // Normalize the OS path separator (`\` on Windows) before comparing.
        assert_eq!(got.replace('\\', "/"), "lib/Foo/Bar.hs");
    }

    #[test]
    fn resolve_main_source_falls_back_to_relative_path() {
        let root = tempfile::tempdir().unwrap();
        // No matching file exists; returns the bare relative path so bhc emits a
        // clear missing-file error rather than treating "Main" as a file.
        let got = resolve_main_source("Main", &[PathBuf::from("src")], root.path());
        assert_eq!(got, "Main.hs");
    }

    fn project(workspace: &[&str]) -> hx_config::Project {
        let mut manifest = hx_config::Manifest::default();
        manifest.project.name = "demo".to_string();
        hx_config::Project {
            root: PathBuf::from("/ws"),
            manifest,
            cabal_file: Some(PathBuf::from("/ws/demo.cabal")),
            has_cabal_project: !workspace.is_empty(),
            workspace_packages: workspace
                .iter()
                .map(|name| hx_config::WorkspacePackage {
                    name: name.to_string(),
                    path: PathBuf::from(name),
                    cabal_file: PathBuf::from(format!("{name}/{name}.cabal")),
                    version: None,
                    dependencies: Vec::new(),
                    executables: Vec::new(),
                    test_suites: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn bhc_test_selection_single_package() {
        use super::select_bhc_test_packages;
        let p = project(&[]);

        let (pkgs, filter) = select_bhc_test_packages(&p, None);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "demo");
        assert_eq!(pkgs[0].root, PathBuf::from("/ws"));
        assert_eq!(pkgs[0].cabal_file, Some(PathBuf::from("/ws/demo.cabal")));
        assert_eq!(filter, None);

        // The project's own name selects every suite.
        let (_, filter) = select_bhc_test_packages(&p, Some("demo"));
        assert_eq!(filter, None);

        // Any other name narrows the run to that suite.
        let (pkgs, filter) = select_bhc_test_packages(&p, Some("demo-test"));
        assert_eq!(pkgs.len(), 1);
        assert_eq!(filter.as_deref(), Some("demo-test"));
    }

    #[test]
    fn bhc_test_selection_workspace() {
        use super::select_bhc_test_packages;
        let p = project(&["core", "cli"]);

        let (pkgs, filter) = select_bhc_test_packages(&p, None);
        let names: Vec<&str> = pkgs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["core", "cli"]);
        assert_eq!(filter, None);

        // A member name selects that member's suites only.
        let (pkgs, filter) = select_bhc_test_packages(&p, Some("cli"));
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].root, PathBuf::from("/ws/cli"));
        assert_eq!(pkgs[0].cabal_file, Some(PathBuf::from("/ws/cli/cli.cabal")));
        assert_eq!(filter, None);

        // Otherwise it names a suite, searched across all members.
        let (pkgs, filter) = select_bhc_test_packages(&p, Some("core-test"));
        assert_eq!(pkgs.len(), 2);
        assert_eq!(filter.as_deref(), Some("core-test"));
    }
}
