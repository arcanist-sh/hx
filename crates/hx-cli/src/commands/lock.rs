//! Lock command implementation.

use anyhow::{Context, Result};
use hx_cabal::freeze;
use hx_config::{LOCKFILE_FILENAME, MANIFEST_FILENAME, Manifest, Project, find_project_root};
use hx_lock::{
    LOCK_VERSION, LockedPackage, Lockfile, WorkspacePackageInfo, parse_freeze_file,
    parse_freeze_index_state,
};
use hx_plugins::HookEvent;
use hx_solver::{
    CabalContext, Dependency, IndexOptions, MirrorOptions, Resolver, ResolverConfig, SnapshotId,
    best_index_path, compute_deps_fingerprint, default_index_path, index_is_current,
    load_cached_index, load_cached_resolution, load_index, load_snapshot, parse_cabal_ctx,
    save_index_cache, save_resolution_cache, update_index,
};
use hx_toolchain::Toolchain;
use hx_ui::{Output, Spinner};
use std::path::{Path, PathBuf};

use super::sources::{self, IndexFinding};
use crate::plugins::PluginHooks;

/// Run the lock command.
pub async fn run(
    use_cabal: bool,
    update: Option<Vec<String>>,
    snapshot: Option<String>,
    check: bool,
    output: &Output,
) -> Result<i32> {
    if check {
        return run_check(output).await;
    }

    // Handle --snapshot flag: set snapshot in manifest before locking
    if let Some(snapshot_str) = snapshot {
        let project_root = find_project_root(".")?;
        let manifest_path = project_root.join(MANIFEST_FILENAME);

        // Validate the snapshot first
        let snapshot_id = match SnapshotId::parse(&snapshot_str) {
            Ok(id) => id,
            Err(e) => {
                output.error(&format!(
                    "Invalid snapshot identifier '{}': {}",
                    snapshot_str, e
                ));
                output.info("Examples: lts-24.64, lts-24, nightly-2026-10-19, nightly");
                return Ok(1);
            }
        };

        output.status("Validating", &format!("snapshot {}...", snapshot_str));
        match load_snapshot(&snapshot_id, None).await {
            Ok(snap) => {
                output.info(&format!(
                    "Snapshot {} uses GHC {} with {} packages",
                    snapshot_str,
                    snap.metadata.ghc_version,
                    snap.packages.len()
                ));
            }
            Err(e) => {
                output.error(&format!("Failed to load snapshot: {}", e));
                return Ok(1);
            }
        }

        // Update manifest
        let mut manifest = Manifest::from_file(&manifest_path)?;
        manifest.stackage.snapshot = Some(snapshot_str.clone());
        manifest.to_file(&manifest_path)?;
        output.info(&format!(
            "Set snapshot to {} in {}",
            snapshot_str, MANIFEST_FILENAME
        ));
    }

    // Default is native solver, --cabal uses cabal freeze
    if !use_cabal {
        return run_native(update, output).await;
    }

    // --update is only supported with native solver
    if update.is_some() {
        output.warn("--update flag is only supported with native solver (without --cabal)");
        output.info("Run `hx lock --update` without --cabal to update specific packages");
    }
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Initialize plugin hooks
    let mut hooks = PluginHooks::from_project(&project, None);
    if let Some(ref mut h) = hooks {
        if let Err(e) = h.initialize() {
            output.verbose(&format!("Plugin initialization warning: {}", e));
        }
    }

    // Run pre-lock hooks
    if let Some(ref mut h) = hooks {
        if !h.run_pre_hook(HookEvent::PreLock, output) {
            output.error("Pre-lock hook failed");
            return Ok(6); // Hook failure exit code
        }
    }

    if project.is_workspace() {
        output.status(
            "Locking",
            &format!("{} ({} packages)", project.name(), project.package_count()),
        );
    } else {
        output.status("Locking", project.name());
    }

    // Update package index
    freeze::update(output.is_verbose()).await?;

    // Run cabal freeze
    let build_dir = project.cabal_build_dir();
    let freeze_content = freeze::freeze(&project.root, &build_dir).await?;

    // Create lockfile
    let spinner = Spinner::new("Creating lockfile...");

    let mut lockfile = Lockfile::new();

    // Set toolchain versions from detected tools
    let toolchain = Toolchain::detect().await;
    lockfile.set_toolchain(
        toolchain.ghc.status.version().map(|v| v.to_string()),
        toolchain.cabal.status.version().map(|v| v.to_string()),
    );

    // Set workspace info if this is a workspace
    if project.is_workspace() {
        let workspace_packages = collect_workspace_packages(&project);
        lockfile.set_workspace(workspace_packages);
    }

    // Parse freeze file and add packages
    // Filter out workspace packages from external dependencies
    let workspace_names: Vec<_> = project
        .package_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let packages = parse_freeze_file(&freeze_content);
    for pkg in packages {
        // Skip workspace packages - they're tracked separately
        if !workspace_names.contains(&pkg.name) {
            lockfile.add_package(pkg);
        }
    }

    lockfile.plan.compiler_id = lockfile.toolchain.ghc.as_ref().map(|v| format!("ghc-{v}"));
    apply_project_context(&mut lockfile, &project);
    spinner.finish_success("Collected locked versions");

    // cabal resolved against its own index; pin hashes from that index, as of
    // the index-state the freeze file recorded (if any).
    let index_path = default_index_path().or_else(best_index_path);
    record_hashes(
        &mut lockfile,
        index_path,
        parse_freeze_index_state(&freeze_content),
        &project,
        output,
    );

    let spinner = Spinner::new("Writing lockfile...");
    let lockfile_path = project.lockfile_path();
    let write = write_lockfile(&mut lockfile, &lockfile_path)?;

    finish_write(spinner, &lockfile, &project, write, output);

    // Run post-lock hooks
    if let Some(ref mut h) = hooks {
        h.run_post_hook(HookEvent::PostLock, output);
    }

    Ok(0)
}

/// Run the lock command using the native solver.
async fn run_native(update: Option<Vec<String>>, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Initialize plugin hooks
    let mut hooks = PluginHooks::from_project(&project, None);
    if let Some(ref mut h) = hooks {
        if let Err(e) = h.initialize() {
            output.verbose(&format!("Plugin initialization warning: {}", e));
        }
    }

    // Run pre-lock hooks
    if let Some(ref mut h) = hooks {
        if !h.run_pre_hook(HookEvent::PreLock, output) {
            output.error("Pre-lock hook failed");
            return Ok(6); // Hook failure exit code
        }
    }

    let is_update_mode = update.is_some();
    let update_packages = update.unwrap_or_default();

    // Load existing lockfile if updating
    let existing_lockfile = if is_update_mode {
        let lockfile_path = project.lockfile_path();
        if lockfile_path.exists() {
            match Lockfile::from_file(&lockfile_path) {
                Ok(lf) => Some(lf),
                Err(e) => {
                    output.warn(&format!("Could not read existing lockfile: {}", e));
                    output.info("Performing full resolution instead");
                    None
                }
            }
        } else {
            output.warn("No existing lockfile found, performing full resolution");
            None
        }
    } else {
        None
    };

    if is_update_mode {
        if update_packages.is_empty() {
            output.status("Updating", "all packages to latest versions");
        } else {
            output.status(
                "Updating",
                &format!("packages: {}", update_packages.join(", ")),
            );
        }
    } else if project.is_workspace() {
        output.status(
            "Locking",
            &format!("{} ({} packages)", project.name(), project.package_count()),
        );
    } else {
        output.status("Locking", project.name());
    }

    let cabal_ctx = lock_context(&project).await;

    // Collect dependencies from project .cabal files first
    // (so we can check the resolution cache before loading the index)
    let spinner = Spinner::new("Collecting project dependencies...");
    let all_deps = collect_project_deps(&project, &cabal_ctx)?;
    spinner.finish_success(format!("Found {} dependencies", all_deps.len()));

    // Check for Stackage snapshot configuration
    let snapshot_config = &project.manifest.stackage;
    let snapshot = if let Some(snapshot_str) = &snapshot_config.snapshot {
        let spinner = Spinner::new(format!("Loading Stackage snapshot {}...", snapshot_str));
        match SnapshotId::parse(snapshot_str) {
            Ok(snapshot_id) => match load_snapshot(&snapshot_id, None).await {
                Ok(snapshot) => {
                    spinner.finish_success(format!(
                        "Loaded snapshot {} ({} packages, GHC {})",
                        snapshot_str,
                        snapshot.packages.len(),
                        snapshot.metadata.ghc_version
                    ));

                    // Warn if project GHC version doesn't match snapshot
                    if let Some(ref project_ghc) = project.manifest.toolchain.ghc {
                        if *project_ghc != snapshot.metadata.ghc_version {
                            output.warn(&format!(
                                "Project uses GHC {} but snapshot {} uses GHC {}",
                                project_ghc, snapshot_str, snapshot.metadata.ghc_version
                            ));
                            output.info(&format!(
                                "Consider updating [toolchain].ghc to \"{}\" in hx.toml",
                                snapshot.metadata.ghc_version
                            ));
                        }
                    }

                    Some((snapshot_str.clone(), snapshot))
                }
                Err(e) => {
                    spinner.finish_error(format!("Failed to load snapshot: {}", e));
                    if !snapshot_config.allow_newer {
                        return Err(anyhow::anyhow!(
                            "Snapshot {} could not be loaded and allow_newer is not set. \
                             Either fix the snapshot or set [stackage].allow_newer = true",
                            snapshot_str
                        ));
                    }
                    output.warn("Continuing without snapshot constraints (allow_newer is set)");
                    None
                }
            },
            Err(e) => {
                output.error(&format!(
                    "Invalid snapshot identifier '{}': {}",
                    snapshot_str, e
                ));
                return Ok(3); // Config error
            }
        }
    } else {
        None
    };

    // Compute fingerprint for dependencies (for resolution cache)
    // Include snapshot name in fingerprint if configured
    let mut deps_for_fingerprint: Vec<(String, String)> = all_deps
        .iter()
        .map(|d| (d.name.clone(), d.constraint.to_string()))
        .collect();
    if let Some((ref snapshot_name, _)) = snapshot {
        deps_for_fingerprint.push(("__snapshot__".to_string(), snapshot_name.clone()));
    }

    // Versions the installed GHC fixes (see `toolchain_fixed_packages`); they
    // shape the plan, so they are part of the resolution cache key.
    let direct_names: Vec<String> = all_deps.iter().map(|d| d.name.clone()).collect();
    let toolchain_pins =
        toolchain_fixed_packages(&cabal_ctx.ghc_version, &direct_names, output).await;
    for (name, version) in &toolchain_pins {
        deps_for_fingerprint.push((format!("__ghc__{name}"), version.clone()));
    }
    let deps_fingerprint = compute_deps_fingerprint(&deps_for_fingerprint);

    // Skip cache if we're updating packages
    let use_cache = !is_update_mode;

    // Try to load cached resolution (only if not updating)
    let plan = if use_cache {
        if let Ok(cached_plan) = load_cached_resolution(&deps_fingerprint) {
            output.info("Using cached resolution");
            Some(cached_plan)
        } else {
            None
        }
    } else {
        None
    };

    let plan = if let Some(plan) = plan {
        plan
    } else {
        // Auto-update index if stale (> 24 hours old)
        if !index_is_current(24) {
            output.info("Index is stale or missing, updating...");
            let options = MirrorOptions {
                show_progress: !output.verbosity().eq(&hx_ui::Verbosity::Quiet),
                ..Default::default()
            };
            match update_index(&options).await {
                Ok(result) => {
                    if result.downloaded {
                        output.status(
                            "Updated",
                            &format!(
                                "index ({:.2} MB)",
                                result.bytes_downloaded as f64 / 1_000_000.0
                            ),
                        );
                    }
                }
                Err(e) => {
                    output.warn(&format!("Failed to update index: {}", e));
                    output.info("Continuing with existing index (if available)...");
                }
            }
        }

        // Find the Hackage index (prefers hx-managed, falls back to cabal)
        let index_path = best_index_path().context(
            "Hackage index not found. Run `hx index update` or `cabal update` to download the package index.",
        )?;

        // Try to load cached index, fall back to parsing the tar.gz. The index
        // is parsed with the build context so conditionals resolve correctly;
        // the cache is keyed by that context.
        let index_options = IndexOptions {
            context: Some(cabal_ctx.clone()),
            ..IndexOptions::default()
        };
        let context_key = cabal_ctx.cache_key();
        let spinner = Spinner::new("Loading package index...");
        let index = match load_cached_index(&index_path, &context_key) {
            Ok(cached_index) => {
                spinner.finish_success(format!(
                    "Loaded {} packages from cache",
                    cached_index.package_count()
                ));
                cached_index
            }
            Err(_) => {
                // Parse from source and cache
                let index = load_index(&index_path, &index_options)
                    .context("Failed to load Hackage index")?;

                // Save to cache for next time
                if let Err(e) = save_index_cache(&index, &index_path, &context_key) {
                    output.warn(&format!("Failed to cache index: {}", e));
                }

                spinner.finish_success(format!(
                    "Loaded {} packages with {} versions",
                    index.package_count(),
                    index.version_count()
                ));
                index
            }
        };

        // Run the resolver
        let spinner = Spinner::new("Resolving dependencies...");

        // Configure resolver with pinned versions if updating specific packages
        let mut config = ResolverConfig::default();

        if is_update_mode && !update_packages.is_empty() {
            // Pin versions from existing lockfile for packages we're NOT updating
            if let Some(ref existing) = existing_lockfile {
                for pkg in &existing.packages {
                    // Don't pin packages that are being updated
                    if !update_packages
                        .iter()
                        .any(|u| u.eq_ignore_ascii_case(&pkg.name))
                        && let Ok(version) = pkg.version.parse()
                    {
                        config.installed.insert(pkg.name.clone(), version);
                    }
                }
                let pinned_count = config.installed.len();
                if pinned_count > 0 {
                    output.info(&format!(
                        "Pinning {} packages at their locked versions",
                        pinned_count
                    ));
                }
            }
        }

        // Packages fixed by the installed GHC override everything else: a
        // plan that disagrees with them cannot be built.
        for (name, version) in &toolchain_pins {
            if let Ok(v) = version.parse() {
                config.installed.insert(name.clone(), v);
            }
        }

        // Pin versions from BHC Platform snapshot if configured
        let bhc_platform_config = &project.manifest.bhc_platform;
        if let Some(ref platform_str) = bhc_platform_config.snapshot
            && project.manifest.compiler.backend == hx_config::CompilerBackend::Bhc
        {
            if let Ok(platform_id) = SnapshotId::parse(platform_str)
                && let Ok(platform_snap) = hx_solver::bhc_platform::load_bhc_platform(&platform_id)
            {
                let mut pinned = 0;
                for pkg in platform_snap.packages.values() {
                    if !config.installed.contains_key(&pkg.name)
                        && let Ok(version) = pkg.version.parse()
                    {
                        config.installed.insert(pkg.name.clone(), version);
                        pinned += 1;
                    }
                }

                // Also add extra_deps from bhc-platform config
                for (name, version_str) in &bhc_platform_config.extra_deps {
                    if let Ok(version) = version_str.parse() {
                        config.installed.insert(name.clone(), version);
                    }
                }

                output.info(&format!(
                    "Pinning {} packages from BHC Platform {}",
                    pinned, platform_str
                ));
            }
        }

        // Pin versions from Stackage snapshot if configured
        if let Some((ref snapshot_name, ref snap)) = snapshot {
            let mut pinned = 0;
            for pkg in snap.packages.values() {
                // Don't override packages being updated or already pinned
                if !config.installed.contains_key(&pkg.name)
                    && let Ok(version) = pkg.version.parse()
                {
                    config.installed.insert(pkg.name.clone(), version);
                    pinned += 1;
                }
            }

            // Also add extra_deps from config (overrides snapshot)
            for (name, version_str) in &snapshot_config.extra_deps {
                if let Ok(version) = version_str.parse() {
                    config.installed.insert(name.clone(), version);
                }
            }

            output.info(&format!(
                "Pinning {} packages from Stackage snapshot {}",
                pinned, snapshot_name
            ));
        }

        let resolver = Resolver::with_config(&index, config);

        let plan = resolver
            .resolve_all(&all_deps)
            .context("Failed to resolve dependencies")?;

        // Cache the resolution result
        if let Err(e) = save_resolution_cache(&deps_fingerprint, &plan) {
            output.warn(&format!("Failed to cache resolution: {}", e));
        }

        spinner.finish_success(format!("Resolved {} packages", plan.packages.len()));
        plan
    };

    // Create lockfile
    let spinner = Spinner::new("Creating lockfile...");

    let mut lockfile = Lockfile::new();

    // Set toolchain versions from detected tools
    let toolchain = Toolchain::detect().await;
    lockfile.set_toolchain(
        toolchain.ghc.status.version().map(|v| v.to_string()),
        toolchain.cabal.status.version().map(|v| v.to_string()),
    );

    // Set Stackage snapshot if used
    if let Some((ref snapshot_name, _)) = snapshot {
        lockfile.set_snapshot(Some(snapshot_name.clone()));
    }

    // Set workspace info if this is a workspace
    if project.is_workspace() {
        let workspace_packages = collect_workspace_packages(&project);
        lockfile.set_workspace(workspace_packages);
    }

    // Add resolved packages to lockfile and track updates
    let mut updated_packages: Vec<(String, String, String)> = Vec::new(); // (name, old_version, new_version)

    for pkg in &plan.packages {
        // Skip base packages (they come with GHC)
        if pkg.name == "base" || pkg.name == "ghc-prim" || pkg.name == "integer-gmp" {
            continue;
        }

        // Track version changes when updating
        if is_update_mode
            && let Some(ref existing) = existing_lockfile
            && let Some(old_pkg) = existing.packages.iter().find(|p| p.name == pkg.name)
        {
            let new_version = pkg.version.to_string();
            if old_pkg.version != new_version {
                updated_packages.push((pkg.name.clone(), old_pkg.version.clone(), new_version));
            }
        }

        // Determine the source: the installed GHC's copy if the toolchain
        // fixes it, stackage if it came from the snapshot, otherwise hackage
        let source = if toolchain_pins
            .get(&pkg.name)
            .is_some_and(|v| *v == pkg.version.to_string())
        {
            "ghc".to_string()
        } else if snapshot
            .as_ref()
            .is_some_and(|(_, s)| s.packages.contains_key(&pkg.name))
        {
            "stackage".to_string()
        } else {
            "hackage".to_string()
        };

        lockfile.add_package(
            LockedPackage::hackage(pkg.name.clone(), pkg.version.to_string()).with_source(source),
        );
    }

    // Show updated packages
    if is_update_mode && !updated_packages.is_empty() {
        output.info(&format!("Updated {} package(s):", updated_packages.len()));
        for (name, old_ver, new_ver) in &updated_packages {
            output.info(&format!("  {} {} -> {}", name, old_ver, new_ver));
        }
    } else if is_update_mode {
        output.info("No package updates available");
    }

    // The plan was resolved for this compiler and platform: record both, so
    // a build elsewhere can tell it is using a plan made for another context.
    lockfile.plan.compiler_id = Some(format!("ghc-{}", cabal_ctx.ghc_version));
    apply_project_context(&mut lockfile, &project);
    spinner.finish_success("Collected locked versions");

    record_hashes(&mut lockfile, best_index_path(), None, &project, output);

    let spinner = Spinner::new("Writing lockfile...");
    let lockfile_path = project.lockfile_path();
    let write = write_lockfile(&mut lockfile, &lockfile_path)?;
    finish_write(spinner, &lockfile, &project, write, output);

    // Run post-lock hooks
    if let Some(ref mut h) = hooks {
        h.run_post_hook(HookEvent::PostLock, output);
    }

    Ok(0)
}

/// The context `.cabal` conditionals are evaluated in when locking: the GHC
/// version we're locking for (manifest pin, else detected, else a sensible
/// default) plus the host platform. This keeps platform/compiler-specific deps
/// (e.g. `Win32` on Windows, `semigroups` on old GHC) out of the lockfile.
async fn lock_context(project: &Project) -> CabalContext {
    let ghc: hx_solver::Version = match project
        .manifest
        .toolchain
        .ghc
        .as_deref()
        .and_then(|s| s.parse().ok())
    {
        Some(v) => v,
        None => Toolchain::detect()
            .await
            .ghc
            .status
            .version()
            .and_then(|v| v.to_string().parse().ok())
            .unwrap_or_else(|| "9.8.2".parse().expect("valid default GHC version")),
    };
    CabalContext::host(ghc)
}

/// Package versions the installed GHC fixes for any build: its
/// compiler-internal packages (`base`, `template-haskell`, ...) and every
/// package they depend on in the global package database (e.g. `deepseq`,
/// `pretty`). Cabal cannot pick other versions of these, because the installed
/// compiler-internal libraries were built against exactly these instances.
///
/// Empty when `ghc-pkg` is unavailable or belongs to a different GHC than the
/// one being locked for (the plan then cannot account for installed versions).
async fn toolchain_fixed_packages(
    ghc: &hx_solver::Version,
    direct_deps: &[String],
    output: &Output,
) -> std::collections::BTreeMap<String, String> {
    let mut fixed = std::collections::BTreeMap::new();
    let runner = hx_core::CommandRunner::new();
    let version_ok = match runner.run("ghc-pkg", ["--version"]).await {
        Ok(out) if out.success() => out
            .stdout
            .split_whitespace()
            .last()
            .is_some_and(|v| v == ghc.to_string()),
        _ => false,
    };
    if !version_ok {
        output.verbose(&format!(
            "ghc-pkg for GHC {ghc} not found; installed package versions are not pinned"
        ));
        return fixed;
    }

    let packages = hx_cabal::query_all_packages(&[]).await;
    let by_id: std::collections::HashMap<&str, &hx_cabal::PackageInfo> =
        packages.iter().map(|p| (p.id.as_str(), p)).collect();
    // Roots: the compiler-internal packages every plan may use, plus any
    // compiler-API package (`ghc`, `ghci`, ...) the project depends on
    // directly. Pinning the closure of `ghc` unconditionally would freeze
    // half of the boot packages for projects that never use the GHC API.
    let mut stack: Vec<&hx_cabal::PackageInfo> = packages
        .iter()
        .filter(|p| hx_solver::resolver::is_ghc_builtin(&p.name))
        .filter(|p| !is_compiler_api(&p.name) || direct_deps.iter().any(|d| d == &p.name))
        .collect();
    while let Some(pkg) = stack.pop() {
        if fixed
            .insert(pkg.name.clone(), pkg.version.clone())
            .is_some()
        {
            continue;
        }
        stack.extend(pkg.depends.iter().filter_map(|id| by_id.get(id.as_str())));
    }
    if !fixed.is_empty() {
        output.verbose(&format!(
            "Pinning {} packages fixed by the installed GHC {}",
            fixed.len(),
            ghc
        ));
    }
    fixed
}

/// GHC-API packages that only constrain a plan when it actually uses them.
fn is_compiler_api(name: &str) -> bool {
    matches!(
        name,
        "ghc" | "ghci" | "ghc-boot" | "ghc-heap" | "ghc-experimental" | "libiserv"
    )
}

/// Collect the direct dependencies declared by the project's `.cabal` files
/// (excluding workspace members), deduplicated by name.
fn collect_project_deps(project: &Project, cabal_ctx: &CabalContext) -> Result<Vec<Dependency>> {
    let mut all_deps: Vec<Dependency> = Vec::new();
    let workspace_names: Vec<String> = project
        .package_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Scan every project `.cabal` file. `cabal_files()` covers both workspace
    // members and single-package projects; iterating `workspace_packages`
    // directly skipped single-package projects entirely (empty lockfile).
    for cabal_file in project.cabal_files() {
        let content = std::fs::read_to_string(&cabal_file)
            .with_context(|| format!("Failed to read {}", cabal_file.display()))?;
        let cabal = parse_cabal_ctx(&content, cabal_ctx);
        for dep in cabal.all_dependencies() {
            // Skip workspace packages
            if !workspace_names.contains(&dep.name) && !all_deps.iter().any(|d| d.name == dep.name)
            {
                all_deps.push(dep);
            }
        }
    }
    Ok(all_deps)
}

/// Record the platform, the BHC toolchain/profile and the BHC Platform
/// snapshot (for BHC projects) in the lockfile.
fn apply_project_context(lockfile: &mut Lockfile, project: &Project) {
    lockfile.plan.platform = Some(detect_platform());

    let manifest = &project.manifest;
    if manifest.compiler.backend == hx_config::CompilerBackend::Bhc {
        lockfile.toolchain.bhc = manifest
            .compiler
            .version
            .clone()
            .or_else(|| hx_toolchain::detect_bhc().map(|b| b.version));
        lockfile.toolchain.bhc_profile = Some(manifest.compiler.bhc.profile.as_str().to_string());
        lockfile.plan.bhc_platform = manifest.bhc_platform.snapshot.clone();
    }
}

/// Pin source hashes and `.cabal` revisions from the Hackage index.
///
/// Pins for unchanged versions are first carried over from the existing
/// lockfile, so a missing index never silently drops them.
fn record_hashes(
    lockfile: &mut Lockfile,
    index_path: Option<PathBuf>,
    index_state: Option<String>,
    project: &Project,
    output: &Output,
) {
    let previous = Lockfile::from_file(project.lockfile_path()).ok();
    if let Some(ref prev) = previous {
        for pkg in &mut lockfile.packages {
            if let Some(old) = prev.package(&pkg.name)
                && old.version == pkg.version
                && old.source == pkg.source
            {
                pkg.sha256 = old.sha256.clone();
                pkg.revision = old.revision;
                pkg.cabal_sha256 = old.cabal_sha256.clone();
            }
        }
    }

    let Some(index_path) = index_path else {
        lockfile.plan.index_state =
            index_state.or_else(|| previous.as_ref().and_then(|p| p.plan.index_state.clone()));
        output.warn("No Hackage index found, so source hashes could not be recorded");
        output.info("fix: Run `hx index update` and then `hx lock` again");
        return;
    };

    let spinner = Spinner::new("Recording source hashes...");
    match sources::record_source_hashes(lockfile, &index_path, index_state) {
        Ok(recording) => {
            let state = lockfile
                .plan
                .index_state
                .as_deref()
                .map(|s| format!(" (index-state {s})"))
                .unwrap_or_default();
            spinner.finish_success(format!(
                "Pinned {} source hash{}{}",
                recording.hashed,
                if recording.hashed == 1 { "" } else { "es" },
                state
            ));
            if !recording.missing.is_empty() {
                output.warn(&format!(
                    "no source hash in the Hackage index for: {}",
                    recording.missing.join(", ")
                ));
                output.info("fix: Run `hx index update` and then `hx lock` again");
            }
        }
        Err(e) => {
            spinner.finish_warning("Could not record source hashes");
            output.warn(&format!("{:#}", e));
        }
    }
}

/// What writing the lockfile changed.
enum WriteOutcome {
    /// No lockfile existed before.
    Created,
    /// The content changed.
    Updated,
    /// An older-format lockfile was rewritten in the current format.
    Upgraded(u32),
    /// Nothing changed; the file was left untouched.
    Unchanged,
}

/// Finalize (fingerprint) and write the lockfile, keeping the previous
/// `created_at` and leaving the file untouched when nothing changed.
fn write_lockfile(lockfile: &mut Lockfile, path: &Path) -> Result<WriteOutcome> {
    lockfile.plan.hash = None;
    lockfile.plan.hash = Some(lockfile.fingerprint());

    let previous = if path.exists() {
        Lockfile::from_file(path).ok()
    } else {
        None
    };
    let outcome = match previous {
        None if path.exists() => WriteOutcome::Updated,
        None => WriteOutcome::Created,
        Some(prev) if prev.needs_upgrade() => WriteOutcome::Upgraded(prev.version),
        Some(prev) if prev.same_content(lockfile) => {
            lockfile.created_at = prev.created_at;
            return Ok(WriteOutcome::Unchanged);
        }
        Some(_) => WriteOutcome::Updated,
    };
    lockfile.to_file(path)?;
    Ok(outcome)
}

/// Report a lockfile write.
fn finish_write(
    spinner: Spinner,
    lockfile: &Lockfile,
    project: &Project,
    write: WriteOutcome,
    output: &Output,
) {
    let contents = if project.is_workspace() {
        format!(
            "{} workspace packages and {} external dependencies",
            lockfile.workspace.packages.len(),
            lockfile.packages.len()
        )
    } else {
        format!("{} packages", lockfile.packages.len())
    };
    match write {
        WriteOutcome::Created => {
            spinner.finish_success(format!("Created {} with {}", LOCKFILE_FILENAME, contents))
        }
        WriteOutcome::Updated => {
            spinner.finish_success(format!("Updated {} with {}", LOCKFILE_FILENAME, contents))
        }
        WriteOutcome::Upgraded(from) => spinner.finish_success(format!(
            "Upgraded {} from format v{} to v{} ({})",
            LOCKFILE_FILENAME, from, LOCK_VERSION, contents
        )),
        WriteOutcome::Unchanged => spinner.finish_success(format!(
            "{} is up to date ({})",
            LOCKFILE_FILENAME, contents
        )),
    }
    output.info(&format!("Lockfile: {}", project.lockfile_path().display()));
}

/// `hx lock --check`: verify the lockfile without changing it.
///
/// Checks that hx.lock is current-format, matches hx.toml and the `.cabal`
/// files, pins every source, agrees with the Hackage index (as of its
/// `index_state`), and that every cached tarball matches its pin. Intended
/// for CI; exits non-zero on any mismatch.
async fn run_check(output: &Output) -> Result<i32> {
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;
    let started = std::time::Instant::now();
    let lockfile_path = project.lockfile_path();
    output.status("Checking", LOCKFILE_FILENAME);

    if !lockfile_path.exists() {
        output.error(&format!("{} not found", LOCKFILE_FILENAME));
        output.info("fix: Run `hx lock` to create it, and commit it");
        return Ok(3);
    }
    let lockfile = match Lockfile::from_file(&lockfile_path) {
        Ok(lock) => lock,
        Err(e) => {
            output.error(&format!("{} could not be read: {}", LOCKFILE_FILENAME, e));
            output.info("fix: Run `hx lock` to regenerate it");
            return Ok(3);
        }
    };

    let mut problems: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    if lockfile.needs_upgrade() {
        problems.push(format!(
            "lockfile uses format v{} (current is v{}) and has no source hashes",
            lockfile.version, LOCK_VERSION
        ));
    }

    // Consistency with hx.toml and the .cabal files.
    let ctx = lock_context(&project).await;
    for dep in collect_project_deps(&project, &ctx)? {
        if hx_lock::is_non_reinstallable(&dep.name) {
            continue;
        }
        match lockfile.package(&dep.name) {
            None => problems.push(format!(
                "dependency `{}` (from the .cabal file) is not in {}",
                dep.name, LOCKFILE_FILENAME
            )),
            Some(locked) => {
                if let Ok(v) = locked.version.parse::<hx_solver::Version>()
                    && !dep.constraint.matches(&v)
                {
                    problems.push(format!(
                        "`{}` is locked at {}, which does not satisfy `{}`",
                        dep.name, locked.version, dep.constraint
                    ));
                }
            }
        }
    }

    let mut current: Vec<String> = project
        .package_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut locked: Vec<String> = lockfile
        .workspace_package_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    current.sort();
    locked.sort();
    if project.is_workspace() != lockfile.is_workspace()
        || (project.is_workspace() && current != locked)
    {
        problems.push("workspace packages changed since the lock was created".to_string());
    }

    let manifest = &project.manifest;
    if manifest.stackage.snapshot != lockfile.plan.snapshot {
        problems.push(format!(
            "Stackage snapshot is {} in hx.toml but {} in {}",
            manifest.stackage.snapshot.as_deref().unwrap_or("unset"),
            lockfile.plan.snapshot.as_deref().unwrap_or("unset"),
            LOCKFILE_FILENAME
        ));
    }
    if let (Some(pin), Some(id)) = (&manifest.toolchain.ghc, &lockfile.plan.compiler_id)
        && id != &format!("ghc-{pin}")
    {
        problems.push(format!(
            "hx.toml pins GHC {pin}, but the lock was resolved for {id}"
        ));
    }
    if manifest.compiler.backend == hx_config::CompilerBackend::Bhc
        && lockfile.toolchain.bhc_profile.as_deref() != Some(manifest.compiler.bhc.profile.as_str())
    {
        problems.push(format!(
            "BHC profile is {} in hx.toml but {} in {}",
            manifest.compiler.bhc.profile.as_str(),
            lockfile.toolchain.bhc_profile.as_deref().unwrap_or("unset"),
            LOCKFILE_FILENAME
        ));
    }

    // Every sdist must be pinned.
    let unpinned: Vec<String> = lockfile
        .fetchable_sdists()
        .iter()
        .filter(|p| p.sha256.is_none())
        .map(|p| p.package_id())
        .collect();
    if !unpinned.is_empty() && !lockfile.needs_upgrade() {
        problems.push(format!(
            "{} package(s) have no source hash: {}",
            unpinned.len(),
            unpinned.join(", ")
        ));
    }
    for pkg in &lockfile.packages {
        if pkg.source_kind() == hx_lock::SourceKind::Git
            && !pkg
                .rev
                .as_deref()
                .is_some_and(|r| r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit()))
        {
            problems.push(format!(
                "git package `{}` is not pinned to a full commit hash",
                pkg.name
            ));
        }
    }

    // Agreement with the Hackage index (the one cabal will build from).
    if let Some(index) = default_index_path().or_else(best_index_path) {
        let spinner = Spinner::new("Verifying pins against the Hackage index...");
        match sources::verify_against_index(&lockfile, &index) {
            Ok(findings) => {
                spinner.finish_clear();
                for finding in findings {
                    match finding {
                        IndexFinding::Mismatch(m) => problems.push(m),
                        IndexFinding::Unverifiable(m) => notes.push(m),
                    }
                }
            }
            Err(e) => {
                spinner.finish_warning("Could not read the Hackage index");
                notes.push(format!("{:#}", e));
            }
        }
    } else {
        notes.push("no Hackage index available; pins were not checked against it".to_string());
    }

    // Cached tarballs must match their pins.
    let mut mismatches = Vec::new();
    let mut verified = 0;
    let caches: Vec<(PathBuf, bool)> = [
        hx_solver::default_package_cache_dir().map(|d| (d, false)),
        hx_solver::cabal_repo_cache_dir().map(|d| (d, true)),
    ]
    .into_iter()
    .flatten()
    .collect();
    for spec in sources::source_specs(&lockfile) {
        for (dir, cabal_layout) in &caches {
            let path = if *cabal_layout {
                hx_solver::cabal_cached_tarball_path(dir, &spec.name, &spec.version)
            } else {
                hx_solver::cached_tarball_path(dir, &spec.name, &spec.version)
            };
            match hx_solver::check_source_file(&path, spec.sha256.as_deref())? {
                hx_solver::SourceStatus::Verified { .. } => verified += 1,
                hx_solver::SourceStatus::Mismatch {
                    path,
                    expected,
                    actual,
                } => mismatches.push(sources::Mismatch {
                    id: spec.package_id(),
                    path: Some(path),
                    expected,
                    actual,
                }),
                _ => {}
            }
        }
    }

    for note in &notes {
        output.warn(note);
    }
    for m in &mismatches {
        sources::report_mismatch(output, m);
    }
    if problems.is_empty() && mismatches.is_empty() {
        output.success_summary(
            &format!(
                "{} is consistent ({} packages, {} cached tarball{} verified)",
                LOCKFILE_FILENAME,
                lockfile.packages.len(),
                verified,
                if verified == 1 { "" } else { "s" }
            ),
            started.elapsed(),
        );
        return Ok(0);
    }

    if !problems.is_empty() {
        output.error(&format!("{} is out of date", LOCKFILE_FILENAME));
        for p in &problems {
            output.info(&format!("  - {}", p));
        }
        output.info("fix: Run `hx lock` to update the lockfile, and commit the result");
    }
    Ok(3)
}

/// Collect workspace package information from the project.
fn collect_workspace_packages(project: &Project) -> Vec<WorkspacePackageInfo> {
    let mut packages = Vec::new();

    for pkg in &project.workspace_packages {
        // Parse version from .cabal file
        let version = parse_cabal_version(&pkg.cabal_file).unwrap_or_else(|| "0.0.0".to_string());

        packages.push(WorkspacePackageInfo {
            name: pkg.name.clone(),
            version,
            path: pkg.path.display().to_string(),
        });
    }

    packages
}

/// Parse the version field from a .cabal file.
fn parse_cabal_version(cabal_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(cabal_path).ok()?;

    for line in content.lines() {
        let line = line.trim();
        if line.to_lowercase().starts_with("version:") {
            let version = line
                .strip_prefix("version:")
                .or_else(|| line.strip_prefix("Version:"))
                .map(|s| s.trim().to_string());
            return version;
        }
    }

    None
}

/// Run the sync command.
///
/// Builds with the dependency versions, flags and Hackage index state pinned
/// in hx.lock. With `offline`, every locked sdist must already be cached and
/// verified (see `hx fetch`) and cabal's index present, so the build needs no
/// network access; otherwise sync fails up front, listing what is missing.
pub async fn sync(force: bool, offline: bool, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    if project.is_workspace() {
        output.status(
            "Syncing",
            &format!("{} ({} packages)", project.name(), project.package_count()),
        );
    } else {
        output.status("Syncing", project.name());
    }

    // Check if lockfile exists
    let lockfile_path = project.lockfile_path();
    if !lockfile_path.exists() {
        output.error(&format!("{} not found", LOCKFILE_FILENAME));
        output.info("Run `hx lock` to create a lockfile first");
        return Ok(3); // Config error
    }

    // Load lockfile
    let lockfile = Lockfile::from_file(&lockfile_path)?;

    if lockfile.needs_upgrade() {
        if offline {
            output.error(&format!(
                "{} uses format v{}, which has no source hashes to build offline from",
                LOCKFILE_FILENAME, lockfile.version
            ));
            output.info("fix: Run `hx lock` (online) to upgrade it, then `hx fetch`");
            return Ok(3);
        }
        output.warn(&format!(
            "{} uses format v{}; sources are not hash-verified",
            LOCKFILE_FILENAME, lockfile.version
        ));
        output.info("Run `hx lock` to upgrade it and pin source hashes");
    }

    // Make sure cabal will build exactly the pinned sources. Tarballs already
    // in cabal's cache must match their pins (a mismatch is never ignored);
    // verified copies from hx's cache are handed over when cabal lacks them.
    let extra_args = lockfile.cabal_args();
    if let (Some(hx_cache), Some(cabal_repo)) = (
        hx_solver::default_package_cache_dir(),
        hx_solver::cabal_repo_cache_dir(),
    ) {
        let report = sources::prepare_cabal_cache(&lockfile, &hx_cache, &cabal_repo)?;
        if !report.mismatched.is_empty() {
            for m in &report.mismatched {
                sources::report_mismatch(output, m);
            }
            return Ok(1);
        }
        if report.seeded > 0 {
            output.verbose(&format!(
                "Copied {} verified source tarball(s) into {}",
                report.seeded,
                cabal_repo.display()
            ));
        }
        if offline {
            if !report.missing.is_empty() {
                sources::report_missing_offline(output, &report.missing);
                return Ok(1);
            }
            if default_index_path().is_none() {
                output.error("cabal's Hackage index is missing, so cabal cannot plan offline");
                output.list_item(
                    "expected",
                    &cabal_repo.join("01-index.tar").display().to_string(),
                );
                output.info("fix: Run `hx fetch` (or `cabal update`) while online");
                return Ok(1);
            }
        }
    } else if offline {
        output.error("could not determine the package cache directories (no home directory)");
        output.info("fix: Set HOME (or CABAL_DIR) and run `hx fetch`");
        return Ok(1);
    }
    // Deliberately no `cabal --offline` here: cabal-install (verified with
    // 3.12.1.0) refuses *every* not-yet-built Hackage package under
    // `--offline`, even when its tarball is already cached, so first builds
    // would always fail. The preflight above already guarantees that every
    // source is cached and verified and that the index is present; with
    // `--index-state` pinned, cabal then builds without touching the network.

    // Verify toolchain if not forcing
    if !force {
        let toolchain = Toolchain::detect().await;

        if let (Some(lock_ghc), Some(detected_ghc)) =
            (&lockfile.toolchain.ghc, toolchain.ghc.status.version())
            && lock_ghc != &detected_ghc.to_string()
        {
            output.warn(&format!(
                "GHC version mismatch: lock has {}, detected {}",
                lock_ghc, detected_ghc
            ));
            output.info("Run `hx toolchain install` to install the correct version");
            output.info("Or run `hx sync --force` to override");
        }

        // Verify workspace consistency
        if project.is_workspace() != lockfile.is_workspace() {
            output.warn("Workspace configuration changed since lock was created");
            output.info("Run `hx lock` to regenerate the lockfile");
            if !force {
                return Ok(3);
            }
        }

        // Check if workspace packages have changed
        if lockfile.is_workspace() {
            let current_packages: Vec<_> = project
                .package_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let locked_packages = lockfile.workspace_package_names();

            let mut packages_changed = current_packages.len() != locked_packages.len();
            if !packages_changed {
                for pkg in &current_packages {
                    if !locked_packages.contains(&pkg.as_str()) {
                        packages_changed = true;
                        break;
                    }
                }
            }

            if packages_changed {
                output.warn("Workspace packages changed since lock was created");
                output.info("Run `hx lock` to regenerate the lockfile");
                if !force {
                    return Ok(3);
                }
            }
        }
    }

    // Build with locked dependencies
    // For now, just run a regular build since we have the freeze file
    let build_dir = project.cabal_build_dir();

    // Detect toolchain for PATH setup
    let toolchain = Toolchain::detect().await;
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

    let options = hx_cabal::BuildOptions {
        release: false,
        jobs: None,
        target: None,
        // A workspace has no package in its root; build every member.
        targets: if project.is_workspace() {
            vec!["all".to_string()]
        } else {
            Vec::new()
        },
        verbose: output.is_verbose(),
        fingerprint: None,
        ghc_version: None,
        package_count: None,
        project_name: None,
        toolchain_bin_dirs,
        extra_args,
    };

    match hx_cabal::build::build(&project.root, &build_dir, &options, output).await {
        Ok(_) => {
            if project.is_workspace() {
                output.success_summary(
                    &format!(
                        "Synced {} packages with locked dependencies",
                        project.package_count()
                    ),
                    std::time::Duration::from_secs(0),
                );
            } else {
                output.success_summary(
                    "Synced with locked dependencies",
                    std::time::Duration::from_secs(0),
                );
            }
            Ok(0)
        }
        Err(e) => {
            output.print_error(&e);
            Ok(5)
        }
    }
}

fn detect_platform() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    format!("{}-{}", arch, os)
}
