//! BHC REPL (interactive mode) support.

use crate::native::{BhcCompilerConfig, BhcNativeError};
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, info};

/// Build argument list for a BHC REPL session.
///
/// Produces an invocation of the `bhc` CLI's `repl` subcommand using BHC's flag
/// spellings: `--profile` and `--import-path` (top-level, before the
/// subcommand) and `--package-db` (global). The `repl` token comes last. This is
/// extracted as a helper for testability.
pub fn build_repl_args(bhc: &BhcCompilerConfig, src_dirs: &[std::path::PathBuf]) -> Vec<String> {
    let mut args = vec![format!("--profile={}", bhc.profile.as_str())];

    // Import paths for resolving `:load` targets (top-level flag).
    for dir in src_dirs {
        args.push("--import-path".to_string());
        args.push(dir.display().to_string());
    }

    // Package databases of compiled `.bhi` interfaces (global flag).
    for db in &bhc.package_dbs {
        args.push("--package-db".to_string());
        args.push(db.display().to_string());
    }

    // The REPL subcommand goes last, after the top-level flags.
    args.push("repl".to_string());

    args
}

/// Start a BHC interactive REPL session.
///
/// This launches `bhc repl` with the project's package databases and source
/// directories configured.
pub async fn start_bhc_repl(
    project_root: &Path,
    bhc: &BhcCompilerConfig,
    src_dirs: &[std::path::PathBuf],
) -> Result<i32, BhcNativeError> {
    info!("Starting BHC REPL in {}", project_root.display());

    let args = build_repl_args(bhc, src_dirs);

    debug!("BHC REPL args: {:?}", args);

    let status = Command::new(&bhc.bhc_path)
        .args(&args)
        .current_dir(project_root)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| BhcNativeError::Io {
            message: "failed to start BHC REPL".to_string(),
            source: e,
        })?;

    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::BhcCompilerConfig;
    use hx_config::BhcProfile;
    use std::path::PathBuf;

    #[test]
    fn test_repl_args_construction() {
        let bhc = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::Numeric,
            package_dbs: vec![
                PathBuf::from("/cache/hx/bhc-2026.2.0/package.db"),
                PathBuf::from("/home/user/.bhc/package.db"),
            ],
            packages: Vec::new(),
            tensor_fusion: true,
            emit_kernel_report: false,
        };

        let src_dirs = vec![PathBuf::from("src"), PathBuf::from("lib")];

        let args = build_repl_args(&bhc, &src_dirs);

        // Launches the `repl` subcommand using BHC's flag spellings.
        assert_eq!(args.last(), Some(&"repl".to_string()));
        assert!(!args.contains(&"--interactive".to_string()));
        assert!(args.contains(&"--package-db".to_string()));
        assert!(args.contains(&"/cache/hx/bhc-2026.2.0/package.db".to_string()));
        assert!(args.contains(&"/home/user/.bhc/package.db".to_string()));
        assert!(args.contains(&"--import-path".to_string()));
        assert!(args.contains(&"src".to_string()));
        assert!(args.contains(&"lib".to_string()));
        assert!(args.contains(&"--profile=numeric".to_string()));
        // The old GHC-style single-dash spellings must be gone.
        assert!(!args.iter().any(|a| a.starts_with("-package-db=")));
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("-i") && a != "--import-path")
        );
    }

    #[test]
    fn test_repl_args_minimal() {
        let bhc = BhcCompilerConfig {
            bhc_path: PathBuf::from("bhc"),
            version: "2026.2.0".to_string(),
            profile: BhcProfile::default(),
            package_dbs: Vec::new(),
            packages: Vec::new(),
            tensor_fusion: false,
            emit_kernel_report: false,
        };

        let args = build_repl_args(&bhc, &[]);

        assert_eq!(args.last(), Some(&"repl".to_string()));
        assert!(args.contains(&"--profile=default".to_string()));
        // `--tensor-fusion` is not a `bhc` flag and must not be emitted.
        assert!(!args.contains(&"--tensor-fusion".to_string()));
        assert!(!args.contains(&"--interactive".to_string()));
    }
}
