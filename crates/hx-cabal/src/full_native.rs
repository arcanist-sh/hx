//! Full native build orchestration.
//!
//! This module provides complete native builds from source, compiling all
//! dependencies using direct GHC invocation instead of relying on cabal.

use crate::native::{GhcConfig, NativeBuildOptions, NativeBuildResult, NativeBuilder};
use crate::package_build::{PackageBuildConfig, build_package};
use crate::package_db::PackageDb;
use hx_core::{Error, Fix, Result};
use hx_solver::{BuildPlan, BuildStyle, BuildUnit, FetchResult, extract_package};
use hx_ui::Output;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Options for full native builds.
#[derive(Debug, Clone)]
pub struct FullNativeBuildOptions {
    /// Number of parallel compilation jobs
    pub jobs: usize,
    /// Optimization level (0, 1, or 2)
    pub optimization: u8,
    /// Enable profiling
    pub profiling: bool,
    /// Verbose output
    pub verbose: bool,
    /// Force rebuild even if cached
    pub force: bool,
    /// Skip packages that can't be built natively
    pub skip_unsupported: bool,
}

impl Default for FullNativeBuildOptions {
    fn default() -> Self {
        Self {
            jobs: num_cpus::get(),
            optimization: 1,
            profiling: false,
            verbose: false,
            force: false,
            skip_unsupported: true,
        }
    }
}

/// Result of a full native build.
#[derive(Debug)]
pub struct FullNativeBuildResult {
    /// Whether the build succeeded
    pub success: bool,
    /// Total build duration
    pub duration: Duration,
    /// Number of packages built from source
    pub packages_built: usize,
    /// Number of packages skipped (pre-installed or cached)
    pub packages_skipped: usize,
    /// Number of packages that failed
    pub packages_failed: usize,
    /// Package IDs that were registered
    pub registered_packages: Vec<String>,
    /// Packages that were skipped due to build type
    pub skipped_packages: Vec<(String, String)>,
    /// The local project build result
    pub project_result: Option<NativeBuildResult>,
    /// Errors encountered
    pub errors: Vec<String>,
    /// Warnings encountered
    pub warnings: Vec<String>,
}

/// Full native builder that handles complete builds from source.
pub struct FullNativeBuilder {
    ghc: GhcConfig,
    package_db: PackageDb,
    cache_dir: PathBuf,
    /// Map of package name -> registered package ID
    built_packages: HashMap<String, String>,
    /// Names of pre-installed boot packages in the plan, exposed by name.
    preinstalled: HashSet<String>,
    /// Name → version of every package available to the build (plan packages
    /// plus GHC boot packages), used to generate `cabal_macros.h`.
    package_versions: Vec<(String, String)>,
}

impl FullNativeBuilder {
    /// Create a new full native builder.
    pub async fn new(ghc: GhcConfig, cache_dir: PathBuf) -> Result<Self> {
        let package_db = PackageDb::open(&ghc.version, &cache_dir).await?;

        Ok(Self {
            ghc,
            package_db,
            cache_dir,
            built_packages: HashMap::new(),
            preinstalled: HashSet::new(),
            package_versions: Vec::new(),
        })
    }

    /// Build a project with all its dependencies from source.
    pub async fn build_project(
        &mut self,
        project_root: &Path,
        build_plan: &BuildPlan,
        fetched_packages: &[FetchResult],
        options: &FullNativeBuildOptions,
        local_options: &NativeBuildOptions,
        output: &Output,
    ) -> Result<FullNativeBuildResult> {
        let start = Instant::now();
        let mut result = FullNativeBuildResult {
            success: true,
            duration: Duration::ZERO,
            packages_built: 0,
            packages_skipped: 0,
            packages_failed: 0,
            registered_packages: Vec::new(),
            skipped_packages: Vec::new(),
            project_result: None,
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        // Assemble the full name → version map (every plan package plus GHC's
        // boot packages) once, so each dependency build can generate a complete
        // cabal_macros.h. Boot packages come first; plan packages override them
        // if a plan pins a different version.
        self.package_versions = self.ghc.boot_packages.clone();
        for unit in &build_plan.packages {
            self.package_versions
                .push((unit.name.clone(), unit.version.to_string()));
        }

        output.status("Building", "dependencies from source");

        // Create fetch result lookup
        let fetch_map: HashMap<String, &FetchResult> = fetched_packages
            .iter()
            .map(|f| (format!("{}-{}", f.name, f.version), f))
            .collect();

        // Build dependencies in topological order
        for unit in &build_plan.packages {
            match unit.style {
                BuildStyle::PreInstalled => {
                    debug!("Skipping pre-installed package: {}", unit.name);
                    result.packages_skipped += 1;
                    // Pre-installed packages live in GHC's global package db and
                    // are referenced by name (`-package <name>`), NOT by a
                    // fabricated `name-version` id — GHC's real ids carry an ABI
                    // hash, so passing `-package-id base-4.19.1.0` fails with
                    // `unknown package`. We therefore do NOT add them to
                    // `built_packages` (which feeds `-package-id`); GHC auto-
                    // exposes base and friends. (Non-auto-exposed boot packages
                    // such as `containers`/`text` are exposed by name below.
                    self.preinstalled.insert(unit.name.clone());
                }
                BuildStyle::Cached => {
                    // Check if we have a cached version
                    let pkg_key = format!("{}-{}", unit.name, unit.version);
                    if self
                        .package_db
                        .has_package(&unit.name, &unit.version.to_string())
                    {
                        debug!("Skipping cached package: {}", pkg_key);
                        result.packages_skipped += 1;
                        self.built_packages.insert(unit.name.clone(), pkg_key);
                    } else {
                        // Need to rebuild
                        if let Err(e) = self
                            .build_dependency(unit, &fetch_map, options, output, &mut result)
                            .await
                        {
                            result.errors.push(format!("{}: {}", unit.name, e));
                            result.packages_failed += 1;
                            if !options.skip_unsupported {
                                result.success = false;
                                break;
                            }
                        }
                    }
                }
                BuildStyle::Source => {
                    if let Err(e) = self
                        .build_dependency(unit, &fetch_map, options, output, &mut result)
                        .await
                    {
                        result.errors.push(format!("{}: {}", unit.name, e));
                        result.packages_failed += 1;
                        if !options.skip_unsupported {
                            result.success = false;
                            break;
                        }
                    }
                }
            }
        }

        // Build the local project if dependencies succeeded
        if result.success {
            output.status("Building", "local project");

            // Build extra flags for package database and dependencies
            let mut extra_flags = vec![
                "-package-db".to_string(),
                self.package_db.db_path().to_string_lossy().to_string(),
            ];

            // Add package-id flags for all built dependencies.
            for pkg_id in self.built_packages.values() {
                extra_flags.push("-package-id".to_string());
                extra_flags.push(pkg_id.clone());
            }
            // Expose pre-installed boot packages (containers, text, …) by name.
            for pkg in &self.preinstalled {
                extra_flags.push("-package".to_string());
                extra_flags.push(pkg.clone());
            }

            // Start from the caller's options (which carry the project's source
            // dirs, main module, and output paths) and add the dependency flags.
            let mut native_options = local_options.clone();
            native_options.jobs = options.jobs;
            native_options.optimization = options.optimization;
            native_options.verbose = options.verbose;
            native_options.extra_flags.extend(extra_flags);

            let builder = NativeBuilder::new(self.ghc.clone());

            match builder.build(project_root, &native_options, output).await {
                Ok(project_result) => {
                    result.project_result = Some(project_result);
                }
                Err(e) => {
                    result.success = false;
                    result.errors.push(format!("Project build failed: {}", e));
                }
            }
        }

        result.duration = start.elapsed();
        Ok(result)
    }

    /// Build a single dependency package.
    async fn build_dependency(
        &mut self,
        unit: &BuildUnit,
        fetch_map: &HashMap<String, &FetchResult>,
        options: &FullNativeBuildOptions,
        output: &Output,
        result: &mut FullNativeBuildResult,
    ) -> Result<()> {
        let pkg_key = format!("{}-{}", unit.name, unit.version);

        // Fast path: if this exact package is already registered in the native
        // store and its compiled archive is on disk, reuse it instead of
        // recompiling from source on every build. We recover the full unit-id
        // (name-version-<abihash>) from the install directory, which is what the
        // local project needs for `-package-id` at link time.
        if self
            .package_db
            .has_package(&unit.name, &unit.version.to_string())
            && let Some(unit_id) =
                self.find_installed_unit_id(&unit.name, &unit.version.to_string())
        {
            debug!("Reusing already-built dependency: {}", unit_id);
            result.packages_skipped += 1;
            self.built_packages.insert(unit.name.clone(), unit_id);
            return Ok(());
        }

        output.status("Building", &format!("{} {}", unit.name, unit.version));

        // Get the tarball path
        let fetch_result = fetch_map.get(&pkg_key).ok_or_else(|| Error::Config {
            message: format!("package not fetched: {}", pkg_key),
            path: None,
            source: None,
            fixes: vec![Fix::with_command(
                "Fetch packages first",
                "hx lock && hx fetch",
            )],
        })?;

        // Extract the package
        let extracted = extract_package(&fetch_result.path, &self.cache_dir, options.force)
            .map_err(|e| Error::Config {
                message: format!("failed to extract {}: {}", pkg_key, e),
                path: Some(fetch_result.path.clone()),
                source: None,
                fixes: vec![Fix::with_command("Try re-fetching", "hx fetch --force")],
            })?;

        // Check if this package can be built natively
        // Custom build types are now supported via the setup module
        let build_type = &extracted.build_info.build_type;
        let is_custom = matches!(build_type, hx_solver::BuildType::Custom);

        if !extracted.can_build_native() && !is_custom {
            let reason = extracted
                .skip_reason()
                .unwrap_or_else(|| "unknown".to_string());
            warn!("Skipping {} {}: {}", unit.name, unit.version, reason);
            result
                .skipped_packages
                .push((pkg_key.clone(), reason.clone()));
            result.warnings.push(format!(
                "Skipped {} {}: {}",
                unit.name, unit.version, reason
            ));
            result.packages_skipped += 1;

            if !options.skip_unsupported {
                return Err(Error::BuildFailed {
                    errors: vec![format!(
                        "Package {} cannot be built natively: {}",
                        pkg_key, reason
                    )],
                    fixes: vec![Fix::new("Use cabal to build this package instead")],
                });
            }

            return Ok(());
        }

        // Handle Custom build type packages
        if is_custom {
            return self
                .build_custom_package(&extracted, unit, options, output, result)
                .await;
        }

        // Built-from-source dependencies are exposed by id; pre-installed boot
        // packages this unit depends on are exposed by name.
        let dep_ids: Vec<String> = unit
            .depends
            .iter()
            .filter_map(|dep| self.built_packages.get(dep).cloned())
            .collect();
        let expose_packages: Vec<String> = unit
            .depends
            .iter()
            .filter(|dep| self.preinstalled.contains(*dep))
            .cloned()
            .collect();

        // The compiler needs the native store's package db on its search path so
        // that `-package-id` flags for previously source-built dependencies (e.g.
        // vector depending on primitive) resolve. Without it GHC reports "cannot
        // satisfy -package-id". The global db alone only covers boot packages.
        let mut dep_ghc = self.ghc.clone();
        let store_db = self.package_db.db_path().to_path_buf();
        if !dep_ghc.package_dbs.contains(&store_db) {
            dep_ghc.package_dbs.push(store_db);
        }

        // Configure the build
        let build_config = PackageBuildConfig {
            ghc: dep_ghc,
            cc: None, // Auto-detect C compiler when needed
            build_dir: self.cache_dir.join("builds").join(&pkg_key),
            install_dir: self
                .cache_dir
                .join(format!("ghc-{}", self.ghc.version))
                .join("lib"),
            dependency_ids: dep_ids,
            expose_packages,
            jobs: options.jobs,
            optimization: options.optimization,
            verbose: options.verbose,
            package_versions: self.package_versions.clone(),
        };

        // Build the package
        let build_result = build_package(&extracted, &build_config, output).await?;

        if !build_result.success {
            return Err(Error::BuildFailed {
                errors: build_result.errors.clone(),
                fixes: vec![Fix::with_command("See full output", "hx build --verbose")],
            });
        }

        // Register the package
        let package_id = self
            .package_db
            .register(&build_result.registration_file)
            .await?;

        result.registered_packages.push(package_id.clone());
        result.packages_built += 1;
        self.built_packages.insert(unit.name.clone(), package_id);

        // Collect warnings
        result.warnings.extend(build_result.warnings.clone());

        info!(
            "Built {} {} ({} modules in {:?})",
            unit.name, unit.version, build_result.modules_compiled, build_result.duration
        );

        Ok(())
    }

    /// Build a package with Custom build type using Setup.hs.
    async fn build_custom_package(
        &mut self,
        extracted: &hx_solver::ExtractedPackage,
        unit: &BuildUnit,
        options: &FullNativeBuildOptions,
        _output: &Output,
        result: &mut FullNativeBuildResult,
    ) -> Result<()> {
        use crate::setup::{
            BuildFlags, ConfigureFlags, CopyFlags, CustomSetupOptions, build_with_setup,
        };

        let pkg_key = format!("{}-{}", unit.name, unit.version);
        info!("Building {} with custom Setup.hs", pkg_key);

        // Prepare setup-depends packages
        let setup_depends: Vec<String> = extracted
            .build_info
            .custom_setup
            .as_ref()
            .map(|cs| cs.setup_depends.iter().map(|d| d.name.clone()).collect())
            .unwrap_or_default();

        // Configure custom setup options
        let setup_options = CustomSetupOptions {
            ghc_path: self.ghc.ghc_path.clone(),
            package_dbs: self.ghc.package_dbs.clone(),
            setup_depends,
            build_dir: self.cache_dir.join("builds").join(&pkg_key).join("setup"),
            verbose: options.verbose,
        };

        // Install directory
        let install_dir = self
            .cache_dir
            .join(format!("ghc-{}", self.ghc.version))
            .join("lib")
            .join(&pkg_key);

        std::fs::create_dir_all(&install_dir).map_err(|e| Error::Io {
            message: "failed to create install directory".to_string(),
            path: Some(install_dir.clone()),
            source: e,
        })?;

        // Configure flags
        let configure_flags = ConfigureFlags {
            prefix: Some(install_dir.clone()),
            package_db: Some(self.package_db.db_path().to_path_buf()),
            with_ghc: Some(self.ghc.ghc_path.clone()),
            ..Default::default()
        };

        // Build flags
        let build_flags = BuildFlags {
            jobs: Some(options.jobs),
            ..Default::default()
        };

        // Copy flags
        let copy_flags = CopyFlags {
            destdir: None, // Install directly to prefix
            ..Default::default()
        };

        // Run the custom build
        let build_result = build_with_setup(
            &extracted.source_dir,
            &setup_options,
            &configure_flags,
            &build_flags,
            Some(&copy_flags),
            Some(self.package_db.db_path()),
        )
        .await?;

        if !build_result.success {
            return Err(Error::BuildFailed {
                errors: build_result.errors,
                fixes: vec![Fix::with_command("See full output", "hx build --verbose")],
            });
        }

        // For Custom builds, we assume the package registers itself
        // via Setup register. Mark it as built.
        let package_id = format!("{}-{}", unit.name, unit.version);
        result.registered_packages.push(package_id.clone());
        result.packages_built += 1;
        self.built_packages.insert(unit.name.clone(), package_id);

        // Collect warnings
        result.warnings.extend(build_result.warnings);

        info!(
            "Built {} {} via custom Setup.hs in {:?}",
            unit.name, unit.version, build_result.duration
        );

        Ok(())
    }

    /// Get the package database.
    pub fn package_db(&self) -> &PackageDb {
        &self.package_db
    }

    /// Get mutable access to the package database.
    pub fn package_db_mut(&mut self) -> &mut PackageDb {
        &mut self.package_db
    }

    /// Get the GHC configuration.
    pub fn ghc_config(&self) -> &GhcConfig {
        &self.ghc
    }

    /// Get the cache directory.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Check if a package is already built.
    pub fn is_built(&self, name: &str) -> bool {
        self.built_packages.contains_key(name)
    }

    /// Find the full unit-id (`name-version-<abihash>`) of an already-installed
    /// dependency by locating its directory in the native store's `lib` and
    /// confirming the compiled archive exists. Returns `None` if not present, so
    /// callers fall back to rebuilding from source.
    fn find_installed_unit_id(&self, name: &str, version: &str) -> Option<String> {
        let lib_dir = self
            .cache_dir
            .join(format!("ghc-{}", self.ghc.version))
            .join("lib");
        let prefix = format!("{}-{}-", name, version);
        let entries = std::fs::read_dir(&lib_dir).ok()?;
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let dir_name = file_name.to_string_lossy();
            if dir_name.starts_with(&prefix) {
                let archive = entry
                    .path()
                    .join("lib")
                    .join(format!("libHS{}-{}.a", name, version));
                if archive.exists() {
                    return Some(dir_name.into_owned());
                }
            }
        }
        None
    }

    /// Get the package ID for a built package.
    pub fn package_id(&self, name: &str) -> Option<&str> {
        self.built_packages.get(name).map(|s| s.as_str())
    }

    /// Clear all built package state.
    pub async fn clear(&mut self) -> Result<()> {
        self.built_packages.clear();

        // Clear the builds directory
        let builds_dir = self.cache_dir.join("builds");
        if builds_dir.exists() {
            std::fs::remove_dir_all(&builds_dir).map_err(|e| Error::Io {
                message: "failed to clear builds directory".to_string(),
                path: Some(builds_dir),
                source: e,
            })?;
        }

        Ok(())
    }
}

/// Pre-installed packages that come with GHC and don't need to be built.
pub fn pre_installed_packages() -> HashSet<&'static str> {
    [
        "base",
        "ghc-prim",
        "ghc-bignum",
        "integer-gmp",
        "integer-simple",
        "template-haskell",
        "ghc",
        "array",
        "binary",
        "bytestring",
        "containers",
        "deepseq",
        "directory",
        "exceptions",
        "filepath",
        "mtl",
        "parsec",
        "pretty",
        "process",
        "stm",
        "text",
        "time",
        "transformers",
        "unix",
        "Win32",
        "xhtml",
        "Cabal",
        "Cabal-syntax",
        "ghc-boot",
        "ghc-boot-th",
        "ghc-compact",
        "ghc-heap",
        "haskeline",
        "hpc",
        "terminfo",
        "rts",
    ]
    .into_iter()
    .collect()
}

/// Check if a package is pre-installed with GHC.
pub fn is_pre_installed(name: &str) -> bool {
    pre_installed_packages().contains(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pre_installed_packages() {
        assert!(is_pre_installed("base"));
        assert!(is_pre_installed("ghc-prim"));
        assert!(is_pre_installed("bytestring"));
        assert!(!is_pre_installed("aeson"));
        assert!(!is_pre_installed("lens"));
    }

    #[test]
    fn test_full_native_build_options_default() {
        let opts = FullNativeBuildOptions::default();
        assert!(opts.jobs > 0);
        assert_eq!(opts.optimization, 1);
        assert!(!opts.profiling);
        assert!(!opts.verbose);
        assert!(!opts.force);
        assert!(opts.skip_unsupported);
    }
}
