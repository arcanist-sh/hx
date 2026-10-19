//! Cabal freeze operations for lockfile generation.

use hx_cache::cabal_store_dir;
use hx_core::{CommandRunner, Error, Result};
use hx_ui::Spinner;
use std::path::Path;
use tracing::{debug, info};

/// Run cabal update to refresh the package index.
pub async fn update(verbose: bool) -> Result<()> {
    let spinner = if !verbose {
        Some(Spinner::new("Updating package index..."))
    } else {
        None
    };

    info!("Running cabal update");

    let runner = CommandRunner::new();
    let output = runner.run("cabal", ["update"]).await?;

    if let Some(spinner) = spinner {
        if output.success() {
            spinner.finish_success("Package index updated");
        } else {
            spinner.finish_error("Failed to update package index");
        }
    }

    if !output.success() {
        return Err(Error::CommandFailed {
            command: "cabal update".to_string(),
            exit_code: Some(output.exit_code),
            stdout: output.stdout,
            stderr: output.stderr,
            fixes: vec![],
        });
    }

    Ok(())
}

/// Run cabal freeze to generate constraints.
pub async fn freeze(project_root: &Path, build_dir: &Path) -> Result<String> {
    let store_dir = cabal_store_dir()?;

    let build_dir_str = build_dir.to_string_lossy().to_string();
    let store_dir_str = store_dir.to_string_lossy().to_string();

    // `--store-dir` is a global option: it must precede the subcommand, or
    // cabal rejects it as an unknown `freeze` option.
    let args = vec![
        "--store-dir",
        &store_dir_str,
        "freeze",
        "--builddir",
        &build_dir_str,
    ];

    info!("Running cabal freeze in {}", project_root.display());
    debug!("Args: {:?}", args);

    let spinner = Spinner::new("Resolving dependencies...");

    let runner = CommandRunner::new().with_working_dir(project_root);
    let output = runner.run("cabal", args.iter().copied()).await?;

    if output.success() {
        spinner.finish_success("Dependencies resolved");
    } else {
        spinner.finish_error("Failed to resolve dependencies");
        return Err(Error::CommandFailed {
            command: "cabal freeze".to_string(),
            exit_code: Some(output.exit_code),
            stdout: output.stdout,
            stderr: output.stderr,
            fixes: vec![],
        });
    }

    // Read the generated cabal.project.freeze file
    let freeze_file = project_root.join("cabal.project.freeze");
    if freeze_file.exists() {
        std::fs::read_to_string(&freeze_file).map_err(|e| Error::Io {
            message: "failed to read freeze file".to_string(),
            path: Some(freeze_file),
            source: e,
        })
    } else {
        // If no freeze file, return empty string
        Ok(String::new())
    }
}

/// Get the build plan dry-run output.
pub async fn dry_run(project_root: &Path, build_dir: &Path) -> Result<String> {
    let store_dir = cabal_store_dir()?;

    let build_dir_str = build_dir.to_string_lossy().to_string();
    let store_dir_str = store_dir.to_string_lossy().to_string();

    // Global options must come before the subcommand
    let args = [
        "--store-dir",
        &store_dir_str as &str,
        "build",
        "--dry-run",
        "--builddir",
        &build_dir_str as &str,
    ];

    info!("Running cabal build --dry-run");

    let runner = CommandRunner::new().with_working_dir(project_root);
    let output = runner.run("cabal", args.iter().copied()).await?;

    Ok(format!("{}\n{}", output.stdout, output.stderr))
}
