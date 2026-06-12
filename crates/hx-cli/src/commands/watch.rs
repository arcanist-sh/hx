//! Watch mode implementation for auto-rebuild on file changes.

use anyhow::{Context, Result};
use hx_config::{Project, find_project_root};
use hx_toolchain::AutoInstallPolicy;
use hx_ui::Output;
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tracing::info;

use super::build;

/// Default debounce delay in milliseconds.
const DEFAULT_DEBOUNCE_MS: u64 = 500;

/// Run the watch command.
pub async fn run(
    test: bool,
    clear: bool,
    debounce_ms: Option<u64>,
    package: Option<String>,
    policy: AutoInstallPolicy,
    output: &Output,
) -> Result<i32> {
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    let debounce = Duration::from_millis(debounce_ms.unwrap_or(DEFAULT_DEBOUNCE_MS));

    output.header("Watch Mode");
    output.status("Watching", project.name());
    output.info(&format!(
        "Monitoring for changes (debounce: {}ms)",
        debounce.as_millis()
    ));

    if test {
        output.info("Will run tests on change");
    } else {
        output.info("Will rebuild on change");
    }

    output.info("Press Ctrl+C to stop\n");

    // Perform initial build/test
    if clear {
        clear_terminal();
    }

    let start = Instant::now();
    let result = run_action(test, package.clone(), policy, output).await;
    print_build_status(result, start.elapsed(), output);

    // Set up file watcher
    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(debounce, tx).context("Failed to create file watcher")?;

    // Watch source directories
    let src_dirs = collect_watch_paths(&project_root, &project);
    for dir in &src_dirs {
        if dir.exists() {
            debouncer
                .watcher()
                .watch(dir, RecursiveMode::Recursive)
                .with_context(|| format!("Failed to watch directory: {}", dir.display()))?;
            info!("Watching: {}", dir.display());
        }
    }

    // Also watch config files
    let config_files = vec![
        project_root.join("hx.toml"),
        project_root.join("cabal.project"),
    ];
    for file in &config_files {
        if file.exists() {
            debouncer
                .watcher()
                .watch(file, RecursiveMode::NonRecursive)
                .with_context(|| format!("Failed to watch file: {}", file.display()))?;
            info!("Watching config: {}", file.display());
        }
    }

    // Watch .cabal files
    if let Ok(entries) = std::fs::read_dir(&project_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "cabal") {
                debouncer
                    .watcher()
                    .watch(&path, RecursiveMode::NonRecursive)
                    .with_context(|| format!("Failed to watch file: {}", path.display()))?;
                info!("Watching cabal: {}", path.display());
            }
        }
    }

    // Main watch loop
    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                // Filter for relevant events
                let relevant_events: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        matches!(
                            e.kind,
                            DebouncedEventKind::Any | DebouncedEventKind::AnyContinuous
                        ) && is_relevant_file(&e.path)
                    })
                    .collect();

                if !relevant_events.is_empty() {
                    // Get changed files for display
                    let changed_files: Vec<_> = relevant_events
                        .iter()
                        .map(|e| {
                            e.path
                                .strip_prefix(&project_root)
                                .unwrap_or(&e.path)
                                .display()
                                .to_string()
                        })
                        .collect();

                    if clear {
                        clear_terminal();
                    }

                    output.warn(&format!(
                        "Change detected: {}",
                        changed_files.first().unwrap_or(&"unknown".to_string())
                    ));

                    if changed_files.len() > 1 {
                        output.info(&format!("  (+{} more files)", changed_files.len() - 1));
                    }

                    println!();

                    let start = Instant::now();
                    let result = run_action(test, package.clone(), policy, output).await;
                    print_build_status(result, start.elapsed(), output);
                }
            }
            Ok(Err(error)) => {
                output.error(&format!("Watch error: {}", error));
            }
            Err(e) => {
                output.error(&format!("Channel error: {}", e));
                break;
            }
        }
    }

    Ok(0)
}

/// Run build or test action.
async fn run_action(
    test: bool,
    package: Option<String>,
    policy: AutoInstallPolicy,
    output: &Output,
) -> Result<i32> {
    if test {
        build::test(None, package, None, None, policy, output).await
    } else {
        build::run(false, None, None, package, false, None, policy, output).await
    }
}

/// Print build status with timing.
fn print_build_status(result: Result<i32>, elapsed: Duration, output: &Output) {
    println!();

    match result {
        Ok(0) => {
            output.status(
                "Success",
                &format!("Build completed in {:.2}s", elapsed.as_secs_f64()),
            );
            output.info("Waiting for changes...\n");
        }
        Ok(code) => {
            output.error(&format!(
                "Build failed (exit code: {}) in {:.2}s",
                code,
                elapsed.as_secs_f64()
            ));
            output.info("Waiting for changes...\n");
        }
        Err(e) => {
            output.error(&format!(
                "Build error: {} ({:.2}s)",
                e,
                elapsed.as_secs_f64()
            ));
            output.info("Waiting for changes...\n");
        }
    }
}

/// Collect paths to watch for a project.
fn collect_watch_paths(project_root: &std::path::Path, project: &Project) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Standard source directories
    let standard_dirs = ["src", "app", "lib", "test", "tests", "bench", "benchmarks"];
    for dir in &standard_dirs {
        let path = project_root.join(dir);
        if path.exists() {
            paths.push(path);
        }
    }

    // Source dirs from manifest
    for src_dir in &project.manifest.build.src_dirs {
        let path = project_root.join(src_dir);
        if path.exists() && !paths.contains(&path) {
            paths.push(path);
        }
    }

    // If no paths found, watch the project root
    if paths.is_empty() {
        paths.push(project_root.to_path_buf());
    }

    paths
}

/// Check if a file is relevant for triggering rebuilds.
fn is_relevant_file(path: &std::path::Path) -> bool {
    // Never react to build outputs or metadata: when the project root is
    // watched recursively, generated files would otherwise trigger an
    // endless rebuild loop
    if path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("dist-newstyle" | "dist" | ".hx" | ".git" | ".stack-work")
        )
    }) {
        return false;
    }

    let Some(extension) = path.extension() else {
        // Check for files without extensions that matter
        return path
            .file_name()
            .is_some_and(|name| name == "hx.toml" || name == "cabal.project");
    };

    let ext = extension.to_string_lossy().to_lowercase();

    matches!(
        ext.as_str(),
        "hs" | "lhs" | "hsc" | "chs" | "cabal" | "toml" | "yaml" | "c" | "h"
    )
}

/// Clear the terminal screen.
fn clear_terminal() {
    // Use ANSI escape codes for cross-platform clearing
    print!("\x1B[2J\x1B[1;1H");
    // Flush stdout
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_relevant_file() {
        assert!(is_relevant_file(std::path::Path::new("src/Main.hs")));
        assert!(is_relevant_file(std::path::Path::new("lib/Lib.lhs")));
        assert!(is_relevant_file(std::path::Path::new("project.cabal")));
        assert!(is_relevant_file(std::path::Path::new("hx.toml")));
        assert!(is_relevant_file(std::path::Path::new("cbits/ffi.c")));
        assert!(!is_relevant_file(std::path::Path::new("file.txt")));
        assert!(!is_relevant_file(std::path::Path::new("image.png")));
    }
}
