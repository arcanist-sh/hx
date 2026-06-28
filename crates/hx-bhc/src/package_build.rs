//! Build a single dependency package from source with BHC.
//!
//! Provides utilities for computing deterministic package identifiers,
//! generating GHC-compatible package registration files, and building
//! complete dependency packages using BHC native compilation.

use crate::compile::compile_module;
use crate::native::{BhcCompilerConfig, BhcNativeBuildOptions, BhcNativeError};
use hx_solver::{ExtractedPackage, build_module_graph};
use hx_ui::Output;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Configuration for building a single package.
pub struct BhcPackageBuildConfig {
    /// BHC compiler configuration.
    pub bhc: BhcCompilerConfig,
    /// Directory for intermediate build artifacts.
    pub build_dir: PathBuf,
    /// Directory where the built package will be installed.
    pub install_dir: PathBuf,
    /// Package IDs of already-built dependencies.
    pub dependency_ids: Vec<String>,
    /// Number of parallel compilation jobs.
    pub jobs: usize,
    /// Optimization level (0-3).
    pub optimization: u8,
    /// Enable verbose output.
    pub verbose: bool,
}

/// Result of building a package.
pub struct PackageBuildResult {
    /// The computed deterministic package ID.
    pub package_id: String,
    /// Path to the generated registration (.conf) file.
    pub registration_file: PathBuf,
    /// Path to the installed library archive.
    pub library_path: PathBuf,
    /// Whether the build succeeded.
    pub success: bool,
    /// Build duration.
    pub duration: Duration,
    /// Number of modules compiled.
    pub modules_compiled: usize,
    /// Warning messages.
    pub warnings: Vec<String>,
    /// Error messages.
    pub errors: Vec<String>,
}

/// Build a single dependency package from source with BHC.
///
/// This function:
/// 1. Discovers source files from the package's library configuration
/// 2. Builds the module dependency graph
/// 3. Compiles modules in parallel groups
/// 4. Creates a static library archive from the compiled `.o` files
/// 5. Generates a registration file for the package database
/// 6. Returns a `PackageBuildResult` with the computed package ID
pub async fn build_package(
    extracted: &ExtractedPackage,
    config: &BhcPackageBuildConfig,
    output: &Output,
) -> Result<PackageBuildResult, BhcNativeError> {
    let start = Instant::now();
    let name = &extracted.name;
    let version = &extracted.version.to_string();

    info!("Building package: {}-{} with BHC", name, version);

    let lib_config = extracted.build_info.library.as_ref().ok_or_else(|| {
        BhcNativeError::CompilationFailed(format!("package {} has no library component", name))
    })?;

    // Compute deterministic package ID
    let package_id = compute_package_id(
        name,
        version,
        &config.bhc.version,
        config.bhc.profile.as_str(),
        &config.dependency_ids,
    );

    // Set up build directories
    let pkg_build_dir = config.build_dir.join(format!("{}-{}", name, version));
    let hi_dir = pkg_build_dir.join("hi");
    let o_dir = pkg_build_dir.join("o");

    for dir in [&pkg_build_dir, &hi_dir, &o_dir] {
        std::fs::create_dir_all(dir).map_err(|e| BhcNativeError::Io {
            message: format!("failed to create build directory {}: {}", dir.display(), e),
            source: e,
        })?;
    }

    // Set up install directories
    let pkg_install_dir = config.install_dir.join(&package_id);
    let lib_install_dir = pkg_install_dir.join("lib");

    std::fs::create_dir_all(&lib_install_dir).map_err(|e| BhcNativeError::Io {
        message: format!(
            "failed to create install directory: {}",
            lib_install_dir.display()
        ),
        source: e,
    })?;

    // Resolve source directories
    let src_dirs: Vec<PathBuf> = if lib_config.hs_source_dirs.is_empty() {
        vec![extracted.source_dir.clone()]
    } else {
        lib_config
            .hs_source_dirs
            .iter()
            .map(|d| extracted.source_dir.join(d))
            .collect()
    };

    // Build module dependency graph
    let graph = build_module_graph(&src_dirs).map_err(|e| {
        BhcNativeError::CompilationFailed(format!(
            "failed to build module graph for {}: {}",
            name, e
        ))
    })?;

    let groups = graph.parallel_groups().map_err(|e| {
        BhcNativeError::CompilationFailed(format!(
            "failed to compute parallel groups for {}: {}",
            name, e
        ))
    })?;

    info!(
        "Package {}: {} modules in {} parallel groups",
        name,
        graph.modules.len(),
        groups.len()
    );

    // Compile modules in parallel groups
    let semaphore = Arc::new(Semaphore::new(config.jobs));
    let mut all_warnings = Vec::new();
    let mut all_errors = Vec::new();
    let mut modules_compiled: usize = 0;
    let mut build_failed = false;

    let build_options = BhcNativeBuildOptions {
        src_dirs: src_dirs.clone(),
        output_dir: pkg_build_dir.clone(),
        optimization: config.optimization,
        warnings: true,
        werror: false,
        extra_flags: Vec::new(),
        jobs: config.jobs,
        verbose: config.verbose,
        main_module: None,
        output_exe: None,
        output_lib: None,
        target: None,
        extensions: lib_config.default_extensions.clone(),
    };

    for (group_idx, group) in groups.iter().enumerate() {
        debug!(
            "Package {}: compiling group {}/{}: {:?}",
            name,
            group_idx + 1,
            groups.len(),
            group
        );

        let mut handles = Vec::new();

        for module_name in group {
            let module_info = match graph.modules.get(module_name) {
                Some(info) => info.clone(),
                None => {
                    warn!(
                        "Module {} not found in graph for {}, skipping",
                        module_name, name
                    );
                    continue;
                }
            };

            let sem = semaphore.clone();
            let bhc = BhcCompilerConfig {
                bhc_path: config.bhc.bhc_path.clone(),
                version: config.bhc.version.clone(),
                profile: config.bhc.profile,
                package_dbs: config.bhc.package_dbs.clone(),
                packages: config.bhc.packages.clone(),
                tensor_fusion: config.bhc.tensor_fusion,
                emit_kernel_report: config.bhc.emit_kernel_report,
            };
            let module_name = module_name.clone();
            let source_file = module_info.path.clone();
            let build_dir = pkg_build_dir.clone();
            let dep_ids = config.dependency_ids.clone();
            let opts = build_options.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed unexpectedly");
                compile_module(
                    &bhc,
                    &module_name,
                    &source_file,
                    &build_dir,
                    &opts,
                    &dep_ids,
                )
                .await
            });

            handles.push(handle);
        }

        for handle in handles {
            match handle.await {
                Ok(Ok(result)) => {
                    if result.success {
                        modules_compiled += 1;
                    } else {
                        build_failed = true;
                    }
                    for w in &result.warnings {
                        all_warnings.push(w.message.clone());
                    }
                    for e in &result.errors {
                        all_errors.push(e.message.clone());
                    }
                }
                Ok(Err(e)) => {
                    build_failed = true;
                    all_errors.push(format!("{}", e));
                }
                Err(e) => {
                    build_failed = true;
                    all_errors.push(format!("task join error: {}", e));
                }
            }
        }

        if build_failed {
            break;
        }
    }

    if build_failed {
        return Ok(PackageBuildResult {
            package_id,
            registration_file: PathBuf::new(),
            library_path: PathBuf::new(),
            success: false,
            duration: start.elapsed(),
            modules_compiled,
            warnings: all_warnings,
            errors: all_errors,
        });
    }

    // Create static library from object files
    let lib_name = format!("libHS{}.a", package_id);
    let lib_path = lib_install_dir.join(&lib_name);

    let object_files = crate::native_builder::collect_object_files(&pkg_build_dir);
    if !object_files.is_empty() {
        create_static_library(&lib_path, &object_files).await?;
    }

    // Install compiled interfaces into the library directory. The registration
    // file declares `import-dirs: <install>/lib`, so the `.bhi` files (built
    // into `<build>/hi`) must be copied there for consumers to resolve imports.
    install_interfaces(&hi_dir, &lib_install_dir)?;

    // Generate registration file
    let exposed_modules = &lib_config.exposed_modules;
    let reg_content = generate_registration_file(
        name,
        version,
        &package_id,
        &pkg_install_dir,
        &config.dependency_ids,
        exposed_modules,
    );

    let reg_file = pkg_install_dir.join(format!("{}.conf", package_id));
    std::fs::write(&reg_file, reg_content).map_err(|e| BhcNativeError::Io {
        message: format!("failed to write registration file: {}", reg_file.display()),
        source: e,
    })?;

    let duration = start.elapsed();

    output.status(
        "Built",
        &format!(
            "{}-{} ({} modules in {:.2}s)",
            name,
            version,
            modules_compiled,
            duration.as_secs_f64()
        ),
    );

    Ok(PackageBuildResult {
        package_id,
        registration_file: reg_file,
        library_path: lib_path,
        success: true,
        duration,
        modules_compiled,
        warnings: all_warnings,
        errors: all_errors,
    })
}

/// Copy compiled `.bhi` interface files from the build interface directory into
/// the package's installed library directory, preserving the module-path layout
/// (`hi/Data/List.bhi` -> `lib/Data/List.bhi`).
///
/// The registration file's `import-dirs` points at the library directory, so the
/// interfaces must live there for consumers to resolve imports.
fn install_interfaces(hi_dir: &Path, lib_dir: &Path) -> Result<(), BhcNativeError> {
    fn copy_bhi(dir: &Path, base: &Path, lib: &Path) -> std::io::Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                copy_bhi(&path, base, lib)?;
            } else if path.extension().is_some_and(|e| e == "bhi") {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                let dest = lib.join(rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&path, &dest)?;
            }
        }
        Ok(())
    }

    copy_bhi(hi_dir, hi_dir, lib_dir).map_err(|e| BhcNativeError::Io {
        message: format!(
            "failed to install interface files from {} to {}",
            hi_dir.display(),
            lib_dir.display()
        ),
        source: e,
    })
}

/// Create a static library from object files.
///
/// Uses `libtool -static` on macOS or `ar rcs` on Linux.
async fn create_static_library(
    output_path: &Path,
    object_files: &[PathBuf],
) -> Result<(), BhcNativeError> {
    let obj_strs: Vec<String> = object_files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let output = if cfg!(target_os = "macos") {
        let mut args = vec![
            "-static".to_string(),
            "-o".to_string(),
            output_path.to_string_lossy().to_string(),
        ];
        args.extend(obj_strs);
        tokio::process::Command::new("libtool")
            .args(&args)
            .output()
            .await
    } else {
        let mut args = vec!["rcs".to_string(), output_path.to_string_lossy().to_string()];
        args.extend(obj_strs);
        tokio::process::Command::new("ar")
            .args(&args)
            .output()
            .await
    };

    let output = output.map_err(|e| BhcNativeError::Io {
        message: format!("failed to create static library: {}", e),
        source: e,
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BhcNativeError::LinkingFailed(format!(
            "library creation failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Compute a deterministic package ID.
///
/// The ID is derived from the package name, version, BHC version, profile,
/// and the sorted set of dependency IDs. This ensures that any change in
/// the dependency graph or compiler produces a distinct identifier,
/// enabling content-addressable caching.
///
/// The format is: `<name>-<version>-<hash12>` where `hash12` is the first
/// 12 hex characters of a SHA-256 digest.
pub fn compute_package_id(
    name: &str,
    version: &str,
    bhc_version: &str,
    profile: &str,
    dependency_ids: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b"-");
    hasher.update(version.as_bytes());
    hasher.update(b"-");
    hasher.update(bhc_version.as_bytes());
    hasher.update(b"-");
    hasher.update(profile.as_bytes());
    let mut sorted_deps = dependency_ids.to_vec();
    sorted_deps.sort();
    for dep in &sorted_deps {
        hasher.update(b"-");
        hasher.update(dep.as_bytes());
    }
    let hash = hex::encode(hasher.finalize());
    format!("{}-{}-{}", name, version, &hash[..12])
}

/// Generate a package registration (.conf) file.
///
/// Produces a GHC-compatible package database entry that can be consumed by
/// `ghc-pkg register` or placed directly in a package database directory.
pub fn generate_registration_file(
    name: &str,
    version: &str,
    package_id: &str,
    install_dir: &Path,
    dependency_ids: &[String],
    exposed_modules: &[String],
) -> String {
    let lib_dir = install_dir.join("lib");
    let depends_str = dependency_ids.join(" ");
    let modules_str = exposed_modules.join(" ");

    format!(
        "name: {name}\n\
         version: {version}\n\
         id: {package_id}\n\
         key: {package_id}\n\
         library-dirs: {lib_dir}\n\
         hs-libraries: HS{package_id}\n\
         import-dirs: {lib_dir}\n\
         depends: {depends_str}\n\
         exposed-modules: {modules_str}\n",
        name = name,
        version = version,
        package_id = package_id,
        lib_dir = lib_dir.display(),
        depends_str = depends_str,
        modules_str = modules_str,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end transitive build (milestone 3 core): build a two-level local
    /// dependency chain (Data.Base <- Data.Mid) into a package database in
    /// topological order, with each package compiled against the in-progress DB,
    /// then `bhc check` a target that imports the top of the chain.
    ///
    /// This exercises the part of the full-native pipeline that does not need
    /// Hackage: per-package `bhc -c` into a shared DB, registration, transitive
    /// dependency resolution during compilation, and the consumer resolving the
    /// whole `depends:` closure. Gated on `BHC_PATH`; skipped when unset.
    #[tokio::test]
    async fn test_transitive_dependency_build_and_check() {
        use crate::package_db::BhcPackageDb;
        use hx_solver::cabal::parse_cabal_full;
        use hx_solver::extract::ExtractedPackage;
        use hx_solver::version::Version;
        use std::str::FromStr;

        let Ok(bhc_path) = std::env::var("BHC_PATH") else {
            eprintln!("skipping: BHC_PATH not set");
            return;
        };
        let bhc_path = PathBuf::from(bhc_path);

        let root = tempfile::tempdir().unwrap();
        let cache_dir = root.path().join("cache");
        let bhc_version = "2026.2.0";

        // Lay out two dependency packages and a consumer.
        let write = |rel: &str, content: &str| {
            let p = root.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
            p
        };
        write(
            "libbase/base.cabal",
            "name: libbase\nversion: 1.0.0\nbuild-type: Simple\n\nlibrary\n  exposed-modules: Data.Base\n  hs-source-dirs: src\n",
        );
        write(
            "libbase/src/Data/Base.hs",
            "module Data.Base (baseVal) where\nbaseVal :: Int\nbaseVal = 10\n",
        );
        write(
            "libmid/mid.cabal",
            "name: libmid\nversion: 1.0.0\nbuild-type: Simple\n\nlibrary\n  exposed-modules: Data.Mid\n  hs-source-dirs: src\n",
        );
        // Data.Mid imports Data.Base: compiling it REQUIRES resolving the
        // already-built dependency through the package database.
        write(
            "libmid/src/Data/Mid.hs",
            "module Data.Mid (midVal) where\nimport Data.Base (baseVal)\nmidVal :: Int\nmidVal = baseVal + 1\n",
        );
        let main_hs = write(
            "app/Main.hs",
            "module Main (main) where\nimport Data.Mid (midVal)\nmain :: IO ()\nmain = print midVal\n",
        );

        let mut db = BhcPackageDb::open(bhc_version, &cache_dir).await.unwrap();

        // Build one package against the in-progress DB and register it; returns
        // the package id. Mirrors BhcFullNativeBuilder::build_dependency.
        async fn build_and_register(
            db: &mut BhcPackageDb,
            bhc_path: &std::path::Path,
            bhc_version: &str,
            cache_dir: &std::path::Path,
            pkg_root: &std::path::Path,
            cabal: &str,
            dep_ids: Vec<String>,
        ) -> String {
            let build_info = parse_cabal_full(cabal);
            let name = build_info.name.clone();
            let extracted = ExtractedPackage {
                name: name.clone(),
                version: Version::from_str("1.0.0").unwrap(),
                source_dir: pkg_root.to_path_buf(),
                cabal_file: pkg_root.join(format!("{name}.cabal")),
                build_info,
            };
            let config = BhcPackageBuildConfig {
                bhc: BhcCompilerConfig {
                    bhc_path: bhc_path.to_path_buf(),
                    version: bhc_version.to_string(),
                    profile: hx_config::BhcProfile::default(),
                    // The live DB makes already-built deps resolvable.
                    package_dbs: vec![db.db_path().to_path_buf()],
                    packages: Vec::new(),
                    tensor_fusion: false,
                    emit_kernel_report: false,
                },
                build_dir: cache_dir.join("builds").join(&name),
                install_dir: cache_dir.join(format!("bhc-{bhc_version}")).join("lib"),
                dependency_ids: dep_ids,
                jobs: 1,
                optimization: 0,
                verbose: false,
            };
            let result = build_package(&extracted, &config, &Output::new())
                .await
                .expect("build_package failed");
            assert!(result.success, "{} build failed: {:?}", name, result.errors);
            db.register(&result.registration_file).await.unwrap()
        }

        // Topological order: base first, then mid (which depends on base).
        let base_cabal = std::fs::read_to_string(root.path().join("libbase/base.cabal")).unwrap();
        let mid_cabal = std::fs::read_to_string(root.path().join("libmid/mid.cabal")).unwrap();
        let base_id = build_and_register(
            &mut db,
            &bhc_path,
            bhc_version,
            &cache_dir,
            &root.path().join("libbase"),
            &base_cabal,
            Vec::new(),
        )
        .await;
        let mid_id = build_and_register(
            &mut db,
            &bhc_path,
            bhc_version,
            &cache_dir,
            &root.path().join("libmid"),
            &mid_cabal,
            vec![base_id.clone()],
        )
        .await;

        // The consumer imports Data.Mid; exposing libmid must transitively pull
        // in libbase via the registered `depends:`.
        let output = std::process::Command::new(&bhc_path)
            .arg("check")
            .arg(&main_hs)
            .arg("--package-db")
            .arg(db.db_path())
            .arg("--package-id")
            .arg(&mid_id)
            .output()
            .expect("failed to run bhc check");
        assert!(
            output.status.success(),
            "consumer check against transitive DB failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// End-to-end: build a local package with the real BHC backend into a
    /// package database, then have `bhc check` resolve a consumer against it.
    ///
    /// Gated on the `BHC_PATH` environment variable (path to a `bhc` binary);
    /// skipped when unset so CI without BHC stays green.
    #[tokio::test]
    async fn test_build_package_then_check_consumer() {
        use hx_solver::cabal::parse_cabal_full;
        use hx_solver::extract::ExtractedPackage;
        use hx_solver::version::Version;
        use std::str::FromStr;

        let Ok(bhc_path) = std::env::var("BHC_PATH") else {
            eprintln!("skipping: BHC_PATH not set");
            return;
        };
        let bhc_path = PathBuf::from(bhc_path);

        let root = tempfile::tempdir().unwrap();
        let pkg_dir = root.path().join("mysplit");
        let src_data = pkg_dir.join("src").join("Data");
        std::fs::create_dir_all(&src_data).unwrap();

        // A minimal library package depending only on builtins.
        let cabal = "\
name: mysplit
version: 1.0.0
build-type: Simple

library
  exposed-modules: Data.Split
  hs-source-dirs: src
";
        let cabal_path = pkg_dir.join("mysplit.cabal");
        std::fs::write(&cabal_path, cabal).unwrap();
        std::fs::write(
            src_data.join("Split.hs"),
            "module Data.Split (splitOnComma) where\n\
             splitOnComma :: String -> [String]\n\
             splitOnComma s = case break (== ',') s of\n  \
               (a, []) -> [a]\n  \
               (a, _ : rest) -> a : splitOnComma rest\n",
        )
        .unwrap();

        let build_info = parse_cabal_full(cabal);
        let extracted = ExtractedPackage {
            name: "mysplit".to_string(),
            version: Version::from_str("1.0.0").unwrap(),
            source_dir: pkg_dir.clone(),
            cabal_file: cabal_path,
            build_info,
        };

        let bhc = BhcCompilerConfig {
            bhc_path: bhc_path.clone(),
            version: "2026.2.0".to_string(),
            profile: hx_config::BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        };
        let config = BhcPackageBuildConfig {
            bhc,
            build_dir: root.path().join("build"),
            install_dir: root.path().join("install"),
            dependency_ids: Vec::new(),
            jobs: 1,
            optimization: 0,
            verbose: false,
        };

        let result = build_package(&extracted, &config, &Output::new())
            .await
            .expect("build_package should succeed");
        assert!(
            result.success,
            "package build failed: {:?}",
            result.errors
        );

        // The interface must be installed where the .conf's import-dirs points.
        let install_lib = config.install_dir.join(&result.package_id).join("lib");
        assert!(
            install_lib.join("Data").join("Split.bhi").exists(),
            "interface not installed into import-dirs ({})",
            install_lib.display()
        );

        // The package DB directory holds the .conf registration file.
        let db_dir = result.registration_file.parent().unwrap();

        // A consumer whose dependency source is absent resolves via the DB.
        let app = root.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let main_hs = app.join("Main.hs");
        std::fs::write(
            &main_hs,
            "module Main (main) where\n\
             import Data.Split (splitOnComma)\n\
             main :: IO ()\n\
             main = mapM_ putStrLn (splitOnComma \"a,b,c\")\n",
        )
        .unwrap();

        let run_check = |extra: &[&str]| -> std::process::Output {
            std::process::Command::new(&bhc_path)
                .arg("check")
                .arg(&main_hs)
                .arg("--package-db")
                .arg(db_dir)
                .args(extra)
                .output()
                .expect("failed to run bhc check")
        };

        // Resolves with no scoping (all DB packages visible).
        let output = run_check(&[]);
        assert!(
            output.status.success(),
            "bhc check failed against hx-built DB:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // Resolves when the dependency's package id is explicitly exposed.
        let output = run_check(&["--package-id", &result.package_id]);
        assert!(
            output.status.success(),
            "bhc check failed with explicit --package-id:\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Fails when only an unrelated package id is exposed (scoping hides it).
        let output = run_check(&["--package-id", "nonexistent-0.0-deadbeef"]);
        assert!(
            !output.status.success(),
            "bhc check unexpectedly succeeded though the dependency package was not exposed"
        );
    }

    #[test]
    fn test_compute_package_id() {
        let id1 = compute_package_id("text", "2.1.0", "2026.2.0", "default", &[]);
        let id2 = compute_package_id("text", "2.1.0", "2026.2.0", "default", &[]);

        // Deterministic: same inputs produce the same output
        assert_eq!(id1, id2);

        // Format check
        assert!(id1.starts_with("text-2.1.0-"));
        // Hash portion is 12 hex chars
        let hash_part = id1.strip_prefix("text-2.1.0-").unwrap();
        assert_eq!(hash_part.len(), 12);
        assert!(hash_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_compute_package_id_changes_with_deps() {
        let id_no_deps = compute_package_id("text", "2.1.0", "2026.2.0", "default", &[]);
        let id_with_deps = compute_package_id(
            "text",
            "2.1.0",
            "2026.2.0",
            "default",
            &["base-4.18.0-abc123".to_string()],
        );

        assert_ne!(id_no_deps, id_with_deps);
    }

    #[test]
    fn test_compute_package_id_changes_with_profile() {
        let id_default = compute_package_id("text", "2.1.0", "2026.2.0", "default", &[]);
        let id_numeric = compute_package_id("text", "2.1.0", "2026.2.0", "numeric", &[]);

        assert_ne!(id_default, id_numeric);
    }

    #[test]
    fn test_compute_package_id_dep_order_invariant() {
        let deps_a = vec!["base-1.0-aaa".to_string(), "text-2.0-bbb".to_string()];
        let deps_b = vec!["text-2.0-bbb".to_string(), "base-1.0-aaa".to_string()];

        let id_a = compute_package_id("aeson", "2.2.0", "2026.2.0", "default", &deps_a);
        let id_b = compute_package_id("aeson", "2.2.0", "2026.2.0", "default", &deps_b);

        // Sorted internally, so order should not matter
        assert_eq!(id_a, id_b);
    }

    #[test]
    fn test_generate_registration_file() {
        let content = generate_registration_file(
            "text",
            "2.1.0",
            "text-2.1.0-abc123def456",
            Path::new("/home/user/.cache/hx/store/text-2.1.0-abc123def456"),
            &[
                "base-4.18.0-xyz789".to_string(),
                "bytestring-0.12.0-def456".to_string(),
            ],
            &[
                "Data.Text".to_string(),
                "Data.Text.IO".to_string(),
                "Data.Text.Lazy".to_string(),
            ],
        );

        assert!(content.contains("name: text"));
        assert!(content.contains("version: 2.1.0"));
        assert!(content.contains("id: text-2.1.0-abc123def456"));
        assert!(content.contains("key: text-2.1.0-abc123def456"));
        assert!(content.contains("hs-libraries: HStext-2.1.0-abc123def456"));
        assert!(content.contains("depends: base-4.18.0-xyz789 bytestring-0.12.0-def456"));
        assert!(content.contains("exposed-modules: Data.Text Data.Text.IO Data.Text.Lazy"));

        // Library and import dirs should point to install_dir/lib
        assert!(content.contains("library-dirs:"));
        assert!(content.contains("import-dirs:"));
        // Build the expected path with `join` so the separator matches the
        // platform (generation uses join too); a hardcoded `/lib` fails on
        // Windows where the rendered path ends in `\lib`.
        let lib_path = Path::new("/home/user/.cache/hx/store/text-2.1.0-abc123def456").join("lib");
        assert!(content.contains(&lib_path.display().to_string()));
    }

    #[test]
    fn test_generate_registration_file_no_deps() {
        let content = generate_registration_file(
            "base",
            "4.18.0",
            "base-4.18.0-aaa111bbb222",
            Path::new("/opt/bhc/lib"),
            &[],
            &["Prelude".to_string(), "Data.List".to_string()],
        );

        assert!(content.contains("name: base"));
        assert!(content.contains("depends: \n") || content.contains("depends: "));
        assert!(content.contains("exposed-modules: Prelude Data.List"));
    }
}
