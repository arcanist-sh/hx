//! Native GHC build orchestration.
//!
//! This module provides direct GHC invocation for building Haskell projects
//! without going through Cabal. It uses module dependency analysis to compile
//! modules in the correct order with parallel compilation support.

use hx_core::{CommandOutput, CommandRunner, Error, Fix, Result};
use hx_solver::{ModuleGraph, ModuleInfo, build_module_graph};
use hx_ui::{Output, Progress, Spinner};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

// ============================================================================
// Package Information from ghc-pkg
// ============================================================================

/// Information about an installed package from ghc-pkg.
#[derive(Debug, Clone, Default)]
pub struct PackageInfo {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Package unique ID (e.g., text-2.0.1-abc123)
    pub id: String,
    /// Library directories containing object files
    pub library_dirs: Vec<PathBuf>,
    /// Haskell library file (e.g., libHStext-2.0.1.a)
    pub hs_libraries: Vec<String>,
    /// Extra C libraries needed
    pub extra_libraries: Vec<String>,
    /// Extra library directories for C libs
    pub extra_lib_dirs: Vec<PathBuf>,
    /// Include directories
    pub include_dirs: Vec<PathBuf>,
    /// Dependencies (package IDs)
    pub depends: Vec<String>,
    /// Whether this is exposed
    pub exposed: bool,
}

/// Query ghc-pkg for detailed package information.
pub async fn query_package_info(
    package_name: &str,
    package_dbs: &[PathBuf],
) -> Option<PackageInfo> {
    let runner = CommandRunner::new();

    // Build args with package databases
    let mut args = Vec::new();
    for db in package_dbs {
        args.push(format!("--package-db={}", db.display()));
    }
    args.push("describe".to_string());
    args.push(package_name.to_string());

    let output = runner
        .run("ghc-pkg", args.iter().map(|s| s.as_str()))
        .await
        .ok()?;

    if !output.success() {
        debug!(
            "ghc-pkg describe {} failed: {}",
            package_name, output.stderr
        );
        return None;
    }

    Some(parse_package_info(&output.stdout))
}

/// Query all installed packages from ghc-pkg.
pub async fn query_all_packages(package_dbs: &[PathBuf]) -> Vec<PackageInfo> {
    let runner = CommandRunner::new();

    // Build args with package databases
    let mut args = Vec::new();
    for db in package_dbs {
        args.push(format!("--package-db={}", db.display()));
    }
    args.push("dump".to_string());

    let output = match runner.run("ghc-pkg", args.iter().map(|s| s.as_str())).await {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.success() {
        return Vec::new();
    }

    parse_package_dump(&output.stdout)
}

/// Parse ghc-pkg describe output into PackageInfo.
fn parse_package_info(output: &str) -> PackageInfo {
    let mut info = PackageInfo::default();

    for line in output.lines() {
        let line = line.trim();

        if let Some(value) = line.strip_prefix("name:") {
            info.name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("version:") {
            info.version = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("id:") {
            info.id = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("library-dirs:") {
            info.library_dirs = parse_path_list(value);
        } else if let Some(value) = line.strip_prefix("hs-libraries:") {
            info.hs_libraries = parse_string_list(value);
        } else if let Some(value) = line.strip_prefix("extra-libraries:") {
            info.extra_libraries = parse_string_list(value);
        } else if let Some(value) = line.strip_prefix("extra-lib-dirs:") {
            info.extra_lib_dirs = parse_path_list(value);
        } else if let Some(value) = line.strip_prefix("include-dirs:") {
            info.include_dirs = parse_path_list(value);
        } else if let Some(value) = line.strip_prefix("depends:") {
            info.depends = parse_string_list(value);
        } else if let Some(value) = line.strip_prefix("exposed:") {
            info.exposed = value.trim().eq_ignore_ascii_case("true");
        }
    }

    info
}

/// Parse ghc-pkg dump output into multiple PackageInfo entries.
fn parse_package_dump(output: &str) -> Vec<PackageInfo> {
    let mut packages = Vec::new();
    let mut current_block = String::new();

    for line in output.lines() {
        if line == "---" {
            if !current_block.is_empty() {
                packages.push(parse_package_info(&current_block));
                current_block.clear();
            }
        } else {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    // Don't forget the last block
    if !current_block.is_empty() {
        packages.push(parse_package_info(&current_block));
    }

    packages
}

/// Parse a space-separated list of paths.
fn parse_path_list(value: &str) -> Vec<PathBuf> {
    value
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Parse a space-separated list of strings.
fn parse_string_list(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Resolve transitive dependencies for a set of packages.
pub async fn resolve_transitive_deps(
    package_names: &[String],
    package_dbs: &[PathBuf],
) -> Vec<PackageInfo> {
    let all_packages = query_all_packages(package_dbs).await;

    // Build a map for quick lookup
    let pkg_map: HashMap<&str, &PackageInfo> = all_packages
        .iter()
        .flat_map(|p| {
            // Index by both name and full ID
            vec![(p.name.as_str(), p), (p.id.as_str(), p)]
        })
        .collect();

    // Collect all needed packages via DFS
    let mut needed: HashSet<String> = HashSet::new();
    let mut to_visit: Vec<String> = package_names.to_vec();

    while let Some(pkg_name) = to_visit.pop() {
        if needed.contains(&pkg_name) {
            continue;
        }

        if let Some(info) = pkg_map.get(pkg_name.as_str()) {
            needed.insert(info.id.clone());
            for dep in &info.depends {
                if !needed.contains(dep) {
                    to_visit.push(dep.clone());
                }
            }
        }
    }

    // Return the full info for all needed packages
    all_packages
        .into_iter()
        .filter(|p| needed.contains(&p.id))
        .collect()
}

/// Get all library paths needed for linking.
pub fn collect_library_paths(packages: &[PackageInfo]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = packages
        .iter()
        .flat_map(|p| p.library_dirs.iter().cloned())
        .collect();

    // Add extra lib dirs too
    paths.extend(
        packages
            .iter()
            .flat_map(|p| p.extra_lib_dirs.iter().cloned()),
    );

    // Deduplicate while preserving order
    let mut seen = HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));

    paths
}

/// Get all Haskell libraries needed for linking.
pub fn collect_hs_libraries(packages: &[PackageInfo]) -> Vec<String> {
    packages
        .iter()
        .flat_map(|p| p.hs_libraries.iter().cloned())
        .collect()
}

/// Get all extra C libraries needed for linking.
pub fn collect_extra_libraries(packages: &[PackageInfo]) -> Vec<String> {
    let mut libs: Vec<String> = packages
        .iter()
        .flat_map(|p| p.extra_libraries.iter().cloned())
        .collect();

    // Deduplicate while preserving order
    let mut seen = HashSet::new();
    libs.retain(|l| seen.insert(l.clone()));

    libs
}

// ============================================================================
// Main Module Detection
// ============================================================================

/// Detect the main module from a module graph by scanning for `main` function.
///
/// This function scans modules for patterns like:
/// - `main :: IO ()`
/// - `main = `
///
/// Returns the module name if a main function is found.
pub fn detect_main_module(graph: &ModuleGraph) -> Option<String> {
    // First check if there's a module named "Main" - this is the convention
    if let Some(main_module) = graph.modules.get("Main")
        && has_main_function(&main_module.path)
    {
        return Some("Main".to_string());
    }

    // Otherwise scan all modules for a main function
    for (module_name, info) in &graph.modules {
        if has_main_function(&info.path) {
            return Some(module_name.clone());
        }
    }

    None
}

/// Check if a Haskell source file contains a main function definition.
fn has_main_function(path: &Path) -> bool {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Look for main function patterns
    for line in content.lines() {
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with("--") || trimmed.starts_with("{-") {
            continue;
        }

        // Check for main type signature
        if trimmed.starts_with("main ::") {
            return true;
        }

        // Check for main definition (must be at start of line, not indented)
        // This avoids matching things like "doMain = " or "mainHelper ="
        if line.starts_with("main ")
            && (trimmed.starts_with("main =") || trimmed.starts_with("main="))
        {
            return true;
        }

        // Handle case where main appears with type annotation on same line
        if trimmed.starts_with("main ::") && trimmed.contains("IO") {
            return true;
        }
    }

    false
}

/// List all potential main modules (modules containing a `main` function).
pub fn find_all_main_modules(graph: &ModuleGraph) -> Vec<String> {
    graph
        .modules
        .iter()
        .filter_map(|(name, info)| {
            if has_main_function(&info.path) {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect()
}

// ============================================================================
// Module State Tracking for Incremental Builds
// ============================================================================

/// State of a single module for incremental build tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleState {
    /// SHA256 hash of the source file content
    pub source_hash: String,
    /// Modification time (nanoseconds since the Unix epoch)
    pub mtime: u64,
    /// Source file size in bytes. Together with `mtime` this is a cheap
    /// change signal that lets us skip hashing unchanged files.
    #[serde(default)]
    pub size: u64,
    /// Hash of interface files this module depends on
    pub deps_hash: String,
    /// Last successful compile timestamp
    pub compiled_at: u64,
}

/// Build state for incremental builds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildState {
    /// GHC version used
    pub ghc_version: String,
    /// Build flags hash
    pub flags_hash: String,
    /// Per-module state
    pub modules: HashMap<String, ModuleState>,
}

impl BuildState {
    /// Load build state from disk.
    pub fn load(project_root: &Path) -> Option<Self> {
        let path = project_root.join(".hx").join("native-build-state.json");
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Save build state to disk.
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let hx_dir = project_root.join(".hx");
        std::fs::create_dir_all(&hx_dir).map_err(|e| Error::Io {
            message: "failed to create .hx directory".to_string(),
            path: Some(hx_dir.clone()),
            source: e,
        })?;

        let path = hx_dir.join("native-build-state.json");
        let content = serde_json::to_string_pretty(self).map_err(|e| Error::Config {
            message: format!("failed to serialize build state: {}", e),
            path: None,
            source: None,
            fixes: vec![],
        })?;

        std::fs::write(&path, content).map_err(|e| Error::Io {
            message: "failed to write build state".to_string(),
            path: Some(path),
            source: e,
        })?;

        Ok(())
    }

    /// Check if a module needs recompilation.
    pub fn needs_rebuild(
        &self,
        module_name: &str,
        source_path: &Path,
        deps: &[String],
        ghc_version: &str,
        flags_hash: &str,
        output_dir: &Path,
    ) -> bool {
        // Rebuild if GHC version changed
        if self.ghc_version != ghc_version {
            debug!("Module {} needs rebuild: GHC version changed", module_name);
            return true;
        }

        // Rebuild if flags changed
        if self.flags_hash != flags_hash {
            debug!("Module {} needs rebuild: flags changed", module_name);
            return true;
        }

        // Get previous module state
        let state = match self.modules.get(module_name) {
            Some(s) => s,
            None => {
                debug!("Module {} needs rebuild: no previous state", module_name);
                return true;
            }
        };

        // Check if output files exist (in case build dir was cleaned)
        let obj_path = output_dir
            .join("o")
            .join(format!("{}.o", module_name.replace('.', "/")));
        let hi_path = output_dir
            .join("hi")
            .join(format!("{}.hi", module_name.replace('.', "/")));
        if !obj_path.exists() || !hi_path.exists() {
            debug!("Module {} needs rebuild: output files missing", module_name);
            return true;
        }

        // Check if the source file changed.
        //
        // Fast path: a `stat` (mtime + size) is microseconds; hashing reads
        // the whole file. If both mtime and size match the recorded values,
        // the source is unchanged and we skip hashing entirely. Only when
        // they differ do we fall back to a content hash — so a `touch`
        // (mtime bumped, identical content) still avoids a needless recompile.
        let unchanged_by_metadata = file_meta(source_path)
            .is_some_and(|(mtime, size)| mtime == state.mtime && size == state.size);
        if !unchanged_by_metadata {
            match compute_file_hash(source_path) {
                Ok(hash) if hash != state.source_hash => {
                    debug!("Module {} needs rebuild: source changed", module_name);
                    return true;
                }
                Err(_) => {
                    debug!("Module {} needs rebuild: cannot hash source", module_name);
                    return true;
                }
                _ => {}
            }
        }

        // Check if any dependency was recompiled more recently
        let deps_hash = compute_deps_hash(deps, &self.modules);
        if deps_hash != state.deps_hash {
            debug!("Module {} needs rebuild: deps changed", module_name);
            return true;
        }

        false
    }

    /// Update state for a successfully compiled module.
    pub fn mark_compiled(&mut self, module_name: &str, source_path: &Path, deps: &[String]) {
        let source_hash = compute_file_hash(source_path).unwrap_or_default();
        let (mtime, size) = file_meta(source_path).unwrap_or((0, 0));
        let deps_hash = compute_deps_hash(deps, &self.modules);
        let compiled_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.modules.insert(
            module_name.to_string(),
            ModuleState {
                source_hash,
                mtime,
                size,
                deps_hash,
                compiled_at,
            },
        );
    }
}

/// Compute SHA256 hash of a file.
fn compute_file_hash(path: &Path) -> std::io::Result<String> {
    let content = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Cheap source-change signal: `(mtime_nanos, size_bytes)` from a single
/// `stat`. Used to skip hashing files that haven't changed.
fn file_meta(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos() as u64;
    Some((mtime, meta.len()))
}

/// Compute hash of module dependencies.
fn compute_deps_hash(deps: &[String], module_states: &HashMap<String, ModuleState>) -> String {
    let mut hasher = Sha256::new();
    for dep in deps {
        if let Some(state) = module_states.get(dep) {
            hasher.update(dep.as_bytes());
            hasher.update(state.compiled_at.to_le_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}

/// Compute hash of build flags.
pub fn compute_flags_hash(options: &NativeBuildOptions) -> String {
    let mut hasher = Sha256::new();
    hasher.update(options.optimization.to_le_bytes());
    hasher.update(if options.warnings { &[1u8] } else { &[0u8] });
    hasher.update(if options.werror { &[1u8] } else { &[0u8] });
    for flag in &options.extra_flags {
        hasher.update(flag.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

// ============================================================================
// GHC Configuration
// ============================================================================

/// GHC compiler configuration.
#[derive(Debug, Clone)]
pub struct GhcConfig {
    /// Path to GHC executable (defaults to "ghc" on PATH)
    pub ghc_path: PathBuf,
    /// GHC version string
    pub version: String,
    /// Package databases to use
    pub package_dbs: Vec<PathBuf>,
    /// Packages to expose
    pub packages: Vec<String>,
    /// Resolved package information (populated by resolve_packages)
    pub resolved_packages: Vec<PackageInfo>,
}

impl Default for GhcConfig {
    fn default() -> Self {
        Self {
            ghc_path: PathBuf::from("ghc"),
            version: String::new(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            resolved_packages: Vec::new(),
        }
    }
}

impl GhcConfig {
    /// Create a new GHC config, detecting version from PATH.
    pub async fn detect() -> Result<Self> {
        Self::detect_with_path(&PathBuf::from("ghc")).await
    }

    /// Create a new GHC config with a specific GHC path.
    pub async fn detect_with_path(ghc_path: &Path) -> Result<Self> {
        let runner = CommandRunner::new();
        let ghc_str = ghc_path.display().to_string();
        let output = runner.run(ghc_str.as_str(), ["--numeric-version"]).await?;

        if !output.success() {
            return Err(Error::ToolchainMissing {
                tool: "ghc".to_string(),
                source: None,
                fixes: vec![],
            });
        }

        let version = output.stdout.trim().to_string();

        // Auto-detect package databases
        let package_dbs = detect_package_dbs(&version).await;

        Ok(Self {
            ghc_path: ghc_path.to_path_buf(),
            version,
            package_dbs,
            packages: Vec::new(),
            resolved_packages: Vec::new(),
        })
    }

    /// Add a package database.
    pub fn with_package_db(mut self, db: PathBuf) -> Self {
        self.package_dbs.push(db);
        self
    }

    /// Add a package to expose.
    pub fn with_package(mut self, pkg: impl Into<String>) -> Self {
        self.packages.push(pkg.into());
        self
    }

    /// Add multiple packages to expose.
    pub fn with_packages(mut self, pkgs: impl IntoIterator<Item = String>) -> Self {
        self.packages.extend(pkgs);
        self
    }

    /// Resolve all package dependencies and populate resolved_packages.
    pub async fn resolve_packages(&mut self) {
        if self.packages.is_empty() {
            return;
        }

        info!(
            "Resolving {} package dependencies via ghc-pkg",
            self.packages.len()
        );

        self.resolved_packages = resolve_transitive_deps(&self.packages, &self.package_dbs).await;

        info!(
            "Resolved {} packages (including transitive deps)",
            self.resolved_packages.len()
        );

        for pkg in &self.resolved_packages {
            debug!(
                "Package {}-{}: {} lib dirs, {} hs-libs, {} extra-libs",
                pkg.name,
                pkg.version,
                pkg.library_dirs.len(),
                pkg.hs_libraries.len(),
                pkg.extra_libraries.len()
            );
        }
    }

    /// Get all library paths needed for linking.
    pub fn library_paths(&self) -> Vec<PathBuf> {
        collect_library_paths(&self.resolved_packages)
    }

    /// Get all Haskell libraries needed for linking.
    pub fn hs_libraries(&self) -> Vec<String> {
        collect_hs_libraries(&self.resolved_packages)
    }

    /// Get all extra C libraries needed for linking.
    pub fn extra_libraries(&self) -> Vec<String> {
        collect_extra_libraries(&self.resolved_packages)
    }
}

/// Detect available GHC package databases.
async fn detect_package_dbs(ghc_version: &str) -> Vec<PathBuf> {
    let mut dbs = Vec::new();

    // Try to get package databases from ghc-pkg
    let runner = CommandRunner::new();
    if let Ok(output) = runner.run("ghc-pkg", ["list", "--global"]).await
        && output.success()
    {
        // First line is usually the database path
        if let Some(first_line) = output.stdout.lines().next() {
            let db_path = first_line.trim().trim_end_matches(':');
            if !db_path.is_empty() {
                let path = PathBuf::from(db_path);
                if path.exists() {
                    debug!("Found global package db: {}", path.display());
                    dbs.push(path);
                }
            }
        }
    }

    // Add cabal store package database
    if let Some(home) = dirs::home_dir() {
        // Standard cabal store location
        let cabal_store = home
            .join(".cabal")
            .join("store")
            .join(format!("ghc-{}", ghc_version))
            .join("package.db");
        if cabal_store.exists() {
            debug!("Found cabal store db: {}", cabal_store.display());
            dbs.push(cabal_store);
        }

        // hx-managed store location
        let hx_store = home
            .join(".cache")
            .join("hx")
            .join("cabal")
            .join("store")
            .join(format!("ghc-{}", ghc_version))
            .join("package.db");
        if hx_store.exists() {
            debug!("Found hx store db: {}", hx_store.display());
            dbs.push(hx_store);
        }
    }

    dbs
}

/// Get packages needed from a lockfile.
pub fn packages_from_lockfile(lockfile_path: &Path) -> Vec<String> {
    let content = match std::fs::read_to_string(lockfile_path) {
        Ok(c) => c,
        Err(_) => return vec!["base".to_string()],
    };

    // Parse the lockfile TOML to extract package names
    let mut packages = vec!["base".to_string()];

    // Simple parsing - look for [[packages]] sections
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("name = ") {
            let name = trimmed
                .trim_start_matches("name = ")
                .trim_matches('"')
                .to_string();
            if !name.is_empty() && !packages.contains(&name) {
                packages.push(name);
            }
        }
    }

    packages
}

/// Options for native GHC builds.
#[derive(Debug, Clone)]
pub struct NativeBuildOptions {
    /// Source directories to search
    pub src_dirs: Vec<PathBuf>,
    /// Output directory for .hi and .o files
    pub output_dir: PathBuf,
    /// GHC optimization level (0, 1, or 2)
    pub optimization: u8,
    /// Enable all warnings
    pub warnings: bool,
    /// Treat warnings as errors
    pub werror: bool,
    /// Additional GHC flags
    pub extra_flags: Vec<String>,
    /// Number of parallel jobs
    pub jobs: usize,
    /// Verbose output
    pub verbose: bool,
    /// Main module name (for executables)
    pub main_module: Option<String>,
    /// Output executable path
    pub output_exe: Option<PathBuf>,
    /// Output library archive path (.a file)
    pub output_lib: Option<PathBuf>,
    /// Use native linking (include resolved library paths)
    pub native_linking: bool,
    /// Target triple for cross-compilation (e.g., "aarch64-linux-gnu")
    pub target: Option<String>,
}

impl Default for NativeBuildOptions {
    fn default() -> Self {
        Self {
            src_dirs: vec![PathBuf::from("src")],
            output_dir: PathBuf::from(".hx/build"),
            optimization: 1,
            warnings: true,
            werror: false,
            extra_flags: Vec::new(),
            jobs: num_cpus::get(),
            verbose: false,
            main_module: None,
            output_exe: None,
            output_lib: None,
            native_linking: true,
            target: None,
        }
    }
}

/// Result of compiling a single module.
#[derive(Debug, Clone)]
pub struct ModuleCompileResult {
    /// Module name
    pub module: String,
    /// Whether compilation succeeded
    pub success: bool,
    /// Compilation duration
    pub duration: Duration,
    /// Path to object file (if successful)
    pub object_file: Option<PathBuf>,
    /// Path to interface file (if successful)
    pub interface_file: Option<PathBuf>,
    /// Warnings from compilation
    pub warnings: Vec<String>,
    /// Errors (if failed)
    pub errors: Vec<String>,
}

/// Result of a native build.
#[derive(Debug)]
pub struct NativeBuildResult {
    /// Whether the build succeeded
    pub success: bool,
    /// Total build duration
    pub duration: Duration,
    /// Modules compiled
    pub modules_compiled: usize,
    /// Modules skipped (already up to date)
    pub modules_skipped: usize,
    /// Individual module results
    pub module_results: Vec<ModuleCompileResult>,
    /// Output executable path (if linking)
    pub executable: Option<PathBuf>,
    /// Output library archive path (if building library)
    pub library: Option<PathBuf>,
    /// Errors
    pub errors: Vec<String>,
    /// Warnings
    pub warnings: Vec<String>,
}

/// Native GHC build orchestrator.
pub struct NativeBuilder {
    ghc: GhcConfig,
}

impl NativeBuilder {
    /// Create a new native builder.
    pub fn new(ghc: GhcConfig) -> Self {
        Self { ghc }
    }

    /// Create a native builder with auto-detected GHC.
    pub async fn detect() -> Result<Self> {
        let ghc = GhcConfig::detect().await?;
        Ok(Self::new(ghc))
    }

    /// Build a Haskell project.
    pub async fn build(
        &self,
        project_root: &Path,
        options: &NativeBuildOptions,
        output: &Output,
    ) -> Result<NativeBuildResult> {
        let start = Instant::now();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut module_results = Vec::new();

        // Build source directories list and warn about missing directories
        let mut src_dirs: Vec<PathBuf> = Vec::new();
        for dir in &options.src_dirs {
            let full_path = project_root.join(dir);
            if full_path.exists() {
                src_dirs.push(full_path);
            } else {
                warn!("Source directory does not exist: {}", full_path.display());
                warnings.push(format!("Source directory not found: {}", dir.display()));
            }
        }

        // Check if we have any valid source directories
        if src_dirs.is_empty() {
            return Err(Error::BuildFailed {
                errors: vec!["No valid source directories found".to_string()],
                fixes: vec![
                    Fix::new("Check that your source directories exist"),
                    Fix::new("Default source directory is 'src/' - create it if missing"),
                ],
            });
        }

        // Detect and run preprocessors
        let preproc_sources = crate::preprocessor::detect_preprocessors(&src_dirs);
        if !preproc_sources.is_empty() {
            info!(
                "Found {} files requiring preprocessing",
                preproc_sources.total_files()
            );

            // Check preprocessor availability
            let available =
                crate::preprocessor::check_availability(&preproc_sources, Some(&self.ghc.ghc_path))
                    .await?;

            // Configure preprocessing
            let generated_dir = project_root.join(&options.output_dir).join("generated");
            let preproc_config = crate::preprocessor::PreprocessorConfig {
                output_dir: generated_dir.clone(),
                verbose: options.verbose,
                ..Default::default()
            };

            // Run all preprocessors
            let preproc_results = crate::preprocessor::preprocess_all(
                &preproc_sources,
                &preproc_config,
                &self.ghc.ghc_path,
                &available,
            )
            .await?;

            // Collect warnings from preprocessing
            for result in &preproc_results {
                warnings.extend(result.warnings.clone());
            }

            // Add generated directory to source dirs
            if generated_dir.exists() {
                src_dirs.push(generated_dir);
            }
        }

        // Create updated options with full source directories (including generated)
        let options = NativeBuildOptions {
            src_dirs: src_dirs.clone(),
            ..options.clone()
        };

        info!("Building module graph from {:?}", src_dirs);
        let graph = build_module_graph(&src_dirs).map_err(|e| Error::BuildFailed {
            errors: vec![format!("Failed to build module graph: {}", e)],
            fixes: vec![],
        })?;

        let module_count = graph.modules.len();
        if module_count == 0 {
            output.info("No Haskell modules found");
            return Ok(NativeBuildResult {
                success: true,
                duration: start.elapsed(),
                modules_compiled: 0,
                modules_skipped: 0,
                module_results: vec![],
                executable: None,
                library: None,
                errors: vec![],
                warnings: vec![],
            });
        }

        info!("Found {} modules", module_count);

        // Auto-detect main module if not specified and building an executable
        let main_module = if options.main_module.is_some() {
            options.main_module.clone()
        } else if options.output_exe.is_some() {
            let detected = detect_main_module(&graph);
            if let Some(ref m) = detected {
                info!("Auto-detected main module: {}", m);
            } else {
                info!("No main module detected, will compile as library");
            }
            detected
        } else {
            None
        };

        // Create modified options with detected main module
        let options = if main_module != options.main_module {
            let mut opts = options.clone();
            opts.main_module = main_module;
            opts
        } else {
            options.clone()
        };

        // Get parallel compilation groups
        let groups = graph.parallel_groups().map_err(|e| Error::BuildFailed {
            errors: vec![format!("Cycle detected in modules: {}", e)],
            fixes: vec![],
        })?;

        // Ensure output directory exists
        let output_dir = project_root.join(&options.output_dir);
        std::fs::create_dir_all(&output_dir).map_err(|e| Error::Io {
            message: "failed to create build directory".to_string(),
            path: Some(output_dir.clone()),
            source: e,
        })?;

        // Load previous build state for incremental builds
        let mut build_state = BuildState::load(project_root).unwrap_or_default();
        let flags_hash = compute_flags_hash(&options);

        // Determine which modules need recompilation (and their dependents).
        let all_rebuild = compute_rebuild_set(
            &graph,
            &build_state,
            &self.ghc.version,
            &flags_hash,
            &output_dir,
        );

        let rebuild_count = all_rebuild.len();
        let skip_count = module_count - rebuild_count;

        if rebuild_count == 0 {
            output.info(&format!("All {} modules up to date", module_count));
            return Ok(NativeBuildResult {
                success: true,
                duration: start.elapsed(),
                modules_compiled: 0,
                modules_skipped: module_count,
                module_results: vec![],
                executable: options.output_exe.as_ref().map(|p| project_root.join(p)),
                library: options.output_lib.as_ref().map(|p| project_root.join(p)),
                errors: vec![],
                warnings: vec![],
            });
        }

        info!(
            "{} modules need recompilation, {} up to date",
            rebuild_count, skip_count
        );

        // Track compiled modules
        let mut compiled_modules = HashSet::new();
        let mut modules_compiled = 0;

        // Use progress bar for compilation (determinate progress)
        let progress = if !options.verbose && rebuild_count > 0 {
            Some(Progress::new(rebuild_count as u64, "Compiling"))
        } else {
            None
        };

        for (group_idx, group) in groups.iter().enumerate() {
            // Filter to only modules that need rebuild
            let group_to_compile: Vec<String> = group
                .iter()
                .filter(|m| all_rebuild.contains(*m))
                .cloned()
                .collect();

            if group_to_compile.is_empty() {
                continue;
            }

            debug!(
                "Compiling group {} with {} modules",
                group_idx,
                group_to_compile.len()
            );

            // Compile all modules in this group in parallel
            let group_results = self
                .compile_group(
                    &graph,
                    &group_to_compile,
                    project_root,
                    &options,
                    &compiled_modules,
                )
                .await?;

            for result in group_results {
                if result.success {
                    // Update build state for successfully compiled module
                    let deps = graph
                        .dependencies
                        .get(&result.module)
                        .map(|d| d.as_slice())
                        .unwrap_or(&[]);
                    if let Some(info) = graph.modules.get(&result.module) {
                        build_state.mark_compiled(&result.module, &info.path, deps);
                    }
                    compiled_modules.insert(result.module.clone());
                    modules_compiled += 1;

                    // Update progress bar
                    if let Some(ref p) = progress {
                        p.inc(1);
                    }
                } else {
                    errors.extend(result.errors.clone());
                }
                warnings.extend(result.warnings.clone());
                module_results.push(result);
            }

            // If any module failed, stop
            if !errors.is_empty() {
                break;
            }
        }

        // Finish progress bar
        if let Some(p) = progress {
            if errors.is_empty() {
                p.finish(format!(
                    "Compiled {} modules in {} ({} skipped)",
                    modules_compiled,
                    format_duration(start.elapsed()),
                    skip_count
                ));
            } else {
                // Progress bar will be dropped, but we show error separately
                drop(p);
            }
        }

        // Save build state if successful
        if errors.is_empty() {
            build_state.ghc_version = self.ghc.version.clone();
            build_state.flags_hash = flags_hash;
            if let Err(e) = build_state.save(project_root) {
                debug!("Failed to save build state: {}", e);
            }
        }

        // Link executable if requested and compilation succeeded
        let executable =
            if errors.is_empty() && options.main_module.is_some() && options.output_exe.is_some() {
                self.link(project_root, &graph, &options, &output_dir, output)
                    .await?
            } else {
                None
            };

        // Create library archive if requested and compilation succeeded
        let library = if errors.is_empty() && options.output_lib.is_some() {
            self.create_library(&graph, &options, &output_dir, output)
                .await?
        } else {
            None
        };

        Ok(NativeBuildResult {
            success: errors.is_empty(),
            duration: start.elapsed(),
            modules_compiled,
            modules_skipped: 0, // TODO: incremental
            module_results,
            executable,
            library,
            errors,
            warnings,
        })
    }

    /// Compile a group of independent modules in parallel.
    async fn compile_group(
        &self,
        graph: &ModuleGraph,
        modules: &[String],
        project_root: &Path,
        options: &NativeBuildOptions,
        _compiled: &HashSet<String>,
    ) -> Result<Vec<ModuleCompileResult>> {
        let semaphore = Arc::new(Semaphore::new(options.jobs));
        let mut handles = Vec::new();

        for module_name in modules {
            let module_info = match graph.modules.get(module_name) {
                Some(info) => info.clone(),
                None => continue,
            };

            let sem = Arc::clone(&semaphore);
            let ghc = self.ghc.clone();
            let project_root = project_root.to_path_buf();
            let options = options.clone();

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                compile_module(&ghc, &module_info, &project_root, &options).await
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    results.push(ModuleCompileResult {
                        module: "unknown".to_string(),
                        success: false,
                        duration: Duration::ZERO,
                        object_file: None,
                        interface_file: None,
                        warnings: vec![],
                        errors: vec![format!("Task failed: {}", e)],
                    });
                }
            }
        }

        Ok(results)
    }

    /// Link compiled modules into an executable.
    async fn link(
        &self,
        project_root: &Path,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
        output_dir: &Path,
        output: &Output,
    ) -> Result<Option<PathBuf>> {
        let exe_path = match &options.output_exe {
            Some(p) => project_root.join(p),
            None => return Ok(None),
        };

        let main_module = match &options.main_module {
            Some(m) => m,
            None => return Ok(None),
        };

        info!("Linking executable: {:?}", exe_path);

        let spinner = if !options.verbose {
            Some(Spinner::new("Linking..."))
        } else {
            None
        };

        // Collect all object files from local modules
        // Object files are in the "o" subdirectory (set by -odir flag during compilation)
        let obj_dir = output_dir.join("o");
        let mut object_files = Vec::new();
        for module_name in graph.modules.keys() {
            let obj_file = obj_dir.join(format!("{}.o", module_name.replace('.', "/")));
            if obj_file.exists() {
                object_files.push(obj_file);
            }
        }

        // Build GHC link command
        let mut args: Vec<String> = vec!["-o".to_string(), exe_path.display().to_string()];

        // Add package databases
        for db in &self.ghc.package_dbs {
            args.push(format!("-package-db={}", db.display()));
        }

        // Add packages
        for pkg in &self.ghc.packages {
            args.push("-package".to_string());
            args.push(pkg.clone());
        }

        // Add library paths from resolved packages (for native linking)
        if options.native_linking {
            let lib_paths = self.ghc.library_paths();
            for path in &lib_paths {
                args.push(format!("-L{}", path.display()));
            }

            // Add extra C libraries
            let extra_libs = self.ghc.extra_libraries();
            for lib in &extra_libs {
                args.push(format!("-l{}", lib));
            }

            debug!(
                "Native linking with {} lib paths, {} extra libs",
                lib_paths.len(),
                extra_libs.len()
            );
        }

        // Add cross-compilation target if specified
        if let Some(ref target) = options.target {
            args.push(format!("-target={}", target));
        }

        // Add optimization
        if options.optimization > 0 {
            args.push(format!("-O{}", options.optimization));
        }

        // Add object files
        for obj in &object_files {
            args.push(obj.display().to_string());
        }

        // Add main module hint
        args.push("-main-is".to_string());
        args.push(main_module.clone());

        if options.verbose {
            info!("Link command: ghc {}", args.join(" "));
        }

        let runner = CommandRunner::new().with_working_dir(project_root);
        let ghc_path = self.ghc.ghc_path.display().to_string();
        let cmd_output = runner.run(&ghc_path, &args).await?;

        if let Some(spinner) = spinner {
            if cmd_output.success() {
                spinner.finish_success("Linked");
            } else {
                spinner.finish_error("Link failed");
            }
        }

        if !cmd_output.success() {
            output.info(&cmd_output.stderr);
            return Err(Error::BuildFailed {
                errors: vec!["Linking failed".to_string()],
                fixes: vec![],
            });
        }

        Ok(Some(exe_path))
    }

    /// Create a static library archive from compiled object files.
    async fn create_library(
        &self,
        graph: &ModuleGraph,
        options: &NativeBuildOptions,
        output_dir: &Path,
        output: &Output,
    ) -> Result<Option<PathBuf>> {
        let lib_path = match &options.output_lib {
            Some(p) => p.clone(),
            None => return Ok(None),
        };

        info!("Creating library archive: {:?}", lib_path);

        let spinner = if !options.verbose {
            Some(Spinner::new("Creating library..."))
        } else {
            None
        };

        // Collect all object files from local modules
        // Object files are in the "o" subdirectory (set by -odir flag during compilation)
        let obj_dir = output_dir.join("o");
        let mut object_files = Vec::new();
        for module_name in graph.modules.keys() {
            let obj_file = obj_dir.join(format!("{}.o", module_name.replace('.', "/")));
            if obj_file.exists() {
                object_files.push(obj_file);
            }
        }

        if object_files.is_empty() {
            if let Some(spinner) = spinner {
                spinner.finish_error("No object files found");
            }
            return Ok(None);
        }

        // Ensure parent directory exists
        if let Some(parent) = lib_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                message: "failed to create library output directory".to_string(),
                path: Some(parent.to_path_buf()),
                source: e,
            })?;
        }

        // Use 'ar' to create the static library
        // On macOS, use libtool; on Linux, use ar
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
        for obj in &object_files {
            full_args.push(obj.display().to_string());
        }

        if options.verbose {
            info!("Archive command: {} {}", archiver, full_args.join(" "));
        }

        let runner = CommandRunner::new();
        let cmd_output = runner
            .run(archiver, full_args.iter().map(|s| s.as_str()))
            .await?;

        if let Some(spinner) = spinner {
            if cmd_output.success() {
                spinner.finish_success(format!("Created {}", lib_path.display()));
            } else {
                spinner.finish_error("Archive creation failed");
            }
        }

        if !cmd_output.success() {
            output.info(&cmd_output.stderr);
            return Err(Error::BuildFailed {
                errors: vec!["Library archive creation failed".to_string()],
                fixes: vec![],
            });
        }

        Ok(Some(lib_path))
    }
}

/// Compile a single Haskell module.
async fn compile_module(
    ghc: &GhcConfig,
    module: &ModuleInfo,
    project_root: &Path,
    options: &NativeBuildOptions,
) -> ModuleCompileResult {
    let start = Instant::now();
    let module_name = &module.name;

    debug!("Compiling module: {}", module_name);

    // Build output paths
    let output_dir = project_root.join(&options.output_dir);
    let module_subdir = module_name.replace('.', "/");
    let hi_dir = output_dir.join("hi");
    let o_dir = output_dir.join("o");

    // Ensure directories exist
    let module_hi_dir = hi_dir.join(
        std::path::Path::new(&module_subdir)
            .parent()
            .unwrap_or(std::path::Path::new("")),
    );
    let module_o_dir = o_dir.join(
        std::path::Path::new(&module_subdir)
            .parent()
            .unwrap_or(std::path::Path::new("")),
    );

    if let Err(e) = std::fs::create_dir_all(&module_hi_dir) {
        return ModuleCompileResult {
            module: module_name.clone(),
            success: false,
            duration: start.elapsed(),
            object_file: None,
            interface_file: None,
            warnings: vec![],
            errors: vec![format!("Failed to create directory: {}", e)],
        };
    }
    if let Err(e) = std::fs::create_dir_all(&module_o_dir) {
        return ModuleCompileResult {
            module: module_name.clone(),
            success: false,
            duration: start.elapsed(),
            object_file: None,
            interface_file: None,
            warnings: vec![],
            errors: vec![format!("Failed to create directory: {}", e)],
        };
    }

    // Build GHC args
    let mut args: Vec<String> = vec![
        "-c".to_string(), // Compile only
        format!("-hidir={}", hi_dir.display()),
        format!("-odir={}", o_dir.display()),
    ];

    // Add source directories
    for src_dir in &options.src_dirs {
        args.push(format!("-i{}", project_root.join(src_dir).display()));
    }

    // Add package databases
    for db in &ghc.package_dbs {
        args.push(format!("-package-db={}", db.display()));
    }

    // Add packages
    for pkg in &ghc.packages {
        args.push("-package".to_string());
        args.push(pkg.clone());
    }

    // Add optimization
    if options.optimization > 0 {
        args.push(format!("-O{}", options.optimization));
    }

    // Add warnings
    if options.warnings {
        args.push("-Wall".to_string());
    }
    if options.werror {
        args.push("-Werror".to_string());
    }

    // Add extra flags
    args.extend(options.extra_flags.clone());

    // Add cross-compilation target if specified
    if let Some(ref target) = options.target {
        args.push(format!("-target={}", target));
    }

    // Add source file
    args.push(module.path.display().to_string());

    // Run GHC
    let runner = CommandRunner::new().with_working_dir(project_root);
    let ghc_path = ghc.ghc_path.display().to_string();
    let output = match runner.run(&ghc_path, &args).await {
        Ok(o) => o,
        Err(e) => {
            return ModuleCompileResult {
                module: module_name.clone(),
                success: false,
                duration: start.elapsed(),
                object_file: None,
                interface_file: None,
                warnings: vec![],
                errors: vec![format!("Failed to run GHC: {}", e)],
            };
        }
    };

    // Parse output
    let (warnings, errors) = parse_ghc_output(&output);

    let success = output.success();
    let object_file = if success {
        Some(o_dir.join(format!("{}.o", module_subdir)))
    } else {
        None
    };
    let interface_file = if success {
        Some(hi_dir.join(format!("{}.hi", module_subdir)))
    } else {
        None
    };

    ModuleCompileResult {
        module: module_name.clone(),
        success,
        duration: start.elapsed(),
        object_file,
        interface_file,
        warnings,
        errors,
    }
}

/// Parse GHC output into warnings and errors.
fn parse_ghc_output(output: &CommandOutput) -> (Vec<String>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    let combined = format!("{}\n{}", output.stdout, output.stderr);

    for line in combined.lines() {
        let trimmed = line.trim();
        if trimmed.contains(": error:") || trimmed.contains(": Error:") {
            errors.push(line.to_string());
        } else if trimmed.contains(": warning:") || trimmed.contains(": Warning:") {
            warnings.push(line.to_string());
        } else if !errors.is_empty() && (trimmed.starts_with(' ') || trimmed.starts_with('\t')) {
            // Continuation of previous error
            if let Some(last) = errors.last_mut() {
                last.push('\n');
                last.push_str(line);
            }
        }
    }

    (warnings, errors)
}

/// Collect all modules that depend on a given module (transitively).
/// Compute the set of modules that need recompilation, including the
/// dependents of any changed module. Shared by the incremental build and the
/// no-op short-circuit so the two can never disagree about staleness.
fn compute_rebuild_set(
    graph: &ModuleGraph,
    build_state: &BuildState,
    ghc_version: &str,
    flags_hash: &str,
    output_dir: &Path,
) -> HashSet<String> {
    let modules_needing_rebuild: HashSet<String> = graph
        .modules
        .iter()
        .filter(|(name, info)| {
            let deps = graph
                .dependencies
                .get(*name)
                .map(|d| d.as_slice())
                .unwrap_or(&[]);
            build_state.needs_rebuild(name, &info.path, deps, ghc_version, flags_hash, output_dir)
        })
        .map(|(name, _)| name.clone())
        .collect();

    let mut all_rebuild: HashSet<String> = modules_needing_rebuild.clone();
    for module_name in &modules_needing_rebuild {
        collect_dependents(graph, module_name, &mut all_rebuild);
    }
    all_rebuild
}

/// Cheap, subprocess-free check of whether a native build is already up to
/// date — every module's output exists and no source has changed. This lets
/// the CLI skip GHC/ghc-pkg toolchain resolution entirely on a no-op rebuild
/// (those subprocess spawns dominate the cost of an otherwise empty build).
pub fn native_build_up_to_date(
    project_root: &Path,
    src_dirs: &[PathBuf],
    output_dir: &Path,
    ghc_version: &str,
    flags_hash: &str,
) -> bool {
    // Projects with preprocessor inputs (.x/.y/.hsc/.chs) need generated
    // sources; don't fast-path those — let the full build produce them.
    if !crate::preprocessor::detect_preprocessors(src_dirs).is_empty() {
        return false;
    }
    let graph = match build_module_graph(src_dirs) {
        Ok(g) if !g.modules.is_empty() => g,
        _ => return false,
    };
    let Some(build_state) = BuildState::load(project_root) else {
        return false;
    };
    compute_rebuild_set(&graph, &build_state, ghc_version, flags_hash, output_dir).is_empty()
}

fn collect_dependents(graph: &ModuleGraph, module: &str, dependents: &mut HashSet<String>) {
    for (name, deps) in &graph.dependencies {
        if deps.contains(&module.to_string()) && !dependents.contains(name) {
            dependents.insert(name.clone());
            // Recursively collect dependents of this dependent
            collect_dependents(graph, name, dependents);
        }
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", duration.as_millis())
    } else if secs < 60.0 {
        format!("{:.1}s", secs)
    } else {
        format!("{:.1}m", secs / 60.0)
    }
}

// ============================================================================
// C Compiler Support
// ============================================================================

/// C compiler configuration.
#[derive(Debug, Clone)]
pub struct CCompilerConfig {
    /// Path to C compiler executable
    pub cc_path: PathBuf,
    /// Compiler type (gcc, clang, cc)
    pub compiler_type: CCompilerType,
}

/// Type of C compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CCompilerType {
    Gcc,
    Clang,
    Generic, // Unknown cc
}

impl Default for CCompilerConfig {
    fn default() -> Self {
        Self {
            cc_path: PathBuf::from("cc"),
            compiler_type: CCompilerType::Generic,
        }
    }
}

impl CCompilerConfig {
    /// Detect the C compiler from the environment.
    pub async fn detect() -> Result<Self> {
        let runner = CommandRunner::new();

        // Try clang first (often better diagnostics)
        if let Ok(output) = runner.run("clang", ["--version"]).await
            && output.success()
        {
            return Ok(Self {
                cc_path: PathBuf::from("clang"),
                compiler_type: CCompilerType::Clang,
            });
        }

        // Try gcc
        if let Ok(output) = runner.run("gcc", ["--version"]).await
            && output.success()
        {
            return Ok(Self {
                cc_path: PathBuf::from("gcc"),
                compiler_type: CCompilerType::Gcc,
            });
        }

        // Try generic cc
        if let Ok(output) = runner.run("cc", ["--version"]).await
            && output.success()
        {
            return Ok(Self {
                cc_path: PathBuf::from("cc"),
                compiler_type: CCompilerType::Generic,
            });
        }

        Err(Error::ToolchainMissing {
            tool: "cc".to_string(),
            source: None,
            fixes: vec![hx_core::Fix::new(
                "Install a C compiler (gcc, clang, or cc)",
            )],
        })
    }
}

/// Result of compiling C source files.
#[derive(Debug, Clone)]
pub struct CCompileResult {
    /// Whether all compilations succeeded
    pub success: bool,
    /// Paths to compiled object files
    pub object_files: Vec<PathBuf>,
    /// Number of files compiled
    pub files_compiled: usize,
    /// Compilation duration
    pub duration: Duration,
    /// Warnings from compilation
    pub warnings: Vec<String>,
    /// Errors from compilation
    pub errors: Vec<String>,
}

/// Options for C source compilation.
#[derive(Debug, Clone)]
pub struct CCompileOptions {
    /// Include directories (-I flags)
    pub include_dirs: Vec<PathBuf>,
    /// Preprocessor defines (-D flags)
    pub defines: Vec<String>,
    /// Optimization level (0, 1, 2, 3, or s)
    pub optimization: String,
    /// Additional compiler flags
    pub extra_flags: Vec<String>,
    /// Enable position-independent code (-fPIC)
    pub pic: bool,
    /// Output directory for object files
    pub output_dir: PathBuf,
    /// Enable verbose output
    pub verbose: bool,
}

impl Default for CCompileOptions {
    fn default() -> Self {
        Self {
            include_dirs: Vec::new(),
            defines: Vec::new(),
            optimization: "2".to_string(),
            extra_flags: Vec::new(),
            pic: true, // Usually needed for shared libraries
            output_dir: PathBuf::from("."),
            verbose: false,
        }
    }
}

/// Compile C source files.
///
/// Takes a list of C source files and compiles them to object files.
pub async fn compile_c_sources(
    cc: &CCompilerConfig,
    sources: &[PathBuf],
    options: &CCompileOptions,
    working_dir: &Path,
) -> Result<CCompileResult> {
    let start = Instant::now();

    if sources.is_empty() {
        return Ok(CCompileResult {
            success: true,
            object_files: Vec::new(),
            files_compiled: 0,
            duration: start.elapsed(),
            warnings: Vec::new(),
            errors: Vec::new(),
        });
    }

    // Ensure output directory exists
    let output_dir = working_dir.join(&options.output_dir);
    std::fs::create_dir_all(&output_dir).map_err(|e| Error::Io {
        message: "Failed to create C output directory".to_string(),
        path: Some(output_dir.clone()),
        source: e,
    })?;

    let mut object_files = Vec::new();
    let mut all_warnings = Vec::new();
    let mut all_errors = Vec::new();
    let mut success = true;

    let runner = CommandRunner::new().with_working_dir(working_dir);
    let cc_path = cc.cc_path.display().to_string();

    for source in sources {
        // Compute output object file path
        let source_stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "out".to_string());

        // Preserve directory structure in output
        let relative_path = if source.is_relative() {
            source.clone()
        } else if let Ok(rel) = source.strip_prefix(working_dir) {
            rel.to_path_buf()
        } else {
            PathBuf::from(&source_stem)
        };

        let obj_subdir = relative_path.parent().unwrap_or(Path::new(""));
        let obj_dir = output_dir.join(obj_subdir);
        std::fs::create_dir_all(&obj_dir).ok();

        let obj_file = obj_dir.join(format!("{}.o", source_stem));

        // Build compiler args
        let mut args: Vec<String> = vec![
            "-c".to_string(), // Compile only
            "-o".to_string(),
            obj_file.display().to_string(),
        ];

        // Add optimization
        if !options.optimization.is_empty() {
            args.push(format!("-O{}", options.optimization));
        }

        // Add PIC flag
        if options.pic {
            args.push("-fPIC".to_string());
        }

        // Add include directories
        for inc in &options.include_dirs {
            let full_path = if inc.is_absolute() {
                inc.clone()
            } else {
                working_dir.join(inc)
            };
            args.push(format!("-I{}", full_path.display()));
        }

        // Add defines
        for def in &options.defines {
            args.push(format!("-D{}", def));
        }

        // Add extra flags
        args.extend(options.extra_flags.clone());

        // Add source file
        let source_path = if source.is_absolute() {
            source.clone()
        } else {
            working_dir.join(source)
        };
        args.push(source_path.display().to_string());

        if options.verbose {
            info!("C compile: {} {}", cc_path, args.join(" "));
        }

        // Run the compiler
        let output = match runner.run(&cc_path, &args).await {
            Ok(o) => o,
            Err(e) => {
                all_errors.push(format!("Failed to run C compiler: {}", e));
                success = false;
                continue;
            }
        };

        if output.success() {
            object_files.push(obj_file);

            // Collect any warnings from stderr
            if !output.stderr.is_empty() {
                for line in output.stderr.lines() {
                    if line.contains("warning:") {
                        all_warnings.push(line.to_string());
                    }
                }
            }
        } else {
            success = false;
            all_errors.push(format!(
                "C compilation failed for {}: {}",
                source.display(),
                output.stderr
            ));
        }
    }

    Ok(CCompileResult {
        success,
        object_files,
        files_compiled: sources.len(),
        duration: start.elapsed(),
        warnings: all_warnings,
        errors: all_errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ghc_config_default() {
        let config = GhcConfig::default();
        assert_eq!(config.ghc_path, PathBuf::from("ghc"));
        assert!(config.packages.is_empty());
    }

    #[test]
    fn test_native_build_options_default() {
        let options = NativeBuildOptions::default();
        assert_eq!(options.optimization, 1);
        assert!(options.warnings);
        assert!(!options.werror);
    }

    #[test]
    fn test_native_build_up_to_date_fast_path() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let src_dir = root.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let main_path = src_dir.join("Main.hs");
        fs::write(
            &main_path,
            "module Main where\nmain :: IO ()\nmain = pure ()\n",
        )
        .unwrap();

        // Pretend Main was already compiled: its output files exist...
        let output_dir = root.join(".hx/native-build");
        fs::create_dir_all(output_dir.join("o")).unwrap();
        fs::create_dir_all(output_dir.join("hi")).unwrap();
        fs::write(output_dir.join("o").join("Main.o"), b"").unwrap();
        fs::write(output_dir.join("hi").join("Main.hi"), b"").unwrap();

        // ...and its state is recorded (hash + mtime + size, via mark_compiled).
        let mut state = BuildState {
            ghc_version: "9.8.2".to_string(),
            flags_hash: "flags".to_string(),
            modules: HashMap::new(),
        };
        state.mark_compiled("Main", &main_path, &[]);
        state.save(root).unwrap();

        // Nothing changed -> up to date (taken via the mtime/size fast path).
        assert!(native_build_up_to_date(
            root,
            &[src_dir.clone()],
            &output_dir,
            "9.8.2",
            "flags"
        ));

        // Source changed -> mtime/size differ -> content hash differs -> rebuild.
        fs::write(
            &main_path,
            "module Main where\nmain :: IO ()\nmain = print 42\n",
        )
        .unwrap();
        assert!(!native_build_up_to_date(
            root,
            &[src_dir.clone()],
            &output_dir,
            "9.8.2",
            "flags"
        ));

        // A different GHC version invalidates even if content matches.
        fs::write(
            &main_path,
            "module Main where\nmain :: IO ()\nmain = pure ()\n",
        )
        .unwrap();
        assert!(!native_build_up_to_date(
            root,
            &[src_dir],
            &output_dir,
            "9.10.1",
            "flags"
        ));
    }

    #[test]
    fn test_parse_ghc_output_error() {
        let output = CommandOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "src/Main.hs:10:5: error:\n    Variable not in scope: foo".to_string(),
            duration: Duration::from_secs(1),
        };
        let (warnings, errors) = parse_ghc_output(&output);
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("error:"));
    }

    #[test]
    fn test_parse_ghc_output_warning() {
        let output = CommandOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: "src/Lib.hs:5:1: warning: [-Wunused-imports]\n    The import of 'Data.List' is redundant".to_string(),
            duration: Duration::from_secs(1),
        };
        let (warnings, errors) = parse_ghc_output(&output);
        assert_eq!(warnings.len(), 1);
        assert!(errors.is_empty());
        assert!(warnings[0].contains("warning:"));
    }

    #[test]
    fn test_c_compiler_config_default() {
        let config = CCompilerConfig::default();
        assert_eq!(config.cc_path, PathBuf::from("cc"));
        assert_eq!(config.compiler_type, CCompilerType::Generic);
    }

    #[test]
    fn test_c_compile_options_default() {
        let options = CCompileOptions::default();
        assert!(options.include_dirs.is_empty());
        assert!(options.defines.is_empty());
        assert_eq!(options.optimization, "2");
        assert!(options.pic);
        assert!(!options.verbose);
    }

    #[tokio::test]
    async fn test_compile_c_sources_empty() {
        let cc = CCompilerConfig::default();
        let options = CCompileOptions::default();
        let working_dir = PathBuf::from(".");

        let result = compile_c_sources(&cc, &[], &options, &working_dir).await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.success);
        assert_eq!(result.files_compiled, 0);
        assert!(result.object_files.is_empty());
    }

    #[tokio::test]
    async fn test_c_compiler_detect() {
        // This test may fail on systems without a C compiler
        if let Ok(cc) = CCompilerConfig::detect().await {
            // Should detect one of the known compiler types
            assert!(
                cc.compiler_type == CCompilerType::Clang
                    || cc.compiler_type == CCompilerType::Gcc
                    || cc.compiler_type == CCompilerType::Generic
            );
        }
    }
}
