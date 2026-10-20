//! Building with the plan pinned in hx.lock (`hx build`, `test`, `run`, `check`, `repl`).
//!
//! When hx.lock exists and still describes the project, these commands hand
//! cabal the same pins `hx sync` uses: every dependency version and flag, and
//! the Hackage index state. A lockfile that no longer matches is reported and
//! skipped, so editing dependencies never blocks a build. With `--locked`
//! (or `HX_LOCKED=1`) a missing or stale lockfile is an error instead, and
//! cached source tarballs must match their pinned hashes, as in `hx sync`.

use anyhow::Result;
use hx_config::{LOCKFILE_FILENAME, Project};
use hx_core::CommandRunner;
use hx_lock::Lockfile;
use hx_solver::CabalContext;
use hx_ui::{Output, Spinner};
use std::ffi::OsStr;
use std::path::Path;

use super::lock::plan_problems;
use super::sources;

/// How a build treats hx.lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Use hx.lock when it is current; otherwise warn and build without it.
    Auto,
    /// hx.lock must exist and be current (`--locked`).
    Required,
}

impl LockMode {
    pub fn from_flag(locked: bool) -> Self {
        if locked {
            LockMode::Required
        } else {
            LockMode::Auto
        }
    }
}

/// `ghc-9.8.2` -> `9.8.2`.
fn compiler_version(compiler_id: &str) -> Option<&str> {
    compiler_id.strip_prefix("ghc-")
}

/// Cabal arguments that pin the build to hx.lock, or the exit code to stop with.
///
/// `ghc` is the active GHC version, used to notice a lock resolved for a
/// different compiler; `cabal` is the cabal binary the build will use, which
/// runs `cabal update` when cabal's index is older than the lock. Returns no
/// arguments when there is nothing to apply in [`LockMode::Auto`].
pub async fn plan_args(
    project: &Project,
    ghc: Option<&str>,
    cabal: Option<&Path>,
    mode: LockMode,
    output: &Output,
) -> Result<std::result::Result<Vec<String>, i32>> {
    let required = mode == LockMode::Required;
    let path = project.lockfile_path();
    if !path.exists() {
        if required {
            output.error(&format!(
                "{} not found, and --locked requires it",
                LOCKFILE_FILENAME
            ));
            output.info("fix: Run `hx lock` to create it, and commit it");
            return Ok(Err(3));
        }
        output.verbose(&format!(
            "No {}; cabal resolves the build plan",
            LOCKFILE_FILENAME
        ));
        return Ok(Ok(Vec::new()));
    }

    let lockfile = match Lockfile::from_file(&path) {
        Ok(lock) => lock,
        Err(e) => {
            if required {
                output.error(&format!("{} could not be read: {}", LOCKFILE_FILENAME, e));
                output.info("fix: Run `hx lock` to regenerate it");
                return Ok(Err(3));
            }
            output.warn(&format!(
                "{} could not be read ({}); building without it",
                LOCKFILE_FILENAME, e
            ));
            return Ok(Ok(Vec::new()));
        }
    };

    let mut problems = Vec::new();
    if required && lockfile.needs_upgrade() {
        problems.push(format!(
            "lockfile uses format v{} and has no source hashes",
            lockfile.version
        ));
    }
    // Evaluate .cabal conditionals for the compiler the plan was resolved for,
    // so the comparison matches what `hx lock` saw.
    let lock_ghc = lockfile
        .plan
        .compiler_id
        .as_deref()
        .and_then(compiler_version);
    let ctx_ghc = lock_ghc
        .or(ghc)
        .and_then(|v| v.parse::<hx_solver::Version>().ok());
    let ctx_ghc = match ctx_ghc {
        Some(v) => v,
        None => "9.8.2".parse().expect("valid default GHC version"),
    };
    problems.extend(plan_problems(
        project,
        &lockfile,
        &CabalContext::host(ctx_ghc),
    )?);
    if let (Some(locked), Some(active)) = (lock_ghc, ghc)
        && locked != active
    {
        problems.push(format!(
            "the lock was resolved for GHC {locked}, but GHC {active} is active"
        ));
    }

    if !problems.is_empty() {
        if required {
            output.error(&format!(
                "{} is out of date, and --locked requires a current one",
                LOCKFILE_FILENAME
            ));
        } else {
            output.warn(&format!(
                "{} is out of date; building without its pins",
                LOCKFILE_FILENAME
            ));
        }
        for p in &problems {
            output.info(&format!("  - {}", p));
        }
        output.info("fix: Run `hx lock` to update the lockfile, and commit the result");
        return Ok(if required { Err(3) } else { Ok(Vec::new()) });
    }

    if required {
        // As in `hx sync`: a cached tarball that does not match its pin is never built.
        if let (Some(hx_cache), Some(cabal_repo)) = (
            hx_solver::default_package_cache_dir(),
            hx_solver::cabal_repo_cache_dir(),
        ) {
            let report = sources::prepare_cabal_cache(&lockfile, &hx_cache, &cabal_repo)?;
            if !report.mismatched.is_empty() {
                for m in &report.mismatched {
                    sources::report_mismatch(output, m);
                }
                return Ok(Err(1));
            }
        }
    }

    let mut args = lockfile.cabal_args();
    if let Some(state) = lockfile.plan.index_state.as_deref()
        && !ensure_cabal_index_covers(state, cabal, output).await
    {
        if required {
            output.error(&format!(
                "cabal's Hackage index is older than the index state {} pins ({state})",
                LOCKFILE_FILENAME
            ));
            output.info("fix: Run `cabal update` (online), then build again");
            return Ok(Err(1));
        }
        output.warn(&format!(
            "cabal's Hackage index is older than the index state {} pins ({state}); building with its versions but without the index state",
            LOCKFILE_FILENAME
        ));
        output.info("fix: Run `cabal update` to bring the index up to date");
        args.retain(|a| !a.starts_with("--index-state="));
    }
    output.verbose(&format!(
        "Building with the plan pinned in {} ({} package{})",
        LOCKFILE_FILENAME,
        lockfile.packages.len(),
        if lockfile.packages.len() == 1 {
            ""
        } else {
            "s"
        }
    ));
    Ok(Ok(args))
}

/// Make sure cabal's own copy of the Hackage index reaches `index_state`.
///
/// cabal refuses to build at an index state newer than its index (Cabal-7159),
/// and `hx lock` refreshes hx's index, not cabal's. When cabal's index is older
/// (or missing), this runs `cabal update`. Returns whether cabal's index now
/// covers the state; `true` when that cannot be determined, so cabal decides.
pub(crate) async fn ensure_cabal_index_covers(
    index_state: &str,
    cabal: Option<&Path>,
    output: &Output,
) -> bool {
    let Some(needed) = sources::index_state_timestamp(index_state) else {
        return true;
    };
    let Some(repo) = hx_solver::cabal_repo_cache_dir() else {
        return true;
    };
    let tar = repo.join("01-index.tar");
    let covered = || hx_solver::tar_index_newest_time(&tar).is_some_and(|t| t >= needed);
    if covered() {
        return true;
    }
    let spinner = Spinner::new(format!(
        "Updating cabal's Hackage index to {} ({index_state})...",
        LOCKFILE_FILENAME
    ));
    let program = cabal.map(Path::as_os_str).unwrap_or(OsStr::new("cabal"));
    match CommandRunner::new()
        .run(program, [OsStr::new("update")])
        .await
    {
        Ok(out) if out.success() && covered() => {
            spinner.finish_success("Updated cabal's Hackage index");
            true
        }
        Ok(out) => {
            spinner.finish_clear();
            output.verbose(&out.stderr);
            false
        }
        Err(e) => {
            spinner.finish_clear();
            output.verbose(&format!("cabal update failed: {e}"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compiler_version_strips_prefix() {
        assert_eq!(compiler_version("ghc-9.8.2"), Some("9.8.2"));
        assert_eq!(compiler_version("bhc-0.2"), None);
    }

    #[test]
    fn test_lock_mode_from_flag() {
        assert_eq!(LockMode::from_flag(true), LockMode::Required);
        assert_eq!(LockMode::from_flag(false), LockMode::Auto);
    }
}
