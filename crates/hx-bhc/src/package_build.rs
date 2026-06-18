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
