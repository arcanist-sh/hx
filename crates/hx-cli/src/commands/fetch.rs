//! Fetch command implementation.

use anyhow::Result;
use hx_cabal::fetch::{self, FetchOptions};
use hx_config::{LOCKFILE_FILENAME, Project, find_project_root};
use hx_core::{CommandRunner, EnvVars};
use hx_lock::Lockfile;
use hx_solver::{FetchError, HACKAGE_BASE_URL, default_index_path, fetch_sources};
use hx_ui::{Output, Spinner};

use super::sources;

/// Run the fetch command.
///
/// Pre-downloads all dependencies in parallel, speeding up subsequent builds
/// by separating network I/O from compilation. With a lockfile, every locked
/// source tarball is verified against its pinned hash and handed to cabal so
/// `hx sync --offline` can build without network access.
pub async fn run(jobs: Option<usize>, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Read the lockfile for exact versions and hashes
    let lockfile_path = project.lockfile_path();
    let lockfile = if lockfile_path.exists() {
        match Lockfile::from_file(&lockfile_path) {
            Ok(lock) => Some(lock),
            Err(e) => {
                output.error(&format!("Could not read {}: {}", LOCKFILE_FILENAME, e));
                output.info("fix: Run `hx lock` to regenerate it");
                return Ok(3);
            }
        }
    } else {
        None
    };

    match lockfile {
        Some(lock) if !lock.packages.is_empty() => fetch_locked(&lock, jobs, output).await,
        _ => {
            // No lockfile: let cabal fetch whatever it resolves (unverified).
            output.status("Fetching", "all dependencies");
            output.info(&format!(
                "No {} found; run `hx lock` first for hash-verified fetches",
                LOCKFILE_FILENAME
            ));
            let options = FetchOptions {
                jobs,
                verbose: output.is_verbose(),
                packages: vec![],
            };
            let build_dir = project.cabal_build_dir();
            match fetch::fetch(&project.root, &build_dir, &options, output).await {
                Ok(result) => {
                    if result.packages_fetched > 0 || result.packages_cached > 0 {
                        output.info(&format!(
                            "Ready for build: {} packages available",
                            result.packages_fetched + result.packages_cached
                        ));
                    }
                    Ok(0)
                }
                Err(e) => {
                    output.print_error(&e);
                    Ok(1)
                }
            }
        }
    }
}

/// Download and verify every locked source tarball.
async fn fetch_locked(lock: &Lockfile, jobs: Option<usize>, output: &Output) -> Result<i32> {
    let specs = sources::source_specs(lock);
    output.status(
        "Fetching",
        &format!("{} locked source packages", specs.len()),
    );

    if lock.needs_upgrade() {
        output.warn(&format!(
            "{} uses format v{} and pins no source hashes; downloads cannot be verified",
            LOCKFILE_FILENAME, lock.version
        ));
        output.info("Run `hx lock` to upgrade it and pin source hashes");
    }

    let Some(cache_dir) = hx_solver::default_package_cache_dir() else {
        output.error("could not determine hx's source cache directory (no home directory)");
        output.info("fix: Set HOME and try again");
        return Ok(1);
    };
    let options = hx_solver::FetchOptions {
        jobs: jobs.unwrap_or(8),
        cache_dir: cache_dir.clone(),
        base_url: std::env::var(EnvVars::HX_HACKAGE_URL)
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| HACKAGE_BASE_URL.to_string()),
        ..hx_solver::FetchOptions::default()
    };

    let spinner = Spinner::new(format!(
        "Downloading and verifying {} packages ({} parallel)...",
        specs.len(),
        options.jobs
    ));
    let results = fetch_sources(&specs, &options).await;
    let cabal_repo = hx_solver::cabal_repo_cache_dir();

    let mut downloaded = 0;
    let mut cached = 0;
    let mut unpinned = 0;
    let mut mismatches = Vec::new();
    let mut failures = Vec::new();
    for (spec, result) in &results {
        match result {
            Ok(fetched) => {
                if fetched.cached {
                    cached += 1;
                } else {
                    downloaded += 1;
                }
                if spec.sha256.is_none() {
                    unpinned += 1;
                }
                if let Some(ref repo) = cabal_repo {
                    match hx_solver::seed_cabal_cache(
                        &fetched.path,
                        &fetched.hash,
                        repo,
                        &spec.name,
                        &spec.version,
                    ) {
                        Ok(outcome) => {
                            sources::note_seed(output, &spec.package_id(), outcome, repo)
                        }
                        Err(e) => failures.push(format!(
                            "{}: could not copy into cabal's cache: {}",
                            spec.package_id(),
                            e
                        )),
                    }
                }
            }
            Err(FetchError::HashMismatch {
                package,
                expected,
                actual,
            }) => {
                // A mismatching fresh download is discarded; a mismatching
                // cached copy is left in place for inspection.
                let cached = hx_solver::cached_tarball_path(&cache_dir, &spec.name, &spec.version);
                mismatches.push(sources::Mismatch {
                    id: package.clone(),
                    path: cached.is_file().then_some(cached),
                    expected: expected.clone(),
                    actual: actual.clone(),
                })
            }
            Err(e) => failures.push(format!("{}: {}", spec.package_id(), e)),
        }
    }

    if mismatches.is_empty() && failures.is_empty() {
        spinner.finish_success(format!(
            "Verified {} packages ({} downloaded, {} cached)",
            downloaded + cached,
            downloaded,
            cached
        ));
    } else {
        spinner.finish_error(format!(
            "{} of {} packages failed",
            mismatches.len() + failures.len(),
            results.len()
        ));
    }

    for m in &mismatches {
        sources::report_mismatch(output, m);
    }
    if !failures.is_empty() {
        output.error(&format!(
            "{} package(s) could not be fetched",
            failures.len()
        ));
        for f in &failures {
            output.info(&format!("  - {}", f));
        }
        output.info("fix: Check your network connection and run `hx fetch` again");
    }
    if !mismatches.is_empty() || !failures.is_empty() {
        return Ok(1);
    }
    if unpinned > 0 && !lock.needs_upgrade() {
        output.warn(&format!(
            "{} package(s) have no pinned hash in {}; run `hx lock` to pin them",
            unpinned, LOCKFILE_FILENAME
        ));
    }

    ensure_cabal_index(output).await;
    output.info("Ready for offline builds: run `hx sync --offline`");
    Ok(0)
}

/// Offline builds also need cabal's copy of the Hackage index. Download it
/// with `cabal update` if it is missing and cabal is available.
async fn ensure_cabal_index(output: &Output) {
    if default_index_path().is_some() {
        return;
    }
    let spinner = Spinner::new("Downloading cabal's Hackage index (cabal update)...");
    match CommandRunner::new().run("cabal", ["update"]).await {
        Ok(out) if out.success() => spinner.finish_success("Downloaded cabal's Hackage index"),
        _ => {
            spinner.finish_clear();
            output.warn("cabal's Hackage index is missing and `cabal update` did not succeed");
            output.info("fix: Run `cabal update` before building offline");
        }
    }
}
