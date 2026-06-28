//! BHC native builder for local project compilation.
//!
//! Provides parallel module compilation using the BHC compiler, driven by
//! the module dependency graph. Modules within the same parallel group are
//! compiled concurrently, respecting a configurable concurrency limit.

use crate::compile::compile_module;
use crate::native::{BhcCompilerConfig, BhcNativeBuildOptions, BhcNativeError};
use hx_cache::artifacts::{hash_file, retrieve_artifacts, store_artifacts};
use hx_solver::build_module_graph;
use hx_ui::Output;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Result of a native BHC build.
#[derive(Debug)]
pub struct BhcNativeBuildResult {
    /// Whether the overall build succeeded.
    pub success: bool,
    /// Total wall-clock duration of the build.
    pub duration: Duration,
    /// Number of modules that were compiled.
    pub modules_compiled: usize,
    /// Number of modules that were skipped (up-to-date).
    pub modules_skipped: usize,
    /// Path to the linked executable, if produced.
    pub executable: Option<PathBuf>,
    /// Path to the static library, if produced.
    pub library: Option<PathBuf>,
    /// Warning messages collected across all modules.
    pub warnings: Vec<String>,
    /// Error messages collected across all modules.
    pub errors: Vec<String>,
}

/// BHC native builder for compiling local projects.
///
/// Orchestrates module-level compilation using the dependency graph to
/// maximise parallelism while respecting inter-module dependencies.
pub struct BhcNativeBuilder {
    bhc: BhcCompilerConfig,
    /// Optional cache directory for per-module artifact caching.
    cache_dir: Option<PathBuf>,
}

impl BhcNativeBuilder {
    /// Create a new native builder with the given compiler configuration.
    pub fn new(bhc: BhcCompilerConfig) -> Self {
        Self {
            bhc,
            cache_dir: None,
        }
    }

    /// Set the artifact cache directory for incremental compilation.
    pub fn with_cache_dir(mut self, cache_dir: PathBuf) -> Self {
        self.cache_dir = Some(cache_dir);
        self
    }

    /// Build a project using native BHC compilation.
    ///
    /// Steps:
    /// 1. Create output directories for interface and object files.
    /// 2. Build the module dependency graph from source directories.
    /// 3. Compile modules in parallel groups (topological layers).
    /// 4. Link the executable or create a static library if configured.
    pub async fn build(
        &self,
        project_root: &Path,
        options: &BhcNativeBuildOptions,
        output: &Output,
    ) -> Result<BhcNativeBuildResult, BhcNativeError> {
        let start = Instant::now();

        info!("Starting BHC native build in {}", project_root.display());

        // 1. Create output directories
        let build_dir = project_root.join(&options.output_dir);
        let hi_dir = build_dir.join("hi");
        let o_dir = build_dir.join("o");

        for dir in [&build_dir, &hi_dir, &o_dir] {
            std::fs::create_dir_all(dir).map_err(|e| BhcNativeError::Io {
                message: format!("failed to create build directory {}: {}", dir.display(), e),
                source: e,
            })?;
        }

        // 2. Build module graph
        let graph = build_module_graph(&options.src_dirs).map_err(|e| {
            BhcNativeError::CompilationFailed(format!("failed to build module graph: {}", e))
        })?;

        let groups = graph.parallel_groups().map_err(|e| {
            BhcNativeError::CompilationFailed(format!(
                "failed to compute parallel compilation groups: {}",
                e
            ))
        })?;

        info!(
            "Module graph: {} modules in {} parallel groups",
            graph.modules.len(),
            groups.len()
        );

        // 3. Compile modules in parallel groups (with per-module artifact caching)
        let semaphore = Arc::new(Semaphore::new(options.jobs));
        let mut all_warnings = Vec::new();
        let mut all_errors = Vec::new();
        let mut modules_compiled: usize = 0;
        let mut modules_skipped: usize = 0;
        let mut build_failed = false;

        let cache_flags = vec![
            format!("-O{}", options.optimization),
            format!("--profile={}", self.bhc.profile.as_str()),
        ];
        let empty_deps: HashMap<String, String> = HashMap::new();

        for (group_idx, group) in groups.iter().enumerate() {
            debug!(
                "Compiling group {}/{}: {:?}",
                group_idx + 1,
                groups.len(),
                group
            );

            let mut handles = Vec::new();

            for module_name in group {
                let module_info = match graph.modules.get(module_name) {
                    Some(info) => info.clone(),
                    None => {
                        warn!("Module {} not found in graph, skipping", module_name);
                        continue;
                    }
                };

                // Check per-module artifact cache
                if let Some(ref cache_dir) = self.cache_dir
                    && let Ok(source_hash) = hash_file(&module_info.path)
                    && let Ok(Some(_)) = retrieve_artifacts(
                        cache_dir,
                        &source_hash,
                        &self.bhc.version,
                        &cache_flags,
                        &empty_deps,
                        &build_dir,
                    )
                {
                    debug!("Cache hit for module {}", module_name);
                    modules_skipped += 1;
                    continue;
                }

                let sem = semaphore.clone();
                let bhc = BhcCompilerConfig {
                    bhc_path: self.bhc.bhc_path.clone(),
                    version: self.bhc.version.clone(),
                    profile: self.bhc.profile,
                    package_dbs: self.bhc.package_dbs.clone(),
                    packages: self.bhc.packages.clone(),
                    tensor_fusion: self.bhc.tensor_fusion,
                    emit_kernel_report: self.bhc.emit_kernel_report,
                };
                let module_name = module_name.clone();
                let source_file = module_info.path.clone();
                let build_dir_clone = build_dir.clone();
                let opts = options.clone();
                let cache_dir_opt = self.cache_dir.clone();
                let cache_flags_clone = cache_flags.clone();
                let bhc_version = self.bhc.version.clone();

                let handle = tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed unexpectedly");

                    let result = compile_module(
                        &bhc,
                        &module_name,
                        &source_file,
                        &build_dir_clone,
                        &opts,
                        &[],
                    )
                    .await;

                    // Store successful compilation results in artifact cache
                    if let Ok(ref compile_result) = result
                        && compile_result.success
                        && let Some(ref cache_dir) = cache_dir_opt
                        && let Ok(source_hash) = hash_file(&source_file)
                    {
                        let artifact_files: Vec<PathBuf> = vec![
                            compile_result.object_file.clone(),
                            compile_result.interface_file.clone(),
                        ];
                        let empty: HashMap<String, String> = HashMap::new();
                        let _ = store_artifacts(
                            cache_dir,
                            &module_name,
                            &source_hash,
                            &bhc_version,
                            &cache_flags_clone,
                            &empty,
                            &artifact_files,
                        );
                    }

                    result
                });

                handles.push(handle);
            }

            // Collect results from all tasks in this group
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

            // Stop processing groups if any module in this group failed
            if build_failed {
                break;
            }
        }

        if build_failed {
            let duration = start.elapsed();
            return Ok(BhcNativeBuildResult {
                success: false,
                duration,
                modules_compiled,
                modules_skipped,
                executable: None,
                library: None,
                warnings: all_warnings,
                errors: all_errors,
            });
        }

        // 4. Link or create library
        let mut executable = None;
        let mut library = None;

        if options.main_module.is_some()
            && let Some(ref exe_path) = options.output_exe
        {
            info!("Linking executable: {}", exe_path.display());
            self.link(&build_dir, exe_path, &[]).await?;
            executable = Some(exe_path.clone());
        }

        if let Some(ref lib_path) = options.output_lib {
            info!("Creating library: {}", lib_path.display());
            self.create_library(&build_dir, lib_path).await?;
            library = Some(lib_path.clone());
        }

        let duration = start.elapsed();

        let skip_msg = if modules_skipped > 0 {
            format!(", {} cached", modules_skipped)
        } else {
            String::new()
        };
        output.status(
            "Finished",
            &format!(
                "BHC build: {} modules compiled{} in {:.2}s",
                modules_compiled,
                skip_msg,
                duration.as_secs_f64()
            ),
        );

        Ok(BhcNativeBuildResult {
            success: true,
            duration,
            modules_compiled,
            modules_skipped,
            executable,
            library,
            warnings: all_warnings,
            errors: all_errors,
        })
    }

    /// Link object files into an executable.
    ///
    /// Invokes BHC with all collected object files and dependency package IDs
    /// to produce a final executable.
    async fn link(
        &self,
        build_dir: &Path,
        output_path: &Path,
        dependency_ids: &[String],
    ) -> Result<(), BhcNativeError> {
        let object_files = collect_object_files(build_dir);

        if object_files.is_empty() {
            return Err(BhcNativeError::LinkingFailed(
                "no object files found to link".to_string(),
            ));
        }

        let mut args = Vec::new();

        // Add all object files
        for obj in &object_files {
            args.push(obj.to_string_lossy().to_string());
        }

        // Output path
        args.push("-o".to_string());
        args.push(output_path.to_string_lossy().to_string());

        // Package databases
        for db in &self.bhc.package_dbs {
            args.push("--package-db".to_string());
            args.push(db.to_string_lossy().to_string());
        }

        // Dependency package IDs (BHC spells this `--package-id`).
        for dep_id in dependency_ids {
            args.push("--package-id".to_string());
            args.push(dep_id.clone());
        }

        debug!("Link args: {:?}", args);

        let output = tokio::process::Command::new(&self.bhc.bhc_path)
            .args(&args)
            .output()
            .await
            .map_err(|e| BhcNativeError::Io {
                message: format!("failed to execute BHC linker: {}", e),
                source: e,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BhcNativeError::LinkingFailed(format!(
                "BHC linker exited with {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        Ok(())
    }

    /// Create a static library from object files.
    ///
    /// Uses `ar rcs` on Linux or `libtool -static` on macOS.
    async fn create_library(
        &self,
        build_dir: &Path,
        output_path: &Path,
    ) -> Result<(), BhcNativeError> {
        let object_files = collect_object_files(build_dir);

        if object_files.is_empty() {
            return Err(BhcNativeError::LinkingFailed(
                "no object files found to create library".to_string(),
            ));
        }

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
}

/// Collect all `.o` object files from the build directory.
///
/// Searches recursively under `<build_dir>/o/` for files ending in `.o`.
pub fn collect_object_files(build_dir: &Path) -> Vec<PathBuf> {
    let o_dir = build_dir.join("o");
    let mut object_files = Vec::new();

    if let Ok(entries) = walkdir(&o_dir) {
        for entry in entries {
            if entry.extension().is_some_and(|ext| ext == "o") {
                object_files.push(entry);
            }
        }
    }

    object_files.sort();
    object_files
}

/// Recursively walk a directory and return all file paths.
fn walkdir(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();

    if !dir.exists() {
        return Ok(files);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(walkdir(&path)?);
        } else {
            files.push(path);
        }
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_config::BhcProfile;
    use tempfile::TempDir;

    #[test]
    fn test_bhc_native_builder_new() {
        let config = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        };

        let builder = BhcNativeBuilder::new(config);
        assert_eq!(builder.bhc.bhc_path, PathBuf::from("bhc"));
        assert_eq!(builder.bhc.version, "2026.2.0");
    }

    #[test]
    fn test_collect_object_files() {
        let temp = TempDir::new().unwrap();
        let o_dir = temp.path().join("o");
        std::fs::create_dir_all(o_dir.join("Data")).unwrap();

        // Create some .o files
        std::fs::write(o_dir.join("Main.o"), b"").unwrap();
        std::fs::write(o_dir.join("Data").join("List.o"), b"").unwrap();

        // Create a non-.o file that should be ignored
        std::fs::write(o_dir.join("Main.hi"), b"").unwrap();

        let files = collect_object_files(temp.path());

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.ends_with("Main.o")));
        assert!(files.iter().any(|f| f.ends_with("List.o")));
    }

    #[test]
    fn test_collect_object_files_empty_dir() {
        let temp = TempDir::new().unwrap();
        let files = collect_object_files(temp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn test_collect_object_files_no_dir() {
        let files = collect_object_files(Path::new("/nonexistent/path"));
        assert!(files.is_empty());
    }
}
