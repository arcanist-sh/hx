//! Lock command implementation.

use anyhow::{Context, Result};
use hx_cabal::freeze;
use hx_config::{LOCKFILE_FILENAME, MANIFEST_FILENAME, Manifest, Project, find_project_root};
use hx_lock::{LockedPackage, Lockfile, WorkspacePackageInfo, parse_freeze_file};
use hx_plugins::HookEvent;
use hx_solver::{
    CabalContext, Dependency, IndexOptions, MirrorOptions, Resolver, ResolverConfig, SnapshotId,
    best_index_path, compute_deps_fingerprint, index_is_current, load_cached_index,
    load_cached_resolution, load_index, load_snapshot, parse_cabal_ctx, save_index_cache,
    save_resolution_cache, update_index,
};
use hx_toolchain::Toolchain;
use hx_ui::{Output, Spinner};
use std::path::Path;

use crate::plugins::PluginHooks;

/// Run the lock command.
pub async fn run(
    use_cabal: bool,
    update: Option<Vec<String>>,
    snapshot: Option<String>,
    output: &Output,
) -> Result<i32> {
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
                output.info("Examples: lts-22.28, lts-22, nightly-2024-01-15, nightly");
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

    // Calculate fingerprint
    let fingerprint = lockfile.fingerprint();
    lockfile.plan.hash = Some(fingerprint);

    // Detect platform
    lockfile.plan.platform = Some(detect_platform());

    // Write lockfile
    let lockfile_path = project.lockfile_path();
    lockfile.to_file(&lockfile_path)?;

    if project.is_workspace() {
        spinner.finish_success(format!(
            "Created {} with {} workspace packages and {} external dependencies",
            LOCKFILE_FILENAME,
            lockfile.workspace.packages.len(),
            lockfile.packages.len()
        ));
    } else {
        spinner.finish_success(format!(
            "Created {} with {} packages",
            LOCKFILE_FILENAME,
            lockfile.packages.len()
        ));
    }

    output.info(&format!("Lockfile: {}", lockfile_path.display()));

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

    // Build the context for evaluating `.cabal` conditionals: the GHC version
    // we're locking for (manifest pin, else detected, else a sensible default)
    // plus the host platform. This keeps platform/compiler-specific deps
    // (e.g. `Win32` on Windows, `semigroups` on old GHC) out of the lockfile.
    let cabal_ctx = {
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
    };

    // Collect dependencies from project .cabal files first
    // (so we can check the resolution cache before loading the index)
    let spinner = Spinner::new("Collecting project dependencies...");
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
        let cabal = parse_cabal_ctx(&content, &cabal_ctx);
        for dep in cabal.all_dependencies() {
            // Skip workspace packages
            if !workspace_names.contains(&dep.name) && !all_deps.iter().any(|d| d.name == dep.name)
            {
                all_deps.push(dep);
            }
        }
    }
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

        // Determine the source - stackage if package came from snapshot, otherwise hackage
        let source = if snapshot
            .as_ref()
            .is_some_and(|(_, s)| s.packages.contains_key(&pkg.name))
        {
            "stackage".to_string()
        } else {
            "hackage".to_string()
        };

        lockfile.add_package(LockedPackage {
            name: pkg.name.clone(),
            version: pkg.version.to_string(),
            source,
            hash: None,
        });
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

    // Calculate fingerprint
    let fingerprint = lockfile.fingerprint();
    lockfile.plan.hash = Some(fingerprint);

    // Detect platform
    lockfile.plan.platform = Some(detect_platform());

    // Write lockfile
    let lockfile_path = project.lockfile_path();
    lockfile.to_file(&lockfile_path)?;

    if project.is_workspace() {
        spinner.finish_success(format!(
            "Created {} with {} workspace packages and {} external dependencies",
            LOCKFILE_FILENAME,
            lockfile.workspace.packages.len(),
            lockfile.packages.len()
        ));
    } else {
        spinner.finish_success(format!(
            "Created {} with {} packages",
            LOCKFILE_FILENAME,
            lockfile.packages.len()
        ));
    }

    output.info(&format!("Lockfile: {}", lockfile_path.display()));

    // Run post-lock hooks
    if let Some(ref mut h) = hooks {
        h.run_post_hook(HookEvent::PostLock, output);
    }

    Ok(0)
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
pub async fn sync(force: bool, output: &Output) -> Result<i32> {
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
        package: None,
        verbose: output.is_verbose(),
        fingerprint: None,
        ghc_version: None,
        package_count: None,
        project_name: None,
        toolchain_bin_dirs,
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
