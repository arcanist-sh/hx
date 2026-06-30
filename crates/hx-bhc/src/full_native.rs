//! Full BHC native build orchestrator.
//!
//! Builds all dependencies from source, then the local project. This module
//! ties together the package build pipeline and the local project builder,
//! processing packages in topological order from a resolved build plan.

use crate::native::{BhcCompilerConfig, BhcNativeBuildOptions, BhcNativeError};
use crate::native_builder::{BhcNativeBuildResult, BhcNativeBuilder};
use crate::package_build::{BhcPackageBuildConfig, build_package};
use crate::package_db::BhcPackageDb;
use hx_cache::artifacts::{retrieve_artifacts, store_artifacts};
use hx_solver::plan::BuildPlan;
use hx_solver::{BuildStyle, BuildUnit, FetchResult, extract_package};
use hx_ui::Output;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Result of a full native build including all dependencies.
#[derive(Debug)]
pub struct FullNativeBuildResult {
    /// Whether the overall build succeeded.
    pub success: bool,
    /// Total wall-clock duration.
    pub duration: Duration,
    /// Number of packages built from source.
    pub packages_built: usize,
    /// Number of packages skipped (pre-installed or cached).
    pub packages_skipped: usize,
    /// Number of packages that failed to build.
    pub packages_failed: usize,
    /// Package IDs that were registered in the package database.
    pub registered_packages: Vec<String>,
    /// The local project build result.
    pub project_result: Option<BhcNativeBuildResult>,
    /// Errors encountered during the build.
    pub errors: Vec<String>,
    /// Warnings encountered during the build.
    pub warnings: Vec<String>,
}

/// Full BHC native build orchestrator.
///
/// Handles the complete build pipeline: resolve dependencies from build plan,
/// build each dependency in topological order, register in package database,
/// then build the local project.
pub struct BhcFullNativeBuilder {
    /// BHC compiler configuration.
    bhc: BhcCompilerConfig,
    /// Package database for registering built packages.
    package_db: BhcPackageDb,
    /// Root directory for cached build artifacts.
    cache_dir: PathBuf,
    /// Map of package name to computed package ID for already-built packages.
    built_packages: HashMap<String, String>,
}

impl BhcFullNativeBuilder {
    /// Create a new full native builder.
    ///
    /// The `cache_dir` is used to store built package artifacts for reuse
    /// across builds.
    pub fn new(bhc: BhcCompilerConfig, package_db: BhcPackageDb, cache_dir: PathBuf) -> Self {
        Self {
            bhc,
            package_db,
            cache_dir,
            built_packages: HashMap::new(),
        }
    }

    /// Build the full project with all dependencies.
    ///
    /// Processes the build plan in topological order:
    /// 1. Pre-installed packages are skipped.
    /// 2. Cached packages are checked in the package DB; if missing, rebuilt.
    /// 3. Source packages are extracted, compiled, and registered.
    /// 4. The local project is built using `BhcNativeBuilder` with all
    ///    dependency package IDs.
    pub async fn build_project(
        &mut self,
        project_root: &Path,
        plan: &BuildPlan,
        fetched_packages: &[FetchResult],
        options: &BhcNativeBuildOptions,
        output: &Output,
    ) -> Result<FullNativeBuildResult, BhcNativeError> {
        let start = Instant::now();
        info!("Building project with BHC native pipeline");
        info!("Build plan has {} packages", plan.packages.len());

        let mut result = FullNativeBuildResult {
            success: true,
            duration: Duration::ZERO,
            packages_built: 0,
            packages_skipped: 0,
            packages_failed: 0,
            registered_packages: Vec::new(),
            project_result: None,
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        output.status("Building", "dependencies from source");

        // Create fetch result lookup
        let fetch_map: HashMap<String, &FetchResult> = fetched_packages
            .iter()
            .map(|f| (format!("{}-{}", f.name, f.version), f))
            .collect();

        // Build dependencies in topological order
        for unit in &plan.packages {
            match unit.style {
                BuildStyle::PreInstalled => {
                    debug!("Skipping pre-installed package: {}", unit.name);
                    result.packages_skipped += 1;
                    self.built_packages
                        .insert(unit.name.clone(), format!("{}-{}", unit.name, unit.version));
                }
                BuildStyle::Cached => {
                    let pkg_key = format!("{}-{}", unit.name, unit.version);
                    if self
                        .package_db
                        .has_package(&unit.name, &unit.version.to_string())
                    {
                        debug!("Skipping cached package: {}", pkg_key);
                        result.packages_skipped += 1;
                        self.built_packages.insert(unit.name.clone(), pkg_key);
                    } else {
                        // Cache entry missing, need to rebuild
                        if let Err(e) = self
                            .build_dependency(unit, &fetch_map, options, output, &mut result)
                            .await
                        {
                            result.errors.push(format!("{}: {}", unit.name, e));
                            result.packages_failed += 1;
                            result.success = false;
                            break;
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
                        result.success = false;
                        break;
                    }
                }
            }
        }

        // Build the local project if dependencies succeeded
        if result.success {
            output.status("Building", "local project");

            // Build extra flags for package database and dependencies
            let mut extra_flags = vec![
                "--package-db".to_string(),
                self.package_db.db_path().to_string_lossy().to_string(),
            ];

            for pkg_id in self.built_packages.values() {
                extra_flags.push("--package-id".to_string());
                extra_flags.push(pkg_id.clone());
            }

            let local_options = BhcNativeBuildOptions {
                extra_flags,
                ..options.clone()
            };

            let builder = BhcNativeBuilder::new(BhcCompilerConfig {
                bhc_path: self.bhc.bhc_path.clone(),
                version: self.bhc.version.clone(),
                profile: self.bhc.profile,
                package_dbs: self.bhc.package_dbs.clone(),
                packages: self.bhc.packages.clone(),
                tensor_fusion: self.bhc.tensor_fusion,
                emit_kernel_report: self.bhc.emit_kernel_report,
            });

            match builder.build(project_root, &local_options, output).await {
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
        options: &BhcNativeBuildOptions,
        output: &Output,
        result: &mut FullNativeBuildResult,
    ) -> Result<(), BhcNativeError> {
        let pkg_key = format!("{}-{}", unit.name, unit.version);

        // Check if this package is a BHC builtin (provided by the compiler's stdlib)
        if let Some(builtin_id) = crate::builtin_packages::builtin_package_id(&unit.name) {
            debug!(
                "Skipping BHC builtin package: {} (mapped to {})",
                unit.name, builtin_id
            );
            result.packages_skipped += 1;
            result.registered_packages.push(builtin_id.clone());
            self.built_packages.insert(unit.name.clone(), builtin_id);
            return Ok(());
        }

        output.status("Building", &format!("{} {}", unit.name, unit.version));

        // Look up the fetch result
        let fetch_result = fetch_map.get(&pkg_key).ok_or_else(|| {
            BhcNativeError::CompilationFailed(format!("package not fetched: {}", pkg_key))
        })?;

        // Check artifact cache before building
        let dep_map: HashMap<String, String> = unit
            .depends
            .iter()
            .filter_map(|dep| {
                self.built_packages
                    .get(dep)
                    .map(|id| (dep.clone(), id.clone()))
            })
            .collect();

        let cache_flags = vec![
            format!("-O{}", options.optimization),
            format!("--profile={}", self.bhc.profile.as_str()),
        ];

        let cached = retrieve_artifacts(
            &self.cache_dir,
            &fetch_result.hash,
            &self.bhc.version,
            &cache_flags,
            &dep_map,
            &self.cache_dir.join("artifact-restore"),
        );

        if let Ok(Some(_cached_files)) = cached {
            debug!("Artifact cache hit for {}", pkg_key);
            let package_id = crate::package_build::compute_package_id(
                &unit.name,
                &unit.version.to_string(),
                &self.bhc.version,
                self.bhc.profile.as_str(),
                &unit
                    .depends
                    .iter()
                    .filter_map(|d| self.built_packages.get(d).cloned())
                    .collect::<Vec<_>>(),
            );

            // A cache hit means we skip recompilation, but the package must
            // still be registered in the (possibly fresh) package database so
            // dependents can resolve it. The install tree is persistent at a
            // deterministic path; register its `.conf` from there. If it is
            // missing (install tree pruned), fall through and rebuild rather
            // than registering a package whose interfaces are absent.
            let conf_path = self
                .cache_dir
                .join(format!("bhc-{}", self.bhc.version))
                .join("lib")
                .join(&package_id)
                .join(format!("{}.conf", package_id));

            if conf_path.exists() {
                match self.package_db.register(&conf_path).await {
                    Ok(_) => {
                        result.packages_skipped += 1;
                        result.registered_packages.push(package_id.clone());
                        self.built_packages.insert(unit.name.clone(), package_id);
                        return Ok(());
                    }
                    Err(e) => {
                        warn!(
                            "cache hit for {} but registration failed ({e}); rebuilding",
                            pkg_key
                        );
                    }
                }
            } else {
                debug!(
                    "cache hit for {} but install conf missing at {}; rebuilding",
                    pkg_key,
                    conf_path.display()
                );
            }
        }

        // Extract the package
        let extracted =
            extract_package(&fetch_result.path, &self.cache_dir, false).map_err(|e| {
                BhcNativeError::CompilationFailed(format!("failed to extract {}: {}", pkg_key, e))
            })?;

        // Check if this package can be built natively
        if !extracted.can_build_native() {
            let reason = extracted
                .skip_reason()
                .unwrap_or_else(|| "unknown".to_string());
            warn!("Skipping {} {}: {}", unit.name, unit.version, reason);
            result.warnings.push(format!(
                "Skipped {} {}: {}",
                unit.name, unit.version, reason
            ));
            result.packages_skipped += 1;
            return Ok(());
        }

        // Collect dependency package IDs
        let dep_ids: Vec<String> = unit
            .depends
            .iter()
            .filter_map(|dep| self.built_packages.get(dep).cloned())
            .collect();

        // The in-progress package database must be visible when compiling this
        // package so its already-built dependencies (registered earlier in the
        // topological order) resolve. Thread it in alongside any user-provided
        // package DBs.
        let mut package_dbs = self.bhc.package_dbs.clone();
        let live_db = self.package_db.db_path().to_path_buf();
        if !package_dbs.contains(&live_db) {
            package_dbs.push(live_db);
        }

        // Configure the build
        let build_config = BhcPackageBuildConfig {
            bhc: BhcCompilerConfig {
                bhc_path: self.bhc.bhc_path.clone(),
                version: self.bhc.version.clone(),
                profile: self.bhc.profile,
                package_dbs,
                packages: self.bhc.packages.clone(),
                tensor_fusion: self.bhc.tensor_fusion,
                emit_kernel_report: self.bhc.emit_kernel_report,
            },
            build_dir: self.cache_dir.join("builds").join(&pkg_key),
            install_dir: self
                .cache_dir
                .join(format!("bhc-{}", self.bhc.version))
                .join("lib"),
            dependency_ids: dep_ids,
            jobs: options.jobs,
            optimization: options.optimization,
            verbose: options.verbose,
        };

        // Build the package
        let build_result = build_package(&extracted, &build_config, output).await?;

        if !build_result.success {
            return Err(BhcNativeError::CompilationFailed(format!(
                "build failed for {}: {}",
                pkg_key,
                build_result.errors.join("; ")
            )));
        }

        // Store artifacts in cache
        let artifact_files: Vec<PathBuf> = vec![
            build_result.library_path.clone(),
            build_result.registration_file.clone(),
        ];
        let _ = store_artifacts(
            &self.cache_dir,
            &pkg_key,
            &fetch_result.hash,
            &self.bhc.version,
            &cache_flags,
            &dep_map,
            &artifact_files,
        );

        // Register the package
        let package_id = self
            .package_db
            .register(&build_result.registration_file)
            .await?;

        result.registered_packages.push(package_id.clone());
        result.packages_built += 1;
        result.warnings.extend(build_result.warnings);
        self.built_packages.insert(unit.name.clone(), package_id);

        info!(
            "Built {} {} ({} modules in {:?})",
            unit.name, unit.version, build_result.modules_compiled, build_result.duration
        );

        Ok(())
    }

    /// Get the map of built package name to package ID.
    pub fn built_packages(&self) -> &HashMap<String, String> {
        &self.built_packages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_config::BhcProfile;

    fn test_config() -> BhcCompilerConfig {
        BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        }
    }

    #[test]
    fn test_full_native_builder_new() {
        let config = test_config();
        let package_db = BhcPackageDb {
            path: PathBuf::from("/tmp/test.db"),
            bhc_version: "2026.2.0".to_string(),
            registered: std::collections::HashSet::new(),
        };
        let cache_dir = PathBuf::from("/tmp/cache");

        let builder = BhcFullNativeBuilder::new(config, package_db, cache_dir.clone());

        assert!(builder.built_packages().is_empty());
        assert_eq!(builder.cache_dir, cache_dir);
        assert_eq!(builder.bhc.version, "2026.2.0");
    }

    /// A warm artifact cache must still register the package in a fresh package
    /// database, otherwise the project build cannot resolve it. No BHC binary is
    /// needed: the cache hit skips compilation entirely.
    #[tokio::test]
    async fn test_cache_hit_registers_package() {
        use crate::package_build::compute_package_id;
        use hx_cache::artifacts::store_artifacts;
        use hx_solver::Version;
        use std::collections::HashMap;
        use std::str::FromStr;

        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        let bhc_version = "2026.2.0";
        let profile = BhcProfile::default();

        let package_id = compute_package_id("mypkg", "1.0.0", bhc_version, profile.as_str(), &[]);

        // Persistent install tree holding the package's registration file.
        let install_lib = cache_dir
            .join(format!("bhc-{bhc_version}"))
            .join("lib")
            .join(&package_id);
        std::fs::create_dir_all(&install_lib).unwrap();
        let conf_path = install_lib.join(format!("{package_id}.conf"));
        std::fs::write(
            &conf_path,
            format!(
                "name: mypkg\nversion: 1.0.0\nid: {package_id}\nimport-dirs: {}\nexposed-modules: Data.My\n",
                install_lib.display()
            ),
        )
        .unwrap();

        // Seed the artifact cache so build_dependency takes the cache-hit path.
        // The key inputs must match what build_dependency computes.
        let cache_flags = vec!["-O0".to_string(), format!("--profile={}", profile.as_str())];
        store_artifacts(
            &cache_dir,
            "mypkg-1.0.0",
            "deadbeef",
            bhc_version,
            &cache_flags,
            &HashMap::new(),
            std::slice::from_ref(&conf_path),
        )
        .unwrap();

        let bhc = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc-unused-on-cache-hit"),
            version: bhc_version.to_string(),
            profile,
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        };
        // A FRESH package DB — the bug was that a cache hit left this empty.
        let package_db = BhcPackageDb::open(bhc_version, &cache_dir).await.unwrap();
        let mut builder = BhcFullNativeBuilder::new(bhc, package_db, cache_dir.clone());

        let unit = BuildUnit {
            name: "mypkg".to_string(),
            version: Version::from_str("1.0.0").unwrap(),
            hash: Some("deadbeef".to_string()),
            flags: HashMap::new(),
            pre_installed: false,
            style: BuildStyle::Source,
            depends: Vec::new(),
        };
        let fetch = FetchResult {
            name: "mypkg".to_string(),
            version: Version::from_str("1.0.0").unwrap(),
            path: cache_dir.join("unused.tar.gz"),
            hash: "deadbeef".to_string(),
            cached: true,
        };
        let mut fetch_map: HashMap<String, &FetchResult> = HashMap::new();
        fetch_map.insert("mypkg-1.0.0".to_string(), &fetch);

        let options = BhcNativeBuildOptions {
            src_dirs: Vec::new(),
            output_dir: cache_dir.join("out"),
            optimization: 0,
            warnings: false,
            werror: false,
            extra_flags: Vec::new(),
            jobs: 1,
            verbose: false,
            main_module: None,
            output_exe: None,
            output_lib: None,
            target: None,
            extensions: Vec::new(),
        };
        let mut result = FullNativeBuildResult {
            success: true,
            duration: Duration::ZERO,
            packages_built: 0,
            packages_skipped: 0,
            packages_failed: 0,
            registered_packages: Vec::new(),
            project_result: None,
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        builder
            .build_dependency(&unit, &fetch_map, &options, &Output::new(), &mut result)
            .await
            .expect("cache-hit dependency handling failed");

        assert!(
            builder.package_db.is_registered(&package_id),
            "cache hit must register {package_id} in the package DB"
        );
        assert_eq!(result.packages_skipped, 1);
        assert!(builder.built_packages().contains_key("mypkg"));
    }
}
