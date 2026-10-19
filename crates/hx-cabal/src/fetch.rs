//! Parallel package fetching.
//!
//! This module provides functionality to pre-fetch package dependencies
//! in parallel, improving build times by separating download from compile.

use hx_cache::cabal_store_dir;
use hx_core::{CommandRunner, Error, Fix, Result};
use hx_ui::{Output, Spinner};
use std::path::Path;
use tracing::{debug, info};

/// Options for fetching packages.
#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    /// Number of parallel download jobs
    pub jobs: Option<usize>,
    /// Whether to show verbose output
    pub verbose: bool,
    /// Specific packages to fetch (empty = all dependencies)
    pub packages: Vec<String>,
}

/// Result of a fetch operation.
#[derive(Debug)]
pub struct FetchResult {
    /// Number of packages fetched
    pub packages_fetched: usize,
    /// Number of packages already cached
    pub packages_cached: usize,
}

/// Fetch all dependencies for a project in parallel.
///
/// This runs `cabal fetch` with parallel downloads enabled, pre-downloading
/// all packages needed for a build. This can significantly speed up builds
/// by separating network I/O from compilation.
pub async fn fetch(
    project_root: &Path,
    build_dir: &Path,
    options: &FetchOptions,
    output: &Output,
) -> Result<FetchResult> {
    let store_dir = cabal_store_dir()?;

    let build_dir_str = build_dir.to_string_lossy().to_string();
    let store_dir_str = store_dir.to_string_lossy().to_string();

    // Build command arguments
    // Global options must come before the subcommand
    let mut args = vec![
        format!("--store-dir={}", store_dir_str),
        "fetch".to_string(),
        format!("--builddir={}", build_dir_str),
    ];

    // Add parallel jobs flag (default to 8 for good parallel throughput)
    let jobs = options.jobs.unwrap_or(8);
    args.push(format!("-j{}", jobs));

    // Add specific packages if provided, otherwise fetch all deps
    if options.packages.is_empty() {
        args.push("--dependencies-only".to_string());
    } else {
        args.extend(options.packages.iter().cloned());
    }

    info!(
        "Running cabal fetch in {} with {} jobs",
        project_root.display(),
        jobs
    );
    debug!("Args: {:?}", args);

    let spinner = if !options.verbose {
        Some(Spinner::new(format!(
            "Fetching packages ({} parallel downloads)...",
            jobs
        )))
    } else {
        None
    };

    let runner = CommandRunner::new().with_working_dir(project_root);
    let cmd_output = runner.run("cabal", args.iter().map(|s| s.as_str())).await?;

    // Parse output to count packages
    let result = parse_fetch_output(&cmd_output.stdout, &cmd_output.stderr);

    if let Some(spinner) = spinner {
        if cmd_output.success() {
            if result.packages_fetched > 0 {
                spinner.finish_success(format!(
                    "Fetched {} packages ({} cached)",
                    result.packages_fetched, result.packages_cached
                ));
            } else {
                spinner.finish_success("All packages already cached");
            }
        } else {
            spinner.finish_error("Failed to fetch packages");
        }
    }

    if options.verbose || !cmd_output.success() {
        if !cmd_output.stdout.is_empty() {
            output.verbose(&cmd_output.stdout);
        }
        if !cmd_output.stderr.is_empty() {
            if cmd_output.success() {
                output.verbose(&cmd_output.stderr);
            } else {
                eprintln!("{}", cmd_output.stderr);
            }
        }
    }

    if !cmd_output.success() {
        return Err(Error::CommandFailed {
            command: "cabal fetch".to_string(),
            exit_code: Some(cmd_output.exit_code),
            stdout: cmd_output.stdout,
            stderr: cmd_output.stderr,
            fixes: vec![
                Fix::new("Check your network connection"),
                Fix::with_command("Ensure dependencies are resolved", "hx lock"),
            ],
        });
    }

    Ok(result)
}

/// Fetch dependencies from a lockfile.
///
/// This reads the lockfile and fetches all listed packages in parallel.
pub async fn fetch_from_lockfile(
    project_root: &Path,
    build_dir: &Path,
    lockfile_packages: &[(String, String)], // (name, version) pairs
    options: &FetchOptions,
    output: &Output,
) -> Result<FetchResult> {
    if lockfile_packages.is_empty() {
        output.info("No dependencies to fetch");
        return Ok(FetchResult {
            packages_fetched: 0,
            packages_cached: 0,
        });
    }

    // Build package specifiers (name-version)
    let packages: Vec<String> = lockfile_packages
        .iter()
        .map(|(name, version)| format!("{}-{}", name, version))
        .collect();

    output.verbose(&format!(
        "Fetching {} packages from lockfile",
        packages.len()
    ));

    let opts = FetchOptions {
        packages,
        ..options.clone()
    };

    fetch(project_root, build_dir, &opts, output).await
}

/// Parse cabal fetch output to count fetched vs cached packages.
fn parse_fetch_output(stdout: &str, stderr: &str) -> FetchResult {
    let combined = format!("{}\n{}", stdout, stderr);

    // Count "Downloading" lines for newly fetched packages
    let packages_fetched = combined
        .lines()
        .filter(|line| line.contains("Downloading") || line.contains("downloading"))
        .count();

    // Count "Using cached" or similar for already cached packages
    let packages_cached = combined
        .lines()
        .filter(|line| {
            line.contains("Using cached")
                || line.contains("using cached")
                || line.contains("already downloaded")
        })
        .count();

    FetchResult {
        packages_fetched,
        packages_cached,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fetch_output_downloads() {
        let output = r#"
Downloading text-2.1.1
Downloading aeson-2.2.0.0
Downloading bytestring-0.12.1.0
"#;
        let result = parse_fetch_output(output, "");
        assert_eq!(result.packages_fetched, 3);
        assert_eq!(result.packages_cached, 0);
    }

    #[test]
    fn test_parse_fetch_output_cached() {
        let output = r#"
Using cached text-2.1.1
Using cached aeson-2.2.0.0
Downloading bytestring-0.12.1.0
"#;
        let result = parse_fetch_output(output, "");
        assert_eq!(result.packages_fetched, 1);
        assert_eq!(result.packages_cached, 2);
    }

    #[test]
    fn test_parse_fetch_output_empty() {
        let result = parse_fetch_output("", "");
        assert_eq!(result.packages_fetched, 0);
        assert_eq!(result.packages_cached, 0);
    }
}
