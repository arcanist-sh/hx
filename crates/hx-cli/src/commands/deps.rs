//! Dependency management commands (add, rm, why, graph).

use crate::cli::GraphFormat;
use anyhow::Result;
use hx_cabal::{CabalEditError, add_dependency, remove_dependency};
use hx_config::{Manifest, Project, find_project_root};
use hx_lock::Lockfile;
use hx_solver::{IndexOptions, best_index_path, load_index};
use hx_ui::Output;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Add a dependency to the project.
pub async fn add(package: &str, version: Option<&str>, _dev: bool, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(_) => {
            output.error("Not in an hx project (no hx.toml found)");
            output.info("Run `hx init` to create a new project first.");
            return Ok(3);
        }
    };

    // Find the .cabal file
    let cabal_file = find_cabal_file(&project_root);
    let cabal_file = match cabal_file {
        Some(path) => path,
        None => {
            output.error("No .cabal file found in project");
            output.info("Make sure your project has a .cabal file.");
            return Ok(3);
        }
    };

    // Format display string
    let display_pkg = if let Some(v) = version {
        format!("{} {}", package, v)
    } else {
        package.to_string()
    };

    output.status(
        "Adding",
        &format!(
            "{} to {}",
            display_pkg,
            cabal_file.file_name().unwrap().to_string_lossy()
        ),
    );

    // Add to .cabal file
    match add_dependency(&cabal_file, package, version) {
        Ok(()) => {
            // Also update hx.toml dependencies
            let manifest_path = project_root.join("hx.toml");
            if let Ok(mut manifest) = Manifest::from_file(&manifest_path) {
                let version_str = version.unwrap_or("*").to_string();
                manifest
                    .dependencies
                    .insert(package.to_string(), version_str);
                if let Err(e) = manifest.to_file(&manifest_path) {
                    output.warn(&format!("Failed to update hx.toml: {}", e));
                }
            }

            output.status("Added", &display_pkg);
            output.info("Run `hx lock` to update the lockfile.");
            Ok(0)
        }
        Err(CabalEditError::DependencyExists(_)) => {
            output.warn(&format!("Dependency '{}' already exists", package));
            Ok(0)
        }
        Err(CabalEditError::NoBuildDepends) => {
            output.error("No build-depends section found in .cabal file");
            output.info("Make sure your .cabal file has a build-depends section.");
            Ok(3)
        }
        Err(e) => {
            output.error(&format!("Failed to add dependency: {}", e));
            Ok(1)
        }
    }
}

/// Remove a dependency from the project.
pub async fn remove(package: &str, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(_) => {
            output.error("Not in an hx project (no hx.toml found)");
            output.info("Run `hx init` to create a new project first.");
            return Ok(3);
        }
    };

    // Find the .cabal file
    let cabal_file = find_cabal_file(&project_root);
    let cabal_file = match cabal_file {
        Some(path) => path,
        None => {
            output.error("No .cabal file found in project");
            output.info("Make sure your project has a .cabal file.");
            return Ok(3);
        }
    };

    output.status(
        "Removing",
        &format!(
            "{} from {}",
            package,
            cabal_file.file_name().unwrap().to_string_lossy()
        ),
    );

    match remove_dependency(&cabal_file, package) {
        Ok(()) => {
            // Also update hx.toml dependencies
            let manifest_path = project_root.join("hx.toml");
            if let Ok(mut manifest) = Manifest::from_file(&manifest_path) {
                manifest.dependencies.remove(package);
                if let Err(e) = manifest.to_file(&manifest_path) {
                    output.warn(&format!("Failed to update hx.toml: {}", e));
                }
            }

            output.status("Removed", package);
            output.info("Run `hx lock` to update the lockfile.");
            Ok(0)
        }
        Err(CabalEditError::DependencyNotFound(_)) => {
            output.warn(&format!("Dependency '{}' not found", package));
            Ok(0)
        }
        Err(CabalEditError::NoBuildDepends) => {
            output.error("No build-depends section found in .cabal file");
            Ok(3)
        }
        Err(e) => {
            output.error(&format!("Failed to remove dependency: {}", e));
            Ok(1)
        }
    }
}

/// Show why a package is a dependency.
pub async fn why(package: &str, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(_) => {
            output.error("Not in an hx project (no hx.toml found)");
            output.info("Run `hx init` to create a new project first.");
            return Ok(3);
        }
    };

    let project = Project::load(&project_root)?;

    // Load lockfile to get dependencies
    let lockfile_path = project.lockfile_path();
    let lockfile = match Lockfile::from_file(&lockfile_path) {
        Ok(lf) => lf,
        Err(_) => {
            output.error("No lockfile found");
            output.info("Run `hx lock` to generate a lockfile first.");
            return Ok(3);
        }
    };

    // Check if the package is in the lockfile
    let target_pkg = lockfile.packages.iter().find(|p| p.name == package);
    if target_pkg.is_none() {
        output.error(&format!("Package '{}' is not in the lockfile", package));
        output.info("This package is not a dependency of your project.");
        return Ok(1);
    }
    let target_pkg = target_pkg.unwrap();

    // Get direct dependencies from .cabal file
    let cabal_file = find_cabal_file(&project_root);
    let direct_deps = if let Some(cabal_path) = cabal_file {
        parse_direct_deps(&cabal_path)
    } else {
        // Fall back to hx.toml dependencies
        project.manifest.dependencies.keys().cloned().collect()
    };

    // Check if it's a direct dependency
    if direct_deps.contains(package) {
        println!("{} {}", package, target_pkg.version);
        println!("  └── (direct dependency)");
        return Ok(0);
    }

    // Try to build dependency graph from Hackage index
    let locked_names: Vec<String> = lockfile.packages.iter().map(|p| p.name.clone()).collect();

    // Load index with filtering for efficiency
    let dep_graph = match build_dep_graph_from_index(&lockfile, &locked_names) {
        Ok(graph) => graph,
        Err(e) => {
            output.warn(&format!("Could not load Hackage index: {}", e));
            // Fall back to showing it's a transitive dependency
            println!("{} {}", package, target_pkg.version);
            println!("  └── (transitive dependency)");
            output.info("Run `cabal update` to download the package index for detailed paths.");
            return Ok(0);
        }
    };

    // Find all paths from direct dependencies to the target package
    let paths = find_dependency_paths(&direct_deps, package, &dep_graph);

    // Display the result
    println!("{} {}", package, target_pkg.version);

    if paths.is_empty() {
        println!("  └── (transitive dependency, path unknown)");
    } else {
        for (i, path) in paths.iter().enumerate() {
            let is_last = i == paths.len() - 1;
            let prefix = if is_last { "└── " } else { "├── " };

            // Format the path: direct_dep -> intermediate -> ... -> target
            let path_str = path
                .iter()
                .map(|p| {
                    let version = dep_graph.versions.get(p).map(|v| v.as_str()).unwrap_or("?");
                    if direct_deps.contains(p) {
                        format!("{} {} (direct)", p, version)
                    } else {
                        format!("{} {}", p, version)
                    }
                })
                .collect::<Vec<_>>()
                .join(" → ");

            println!("  {}{}", prefix, path_str);
        }
    }

    Ok(0)
}

/// Parse direct dependencies from a .cabal file.
fn parse_direct_deps(cabal_path: &Path) -> HashSet<String> {
    let mut deps = HashSet::new();

    if let Ok(content) = fs::read_to_string(cabal_path) {
        // Simple parser: look for build-depends lines
        let mut in_build_depends = false;

        for line in content.lines() {
            let trimmed = line.trim().to_lowercase();

            if trimmed.starts_with("build-depends:") {
                in_build_depends = true;
                // Parse deps on the same line
                if let Some(deps_part) = line.split(':').nth(1) {
                    for dep in parse_dep_list(deps_part) {
                        deps.insert(dep);
                    }
                }
            } else if in_build_depends {
                // Check if this is a continuation line
                if line.starts_with(' ') || line.starts_with('\t') {
                    for dep in parse_dep_list(line) {
                        deps.insert(dep);
                    }
                } else if !trimmed.is_empty() {
                    in_build_depends = false;
                }
            }
        }
    }

    deps
}

/// Parse a comma-separated list of dependencies.
fn parse_dep_list(s: &str) -> Vec<String> {
    s.split(',')
        .filter_map(|part| {
            let part = part.trim();
            // Extract package name (before any version constraint)
            let name = part
                .split(|c: char| c.is_whitespace() || c == '>' || c == '<' || c == '=' || c == '^')
                .next()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())?;
            Some(name.to_string())
        })
        .collect()
}

/// Dependency graph with version information.
struct WhyDepGraph {
    /// Package name -> version
    versions: HashMap<String, String>,
    /// Package name -> its dependencies (used for forward traversal)
    #[allow(dead_code)]
    deps: HashMap<String, Vec<String>>,
    /// Reverse map: package name -> packages that depend on it
    reverse_deps: HashMap<String, Vec<String>>,
}

/// Build dependency graph from Hackage index.
fn build_dep_graph_from_index(
    lockfile: &Lockfile,
    filter_packages: &[String],
) -> Result<WhyDepGraph> {
    let index_path = best_index_path().ok_or_else(|| anyhow::anyhow!("Hackage index not found"))?;

    let options = IndexOptions {
        filter_packages: filter_packages.to_vec(),
        skip_errors: true,
        show_progress: false,
        ..Default::default()
    };

    let index = load_index(&index_path, &options)?;

    let mut versions = HashMap::new();
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();

    // Build maps from lockfile packages and their dependencies
    for pkg in &lockfile.packages {
        versions.insert(pkg.name.clone(), pkg.version.clone());
        deps.entry(pkg.name.clone()).or_default();
        reverse_deps.entry(pkg.name.clone()).or_default();
    }

    // Look up dependencies for each locked package from the index
    for pkg in &lockfile.packages {
        if let Some(index_pkg) = index.packages.get(&pkg.name) {
            // Try to find the exact version, or the closest one
            let version: Option<hx_solver::Version> = pkg.version.parse().ok();
            let pkg_version = version
                .as_ref()
                .and_then(|v| index_pkg.get_version(v))
                .or_else(|| index_pkg.versions.values().next());

            if let Some(pv) = pkg_version {
                for dep in &pv.dependencies {
                    // Only track dependencies that are in our lockfile
                    if versions.contains_key(&dep.name) {
                        deps.entry(pkg.name.clone())
                            .or_default()
                            .push(dep.name.clone());
                        reverse_deps
                            .entry(dep.name.clone())
                            .or_default()
                            .push(pkg.name.clone());
                    }
                }
            }
        }
    }

    Ok(WhyDepGraph {
        versions,
        deps,
        reverse_deps,
    })
}

/// Find all paths from direct dependencies to the target package.
fn find_dependency_paths(
    direct_deps: &HashSet<String>,
    target: &str,
    graph: &WhyDepGraph,
) -> Vec<Vec<String>> {
    let mut paths = Vec::new();

    // Use BFS from target back to direct dependencies via reverse_deps
    let mut queue: VecDeque<Vec<String>> = VecDeque::new();
    queue.push_back(vec![target.to_string()]);

    let mut visited_paths: HashSet<String> = HashSet::new();

    while let Some(current_path) = queue.pop_front() {
        let current = current_path.last().unwrap();

        // If we reached a direct dependency, we found a path
        if direct_deps.contains(current) {
            // Reverse the path so it goes from direct dep to target
            let mut path = current_path.clone();
            path.reverse();
            let path_key = path.join("->");
            if !visited_paths.contains(&path_key) {
                visited_paths.insert(path_key);
                paths.push(path);
            }
            continue;
        }

        // Explore reverse dependencies (packages that depend on current)
        if let Some(rdeps) = graph.reverse_deps.get(current) {
            for rdep in rdeps {
                // Avoid cycles
                if !current_path.contains(rdep) {
                    let mut new_path = current_path.clone();
                    new_path.push(rdep.clone());

                    // Limit path length to avoid explosion
                    if new_path.len() <= 10 {
                        queue.push_back(new_path);
                    }
                }
            }
        }
    }

    // Sort by path length (shorter paths first) and limit results
    paths.sort_by_key(|p| p.len());
    paths.truncate(5); // Show at most 5 paths

    paths
}

/// Check for outdated dependencies.
pub async fn outdated(direct_only: bool, _show_all: bool, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(_) => {
            output.error("Not in an hx project (no hx.toml found)");
            output.info("Run `hx init` to create a new project first.");
            return Ok(3);
        }
    };

    let project = Project::load(&project_root)?;

    // Load lockfile to get current versions
    let lockfile_path = project.lockfile_path();
    let lockfile = match Lockfile::from_file(&lockfile_path) {
        Ok(lf) => lf,
        Err(_) => {
            output.error("No lockfile found");
            output.info("Run `hx lock` to generate a lockfile first.");
            return Ok(3);
        }
    };

    if lockfile.packages.is_empty() {
        output.info("No dependencies in lockfile.");
        return Ok(0);
    }

    // Get direct dependencies from .cabal file
    let cabal_file = find_cabal_file(&project_root);
    let direct_deps = if let Some(cabal_path) = cabal_file {
        parse_direct_deps(&cabal_path)
    } else {
        project.manifest.dependencies.keys().cloned().collect()
    };

    // Load Hackage index
    let index_path = match best_index_path() {
        Some(path) => path,
        None => {
            output.error("Hackage index not found");
            output.info("Run `cabal update` to download the package index.");
            return Ok(1);
        }
    };

    output.status("Checking", "for outdated dependencies...");

    let locked_names: Vec<String> = lockfile.packages.iter().map(|p| p.name.clone()).collect();

    let options = IndexOptions {
        filter_packages: locked_names,
        skip_errors: true,
        show_progress: false,
        ..Default::default()
    };

    let index = match load_index(&index_path, &options) {
        Ok(idx) => idx,
        Err(e) => {
            output.error(&format!("Failed to load Hackage index: {}", e));
            return Ok(1);
        }
    };

    // Compare versions and collect outdated packages
    let mut outdated_packages: Vec<OutdatedPackage> = Vec::new();

    for pkg in &lockfile.packages {
        // Skip if we only want direct dependencies and this isn't one
        if direct_only && !direct_deps.contains(&pkg.name) {
            continue;
        }

        if let Some(index_pkg) = index.packages.get(&pkg.name) {
            // Get the latest version from the index
            if let Some(latest_version) = index_pkg.versions_descending().first() {
                let current: Option<hx_solver::Version> = pkg.version.parse().ok();

                if let Some(ref current_v) = current {
                    if *latest_version > current_v {
                        let update_type = categorize_update(current_v, latest_version);
                        outdated_packages.push(OutdatedPackage {
                            name: pkg.name.clone(),
                            current: pkg.version.clone(),
                            latest: latest_version.to_string(),
                            update_type,
                            is_direct: direct_deps.contains(&pkg.name),
                        });
                    }
                }
            }
        }
    }

    // Display results
    if outdated_packages.is_empty() {
        output.status(
            "Up to date",
            "All dependencies are at their latest versions.",
        );
        return Ok(0);
    }

    // Sort: direct deps first, then by name
    outdated_packages.sort_by(|a, b| match (a.is_direct, b.is_direct) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    // Calculate column widths for alignment
    let max_name_len = outdated_packages
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(0);
    let max_current_len = outdated_packages
        .iter()
        .map(|p| p.current.len())
        .max()
        .unwrap_or(0);
    let max_latest_len = outdated_packages
        .iter()
        .map(|p| p.latest.len())
        .max()
        .unwrap_or(0);

    println!();
    println!(
        "{:<width_name$}  {:<width_curr$}  →  {:<width_lat$}  Type",
        "Package",
        "Current",
        "Latest",
        width_name = max_name_len,
        width_curr = max_current_len,
        width_lat = max_latest_len,
    );
    println!(
        "{:-<width_name$}  {:-<width_curr$}     {:-<width_lat$}  -----",
        "",
        "",
        "",
        width_name = max_name_len,
        width_curr = max_current_len,
        width_lat = max_latest_len,
    );

    for pkg in &outdated_packages {
        let type_str = match pkg.update_type {
            UpdateType::Major => "major",
            UpdateType::Minor => "minor",
            UpdateType::Patch => "patch",
        };
        let direct_marker = if pkg.is_direct { " (direct)" } else { "" };

        println!(
            "{:<width_name$}  {:<width_curr$}  →  {:<width_lat$}  {}{}",
            pkg.name,
            pkg.current,
            pkg.latest,
            type_str,
            direct_marker,
            width_name = max_name_len,
            width_curr = max_current_len,
            width_lat = max_latest_len,
        );
    }

    println!();

    // Summary
    let major_count = outdated_packages
        .iter()
        .filter(|p| matches!(p.update_type, UpdateType::Major))
        .count();
    let minor_count = outdated_packages
        .iter()
        .filter(|p| matches!(p.update_type, UpdateType::Minor))
        .count();
    let patch_count = outdated_packages
        .iter()
        .filter(|p| matches!(p.update_type, UpdateType::Patch))
        .count();

    output.info(&format!(
        "Found {} outdated package(s): {} major, {} minor, {} patch",
        outdated_packages.len(),
        major_count,
        minor_count,
        patch_count
    ));

    // Show update hints
    if !outdated_packages.is_empty() {
        let direct_outdated: Vec<_> = outdated_packages.iter().filter(|p| p.is_direct).collect();
        if !direct_outdated.is_empty() {
            output.info("To update a dependency, run: hx add <package>");
        }
    }

    Ok(0)
}

/// Information about an outdated package.
struct OutdatedPackage {
    name: String,
    current: String,
    latest: String,
    update_type: UpdateType,
    is_direct: bool,
}

/// Type of version update.
#[derive(Debug, Clone, Copy)]
enum UpdateType {
    Major,
    Minor,
    Patch,
}

/// Categorize the type of update between two versions.
fn categorize_update(current: &hx_solver::Version, latest: &hx_solver::Version) -> UpdateType {
    let current_parts: Vec<u32> = current
        .to_string()
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    let latest_parts: Vec<u32> = latest
        .to_string()
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    let current_major = current_parts.first().copied().unwrap_or(0);
    let current_minor = current_parts.get(1).copied().unwrap_or(0);

    let latest_major = latest_parts.first().copied().unwrap_or(0);
    let latest_minor = latest_parts.get(1).copied().unwrap_or(0);

    if latest_major > current_major {
        UpdateType::Major
    } else if latest_minor > current_minor {
        UpdateType::Minor
    } else {
        UpdateType::Patch
    }
}

/// Update dependencies to their latest versions.
pub async fn update(
    packages: Vec<String>,
    direct_only: bool,
    dry_run: bool,
    allow_major: bool,
    output: &Output,
) -> Result<i32> {
    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(e) => {
            output.error(&format!("Failed to find project root: {}", e));
            return Ok(1);
        }
    };

    let project = Project::load(&project_root)?;

    // Load lockfile
    let lockfile_path = project.lockfile_path();
    let lockfile = match Lockfile::from_file(&lockfile_path) {
        Ok(lf) => lf,
        Err(_) => {
            output.error("No lockfile found");
            output.info("Run `hx lock` to generate a lockfile first.");
            return Ok(1);
        }
    };

    if lockfile.packages.is_empty() {
        output.info("No dependencies in lockfile.");
        return Ok(0);
    }

    // Get direct dependencies from .cabal file
    let cabal_file = find_cabal_file(&project_root);
    let direct_deps = if let Some(ref cabal_path) = cabal_file {
        parse_direct_deps(cabal_path)
    } else {
        project.manifest.dependencies.keys().cloned().collect()
    };

    // Load Hackage index
    let index_path = match best_index_path() {
        Some(path) => path,
        None => {
            output.error("Hackage index not found");
            output.info("Run `cabal update` to download the package index.");
            return Ok(1);
        }
    };

    output.status("Checking", "for updates...");

    let locked_names: Vec<String> = lockfile.packages.iter().map(|p| p.name.clone()).collect();

    let options = IndexOptions {
        filter_packages: locked_names,
        skip_errors: true,
        show_progress: false,
        ..Default::default()
    };

    let index = match load_index(&index_path, &options) {
        Ok(idx) => idx,
        Err(e) => {
            output.error(&format!("Failed to load Hackage index: {}", e));
            return Ok(1);
        }
    };

    // Find packages to update
    let mut updates: Vec<UpdateInfo> = Vec::new();

    for pkg in &lockfile.packages {
        // Skip if specific packages requested and this isn't one of them
        if !packages.is_empty() && !packages.iter().any(|p| p.eq_ignore_ascii_case(&pkg.name)) {
            continue;
        }

        let is_direct = direct_deps.contains(&pkg.name);

        // Skip transitive deps if --direct flag is set
        if direct_only && !is_direct {
            continue;
        }

        // Check for newer version
        if let Some(index_pkg) = index.packages.get(&pkg.name) {
            if let Some(latest_version) = index_pkg.versions_descending().first() {
                let current: Option<hx_solver::Version> = pkg.version.parse().ok();

                if let Some(ref current_v) = current {
                    if *latest_version > current_v {
                        let update_type = categorize_update(current_v, latest_version);

                        // Skip major updates unless --major flag is set
                        if matches!(update_type, UpdateType::Major) && !allow_major {
                            continue;
                        }

                        updates.push(UpdateInfo {
                            name: pkg.name.clone(),
                            current: pkg.version.clone(),
                            latest: latest_version.to_string(),
                            update_type,
                            is_direct,
                        });
                    }
                }
            }
        }
    }

    if updates.is_empty() {
        output.status(
            "Up to date",
            "All dependencies are at their latest versions.",
        );
        return Ok(0);
    }

    // Sort updates: direct deps first, then by name
    updates.sort_by(|a, b| match (a.is_direct, b.is_direct) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    if dry_run {
        output.status("Dry run", "showing what would be updated");
        println!();

        for update in &updates {
            let type_str = match update.update_type {
                UpdateType::Major => "major",
                UpdateType::Minor => "minor",
                UpdateType::Patch => "patch",
            };
            let direct_marker = if update.is_direct { " (direct)" } else { "" };
            println!(
                "  {} {} → {} ({}{})",
                update.name, update.current, update.latest, type_str, direct_marker
            );
        }

        println!();
        output.info(&format!("Would update {} package(s)", updates.len()));
        output.info("Run without --dry-run to apply updates");
        return Ok(0);
    }

    // Apply updates
    output.status("Updating", &format!("{} package(s)", updates.len()));

    let mut updated_count = 0;

    // Update cabal file if we have one
    if let Some(ref cabal_path) = cabal_file {
        let mut cabal_content = std::fs::read_to_string(cabal_path)?;

        for update in &updates {
            // Only update direct dependencies in cabal file
            if update.is_direct {
                // Update version constraint in cabal file
                let updated =
                    update_dependency_version(&mut cabal_content, &update.name, &update.latest);
                if updated {
                    output.info(&format!(
                        "  {} {} → {}",
                        update.name, update.current, update.latest
                    ));
                    updated_count += 1;
                }
            }
        }

        // Write updated cabal file
        if updated_count > 0 {
            std::fs::write(cabal_path, &cabal_content)?;
        }
    }

    // Also update hx.toml if it has dependencies
    let hx_toml_path = project_root.join("hx.toml");
    if hx_toml_path.exists() {
        let hx_content = std::fs::read_to_string(&hx_toml_path)?;
        let mut hx_updated = hx_content.clone();

        for update in &updates {
            if update.is_direct {
                // Try to update in hx.toml
                update_hx_toml_dep(&mut hx_updated, &update.name, &update.latest);
            }
        }

        if hx_updated != hx_content {
            std::fs::write(&hx_toml_path, &hx_updated)?;
        }
    }

    println!();
    output.status("Updated", &format!("{} package(s)", updated_count));
    output.info("Run `hx lock` to regenerate the lockfile");

    Ok(0)
}

/// Information about a package update.
struct UpdateInfo {
    name: String,
    current: String,
    latest: String,
    update_type: UpdateType,
    is_direct: bool,
}

/// Update a dependency version in the cabal file content.
fn update_dependency_version(content: &mut String, package: &str, new_version: &str) -> bool {
    // Pattern: package >=x.y.z or package ^>=x.y.z or package ==x.y.z
    // We'll update to ^>=new_version (compatible version)
    let patterns = [
        format!(r"(\b{}\s*)(>=\s*[\d.]+)", regex::escape(package)),
        format!(r"(\b{}\s*)(\^>=\s*[\d.]+)", regex::escape(package)),
        format!(r"(\b{}\s*)(==\s*[\d.]+)", regex::escape(package)),
        format!(r"(\b{}\s*)(>\s*[\d.]+)", regex::escape(package)),
        format!(r"(\b{}\s*)(<\s*[\d.]+)", regex::escape(package)),
    ];

    for pattern in &patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            if re.is_match(content) {
                let replacement = format!("${{1}}^>={}", new_version);
                *content = re.replace(content, replacement.as_str()).to_string();
                return true;
            }
        }
    }

    // If no version constraint, try to add one after bare package name
    // Match package name at word boundary followed by comma or newline
    let bare_pattern = format!(r"(\b{})(\s*,|\s*\n)", regex::escape(package));
    if let Ok(re) = regex::Regex::new(&bare_pattern) {
        if re.is_match(content) {
            let replacement = format!("${{1}} ^>={}${{2}}", new_version);
            *content = re.replace(content, replacement.as_str()).to_string();
            return true;
        }
    }

    false
}

/// Update a dependency in hx.toml.
fn update_hx_toml_dep(content: &mut String, package: &str, new_version: &str) {
    // Simple pattern matching for hx.toml dependency entries
    // Format: package = ">=x.y.z" or package = "^>=x.y.z"
    let patterns = [
        format!(r#"({}\s*=\s*")(>=[\d.]+)""#, regex::escape(package)),
        format!(r#"({}\s*=\s*")(\^>=[\d.]+)""#, regex::escape(package)),
        format!(r#"({}\s*=\s*")(==[\d.]+)""#, regex::escape(package)),
    ];

    for pattern in &patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            if re.is_match(content) {
                let replacement = format!(r#"${{1}}^>={}""#, new_version);
                *content = re.replace(content, replacement.as_str()).to_string();
                return;
            }
        }
    }
}

/// Show package information from Hackage.
pub async fn info(package: &str, show_versions: bool, output: &Output) -> Result<i32> {
    // Load Hackage index
    let index_path = match best_index_path() {
        Some(path) => path,
        None => {
            output.error("Hackage index not found");
            output.info("Run `cabal update` to download the package index.");
            return Ok(1);
        }
    };

    output.status("Looking up", package);

    let options = IndexOptions {
        filter_packages: vec![package.to_string()],
        skip_errors: true,
        show_progress: false,
        ..Default::default()
    };

    let index = match load_index(&index_path, &options) {
        Ok(idx) => idx,
        Err(e) => {
            output.error(&format!("Failed to load Hackage index: {}", e));
            return Ok(1);
        }
    };

    // Find the package
    let pkg = match index.packages.get(package) {
        Some(p) => p,
        None => {
            output.error(&format!("Package '{}' not found in Hackage index", package));
            output.info("Try running `hx search` to find similar packages.");
            return Ok(1);
        }
    };

    // Get versions
    let versions = pkg.versions_descending();
    let latest_version = versions.first().copied();

    println!();
    println!(
        "  {} {}",
        package,
        latest_version.map(|v| v.to_string()).unwrap_or_default()
    );
    println!();

    // Try to fetch additional metadata from Hackage
    if let Some(metadata) = fetch_hackage_metadata(package).await {
        if let Some(synopsis) = &metadata.synopsis {
            if !synopsis.is_empty() {
                println!("  {}", synopsis);
                println!();
            }
        }

        if let Some(license) = &metadata.license {
            println!("  License:     {}", license);
        }
        if let Some(author) = &metadata.author {
            if !author.is_empty() {
                println!("  Author:      {}", author);
            }
        }
        if let Some(maintainer) = &metadata.maintainer {
            if !maintainer.is_empty() {
                println!("  Maintainer:  {}", maintainer);
            }
        }
        if let Some(homepage) = &metadata.homepage {
            if !homepage.is_empty() {
                println!("  Homepage:    {}", homepage);
            }
        }
        println!(
            "  Hackage:     https://hackage.haskell.org/package/{}",
            package
        );
        println!();
    } else {
        println!(
            "  Hackage:     https://hackage.haskell.org/package/{}",
            package
        );
        println!();
    }

    // Show dependencies for latest version
    if let Some(latest) = latest_version {
        if let Some(pv) = pkg.get_version(latest) {
            if !pv.dependencies.is_empty() {
                println!("  Dependencies ({}):", pv.dependencies.len());
                for dep in &pv.dependencies {
                    println!("    - {}", dep.name);
                }
                println!();
            }
        }
    }

    // Show versions
    if show_versions {
        println!("  All versions ({}):", versions.len());
        for (i, v) in versions.iter().enumerate() {
            if i < 20 {
                println!("    {}", v);
            } else {
                println!("    ... and {} more", versions.len() - 20);
                break;
            }
        }
        println!();
    } else if versions.len() > 1 {
        let recent: Vec<_> = versions.iter().take(5).collect();
        println!(
            "  Recent versions: {}",
            recent
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if versions.len() > 5 {
            println!("  ({} total, use --versions to see all)", versions.len());
        }
        println!();
    }

    Ok(0)
}

/// Metadata fetched from Hackage.
#[derive(Debug, Default)]
struct HackageMetadata {
    synopsis: Option<String>,
    license: Option<String>,
    author: Option<String>,
    maintainer: Option<String>,
    homepage: Option<String>,
}

/// Fetch package metadata from Hackage API.
async fn fetch_hackage_metadata(package: &str) -> Option<HackageMetadata> {
    let url = format!(
        "https://hackage.haskell.org/package/{}/{}.cabal",
        package, package
    );

    let response = hx_core::http::client().ok()?.get(&url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    let content = response.text().await.ok()?;
    let mut metadata = HackageMetadata::default();

    for line in content.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("synopsis:") {
            metadata.synopsis = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("license:") {
            metadata.license = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("author:") {
            metadata.author = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("maintainer:") {
            metadata.maintainer = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("homepage:") {
            metadata.homepage = Some(value.trim().to_string());
        }
    }

    Some(metadata)
}

/// Show dependency graph.
pub async fn graph(
    format: GraphFormat,
    depth: usize,
    highlight: Option<String>,
    output_file: Option<String>,
    _dev: bool,
    direct_only: bool,
    output: &Output,
) -> Result<i32> {
    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(_) => {
            output.error("Not in an hx project (no hx.toml found)");
            return Ok(3);
        }
    };

    let project = Project::load(&project_root)?;

    // Load lockfile to get dependencies
    let lockfile_path = project.lockfile_path();
    let lockfile = match Lockfile::from_file(&lockfile_path) {
        Ok(lf) => lf,
        Err(_) => {
            output.error("No lockfile found");
            output.info("Run `hx lock` to generate a lockfile first.");
            return Ok(3);
        }
    };

    // Build dependency graph from lockfile
    let graph = build_dep_graph(&lockfile, direct_only);

    // Parse highlight packages
    let highlights: HashSet<String> = highlight
        .map(|h| h.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    // Generate output
    let result = match format {
        GraphFormat::Dot => generate_dot(&graph, &project, &highlights),
        GraphFormat::Tree => generate_tree(&graph, &project, depth, &highlights),
        GraphFormat::List => generate_list(&graph, &highlights),
        GraphFormat::Json => generate_json(&graph),
    };

    // Write output
    if let Some(file_path) = output_file {
        let mut file = fs::File::create(&file_path)?;
        file.write_all(result.as_bytes())?;
        output.status("Written", &file_path);
    } else {
        println!("{}", result);
    }

    Ok(0)
}

/// Dependency graph representation.
#[derive(Debug)]
struct DepGraph {
    /// Package name -> version
    packages: HashMap<String, String>,
    /// Package name -> dependencies
    edges: HashMap<String, Vec<String>>,
    /// Direct dependencies (from .cabal)
    direct: HashSet<String>,
}

/// Build dependency graph from lockfile.
fn build_dep_graph(lockfile: &Lockfile, direct_only: bool) -> DepGraph {
    let mut packages = HashMap::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut direct = HashSet::new();

    // Add all packages from lockfile
    for pkg in &lockfile.packages {
        packages.insert(pkg.name.clone(), pkg.version.clone());
        edges.entry(pkg.name.clone()).or_default();
    }

    // For now, we don't have full dependency edges in the lockfile,
    // so we'll mark all as direct dependencies
    // In a full implementation, we'd parse the .cabal file or use resolver data
    for pkg in &lockfile.packages {
        direct.insert(pkg.name.clone());
    }

    if direct_only {
        // Filter to only direct dependencies
        let direct_pkgs: HashSet<_> = direct.iter().cloned().collect();
        packages.retain(|k, _| direct_pkgs.contains(k));
        edges.retain(|k, _| direct_pkgs.contains(k));
    }

    DepGraph {
        packages,
        edges,
        direct,
    }
}

/// Generate Graphviz DOT format.
fn generate_dot(graph: &DepGraph, project: &Project, highlights: &HashSet<String>) -> String {
    let mut dot = String::new();

    dot.push_str("digraph dependencies {\n");
    dot.push_str("    rankdir=LR;\n");
    dot.push_str("    node [shape=box, fontname=\"Helvetica\"];\n");
    dot.push_str("    edge [color=\"#666666\"];\n\n");

    // Add project node
    dot.push_str(&format!(
        "    \"{}\" [label=\"{}\\n(project)\", style=filled, fillcolor=\"#4a90d9\", fontcolor=white];\n",
        project.name(),
        project.name()
    ));

    // Add package nodes
    for (name, version) in &graph.packages {
        let is_highlighted = highlights.contains(name);
        let is_direct = graph.direct.contains(name);

        let (fill_color, font_color) = if is_highlighted {
            ("#ffd700", "black") // Gold for highlighted
        } else if is_direct {
            ("#90EE90", "black") // Light green for direct
        } else {
            ("#f0f0f0", "black") // Light gray for transitive
        };

        dot.push_str(&format!(
            "    \"{}\" [label=\"{}\\n{}\", style=filled, fillcolor=\"{}\", fontcolor={}];\n",
            name, name, version, fill_color, font_color
        ));
    }

    dot.push('\n');

    // Add edges from project to direct dependencies
    for dep in &graph.direct {
        dot.push_str(&format!("    \"{}\" -> \"{}\";\n", project.name(), dep));
    }

    // Add edges between packages
    for (pkg, deps) in &graph.edges {
        for dep in deps {
            if graph.packages.contains_key(dep) {
                dot.push_str(&format!("    \"{}\" -> \"{}\";\n", pkg, dep));
            }
        }
    }

    dot.push_str("}\n");
    dot
}

/// Generate ASCII tree format.
fn generate_tree(
    graph: &DepGraph,
    project: &Project,
    max_depth: usize,
    highlights: &HashSet<String>,
) -> String {
    let mut result = String::new();

    // Project header
    result.push_str(&format!("{}\n", project.name()));

    // Sort dependencies for consistent output
    let mut deps: Vec<_> = graph.direct.iter().collect();
    deps.sort();

    let dep_count = deps.len();
    for (i, dep) in deps.iter().enumerate() {
        let is_last = i == dep_count - 1;
        let prefix = if is_last { "└── " } else { "├── " };
        let cont_prefix = if is_last { "    " } else { "│   " };

        let version = graph.packages.get(*dep).map(|v| v.as_str()).unwrap_or("?");
        let highlight_marker = if highlights.contains(*dep) { " *" } else { "" };

        result.push_str(&format!(
            "{}{} {}{}\n",
            prefix, dep, version, highlight_marker
        ));

        // Print transitive dependencies if depth allows
        if (max_depth == 0 || max_depth > 1)
            && let Some(trans_deps) = graph.edges.get(*dep)
        {
            print_tree_deps(
                &mut result,
                trans_deps,
                graph,
                highlights,
                cont_prefix,
                if max_depth == 0 { 0 } else { max_depth - 1 },
                1,
            );
        }
    }

    result
}

/// Recursively print tree dependencies.
fn print_tree_deps(
    result: &mut String,
    deps: &[String],
    graph: &DepGraph,
    highlights: &HashSet<String>,
    prefix: &str,
    max_depth: usize,
    current_depth: usize,
) {
    if max_depth > 0 && current_depth >= max_depth {
        return;
    }

    let mut sorted_deps: Vec<_> = deps
        .iter()
        .filter(|d| graph.packages.contains_key(*d))
        .collect();
    sorted_deps.sort();

    let dep_count = sorted_deps.len();
    for (i, dep) in sorted_deps.iter().enumerate() {
        let is_last = i == dep_count - 1;
        let branch = if is_last { "└── " } else { "├── " };
        let cont = if is_last { "    " } else { "│   " };

        let version = graph.packages.get(*dep).map(|v| v.as_str()).unwrap_or("?");
        let highlight_marker = if highlights.contains(*dep) { " *" } else { "" };

        result.push_str(&format!(
            "{}{}{} {}{}\n",
            prefix, branch, dep, version, highlight_marker
        ));

        // Recurse for transitive deps
        if let Some(trans_deps) = graph.edges.get(*dep) {
            let new_prefix = format!("{}{}", prefix, cont);
            print_tree_deps(
                result,
                trans_deps,
                graph,
                highlights,
                &new_prefix,
                max_depth,
                current_depth + 1,
            );
        }
    }
}

/// Generate simple list format.
fn generate_list(graph: &DepGraph, highlights: &HashSet<String>) -> String {
    let mut result = String::new();

    // Sort packages alphabetically
    let mut packages: Vec<_> = graph.packages.iter().collect();
    packages.sort_by(|a, b| a.0.cmp(b.0));

    for (name, version) in packages {
        let marker = if highlights.contains(name) { " *" } else { "" };
        let direct_marker = if graph.direct.contains(name) {
            " (direct)"
        } else {
            ""
        };
        result.push_str(&format!(
            "{} {}{}{}\n",
            name, version, direct_marker, marker
        ));
    }

    result
}

/// Generate JSON format.
fn generate_json(graph: &DepGraph) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"packages\": [\n");

    let mut packages: Vec<_> = graph.packages.iter().collect();
    packages.sort_by(|a, b| a.0.cmp(b.0));

    let pkg_count = packages.len();
    for (i, (name, version)) in packages.iter().enumerate() {
        let deps = graph.edges.get(*name).cloned().unwrap_or_default();
        let deps_json: Vec<String> = deps.iter().map(|d| format!("\"{}\"", d)).collect();
        let is_direct = graph.direct.contains(*name);

        json.push_str("    {\n");
        json.push_str(&format!("      \"name\": \"{}\",\n", name));
        json.push_str(&format!("      \"version\": \"{}\",\n", version));
        json.push_str(&format!("      \"direct\": {},\n", is_direct));
        json.push_str(&format!(
            "      \"dependencies\": [{}]\n",
            deps_json.join(", ")
        ));
        json.push_str(&format!(
            "    }}{}",
            if i < pkg_count - 1 { ",\n" } else { "\n" }
        ));
    }

    json.push_str("  ]\n");
    json.push_str("}\n");
    json
}

/// Find a .cabal file in the given directory.
fn find_cabal_file(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "cabal") {
            return Some(path);
        }
    }
    None
}
