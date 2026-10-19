//! `cabal install` of executables into a staging directory (`hx install`).
//!
//! hx owns the final layout (`~/.hx/bin` plus a manifest), so this module
//! only asks cabal to build the requested package and *copy* its executables
//! into a directory hx chose. Moving them into place, collision checks and
//! bookkeeping happen in the caller.

use crate::build::{build_progress_message, format_duration, parse_build_output};
use hx_cache::{cabal_store_dir, ensure_dir};
use hx_core::{CommandOutput, CommandRunner, Error, Fix, Result};
use hx_ui::{Output, Spinner};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info};

/// Options for installing a package's executables with cabal.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// The cabal executable to run.
    pub cabal: PathBuf,
    /// Directory cabal runs in. For a Hackage package this should be an empty
    /// directory, so a surrounding `cabal.project` cannot leak constraints in.
    pub working_dir: PathBuf,
    /// Install target, e.g. `hlint-3.8`. `None` installs the package in
    /// `working_dir` (a local project).
    pub target: Option<String>,
    /// Where cabal copies the built executables.
    pub install_dir: PathBuf,
    /// Compiler to build with (`--with-compiler`).
    pub ghc: Option<PathBuf>,
    /// Number of parallel jobs.
    pub jobs: Option<usize>,
    /// Stream the underlying tool output.
    pub verbose: bool,
    /// Additional bin directories to prepend to PATH (hx-managed toolchains).
    pub toolchain_bin_dirs: Vec<PathBuf>,
}

/// Build the argument list for `cabal install`.
///
/// `--install-method=copy` produces standalone files rather than symlinks
/// into the store, so the result can be moved; `--overwrite-policy=always`
/// lets a retry reuse the same staging directory.
pub fn install_args(options: &InstallOptions, store_dir: &Path) -> Vec<String> {
    // Global options must come before the subcommand.
    let mut args = vec![
        format!("--store-dir={}", store_dir.display()),
        "install".to_string(),
        format!("--installdir={}", options.install_dir.display()),
        "--install-method=copy".to_string(),
        "--overwrite-policy=always".to_string(),
    ];

    if let Some(ref ghc) = options.ghc {
        args.push(format!("--with-compiler={}", ghc.display()));
    }

    if let Some(jobs) = options.jobs {
        args.push(format!("-j{}", jobs));
    }

    if let Some(ref target) = options.target {
        args.push(target.clone());
    }

    args
}

/// Run `cabal install`, copying the package's executables into
/// `options.install_dir`.
///
/// If cabal has never downloaded a package index (a fresh hx-managed cabal),
/// runs `cabal update` once and retries, instead of failing with an
/// "unknown package" error the user cannot act on.
pub async fn install(options: &InstallOptions, output: &Output) -> Result<Duration> {
    let store_dir = cabal_store_dir()?;
    ensure_dir(&store_dir)?;
    ensure_dir(&options.install_dir)?;

    let args = install_args(options, &store_dir);
    info!("Running cabal install in {}", options.working_dir.display());
    debug!("Args: {:?}", args);

    let mut runner = CommandRunner::new().with_working_dir(&options.working_dir);
    for bin_dir in &options.toolchain_bin_dirs {
        runner = runner.with_ghc_bin(bin_dir);
    }

    let mut cmd_output = run_install(&runner, options, &args, output).await?;

    if !cmd_output.success() && needs_index_update(&cmd_output) {
        update_index(&runner, options, output).await?;
        cmd_output = run_install(&runner, options, &args, output).await?;
    }

    let result = parse_build_output(&cmd_output);
    if !result.success {
        // In compact mode only progress was streamed, not the diagnostics.
        if !options.verbose && !cmd_output.stderr.is_empty() {
            eprintln!("{}", cmd_output.stderr);
        }
        return Err(Error::BuildFailed {
            errors: result.errors,
            fixes: vec![],
        });
    }

    Ok(result.duration)
}

async fn run_install(
    runner: &CommandRunner,
    options: &InstallOptions,
    args: &[String],
    output: &Output,
) -> Result<CommandOutput> {
    let label = options.target.as_deref().unwrap_or("local package");
    let spinner = (!options.verbose).then(|| Spinner::new(format!("Building {label}...")));

    // Stream cabal's output live, as `hx build` does: progress in the
    // spinner in compact mode, the raw tool output in verbose mode.
    let cmd_output = runner
        .run_streaming(
            options.cabal.as_os_str(),
            args.iter().map(OsStr::new),
            |_, line| match &spinner {
                Some(spinner) => {
                    if let Some(msg) = build_progress_message(line) {
                        spinner.set_message(msg);
                    }
                }
                None => output.verbose(line),
            },
        )
        .await?;

    if let Some(spinner) = spinner {
        if cmd_output.success() {
            spinner.finish_success(format!(
                "Built {label} in {}",
                format_duration(cmd_output.duration)
            ));
        } else if needs_index_update(&cmd_output) {
            spinner.finish_clear();
        } else {
            spinner.finish_error(format!("Failed to build {label}"));
        }
    }

    Ok(cmd_output)
}

async fn update_index(
    runner: &CommandRunner,
    options: &InstallOptions,
    output: &Output,
) -> Result<()> {
    output.info("No Hackage package index yet; running `cabal update` first");
    let spinner = Spinner::new("Updating package index...");
    let update = runner
        .run(options.cabal.as_os_str(), [OsStr::new("update")])
        .await?;
    if !update.success() {
        spinner.finish_error("Failed to update package index");
        return Err(Error::CommandFailed {
            command: "cabal update".to_string(),
            exit_code: Some(update.exit_code),
            stdout: update.stdout,
            stderr: update.stderr,
            fixes: vec![Fix::with_command(
                "Check your network connection, then refresh the index",
                "cabal update",
            )],
        });
    }
    spinner.finish_success("Package index updated");
    Ok(())
}

/// Whether a failed `cabal install` failed only because cabal has no package
/// index yet.
fn needs_index_update(output: &CommandOutput) -> bool {
    // cabal wraps its messages to the terminal width, so "cabal update" may be
    // split across lines; compare with whitespace normalised.
    let text = format!("{} {}", output.stdout, output.stderr)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    text.contains("cabal update")
        && (text.contains("package list") || text.contains("does not exist"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> InstallOptions {
        InstallOptions {
            cabal: PathBuf::from("cabal"),
            working_dir: PathBuf::from("/work"),
            target: Some("hlint-3.8".to_string()),
            install_dir: PathBuf::from("/stage"),
            ghc: None,
            jobs: None,
            verbose: false,
            toolchain_bin_dirs: vec![],
        }
    }

    fn cmd_output(exit_code: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn install_args_copy_into_install_dir() {
        let args = install_args(&options(), Path::new("/store"));
        assert_eq!(
            args,
            vec![
                "--store-dir=/store",
                "install",
                "--installdir=/stage",
                "--install-method=copy",
                "--overwrite-policy=always",
                "hlint-3.8",
            ]
        );
    }

    #[test]
    fn install_args_pass_compiler_and_jobs_before_target() {
        let mut opts = options();
        opts.ghc = Some(PathBuf::from("/ghc/bin/ghc"));
        opts.jobs = Some(4);
        let args = install_args(&opts, Path::new("/store"));
        let tail: Vec<_> = args.iter().rev().take(3).rev().cloned().collect();
        assert_eq!(
            tail,
            vec!["--with-compiler=/ghc/bin/ghc", "-j4", "hlint-3.8"]
        );
    }

    #[test]
    fn install_args_without_target_install_local_package() {
        let mut opts = options();
        opts.target = None;
        let args = install_args(&opts, Path::new("/store"));
        assert_eq!(args.last().unwrap(), "--overwrite-policy=always");
    }

    #[test]
    fn detects_missing_package_index() {
        let out = cmd_output(
            1,
            "",
            "Warning: The package list for 'hackage.haskell.org' does not exist. \
             Run 'cabal update' to download it.\nError: unknown package: hello",
        );
        assert!(needs_index_update(&out));
    }

    #[test]
    fn detects_missing_package_index_when_wrapped() {
        // Verbatim from cabal-install 3.16 on a fresh HOME.
        let out = cmd_output(
            1,
            "",
            "Warning: The package list for 'hackage.haskell.org' does not exist. Run 'cabal\n\
             update' to download it.\nError: [Cabal-7144]\nUnknown package \"hello\".",
        );
        assert!(needs_index_update(&out));
    }

    #[test]
    fn ordinary_build_failure_is_not_an_index_problem() {
        let out = cmd_output(1, "", "src/Main.hs:3:1: error: parse error");
        assert!(!needs_index_update(&out));
    }
}
