//! Run command implementation.

use anyhow::Result;
use hx_cabal::build as cabal_build;
use hx_config::{CompilerBackend, Project, find_project_root};
use hx_plugins::HookEvent;
use hx_toolchain::{AutoInstallPolicy, Toolchain, ensure_toolchain};
use hx_ui::Output;

use crate::plugins::PluginHooks;

/// Run the project.
pub async fn run(
    args: Vec<String>,
    package: Option<String>,
    target: Option<String>,
    native: bool,
    backend_override: Option<CompilerBackend>,
    policy: AutoInstallPolicy,
    output: &Output,
) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Determine compiler backend (CLI override > config > default)
    let backend = backend_override.unwrap_or(project.manifest.compiler.backend);

    // Use BHC backend for running
    if backend == CompilerBackend::Bhc {
        if native {
            return run_bhc_native_run(&project, args, target, output).await;
        }
        return run_bhc_run(&project, args, package, target, output).await;
    }

    // Check toolchain requirements
    let mut toolchain = Toolchain::detect().await;
    if let Err(e) = ensure_toolchain(
        &toolchain,
        project.manifest.toolchain.ghc.as_deref(),
        project.manifest.toolchain.cabal.as_deref(),
        policy,
    )
    .await
    {
        output.print_error(&e);
        return Ok(4); // Toolchain error exit code
    }

    // Re-detect toolchain after potential installation to get updated paths
    if !toolchain.ghc.status.is_found() || !toolchain.cabal.status.is_found() {
        toolchain = Toolchain::detect().await;
    }

    // Validate package selection for workspaces
    if let Some(ref pkg_name) = package {
        if project.is_workspace() {
            if project.get_package(pkg_name).is_none() {
                output.error(&format!("Package '{}' not found in workspace", pkg_name));
                output.info(&format!(
                    "Available packages: {}",
                    project.package_names().join(", ")
                ));
                return Ok(1);
            }
        } else {
            output.warn("--package flag ignored (not a workspace project)");
        }
    }

    let run_target = match &package {
        Some(pkg) if project.is_workspace() => pkg.as_str(),
        _ => project.name(),
    };

    // Get GHC version for plugin context
    let ghc_version = toolchain.ghc.status.version().map(|v| v.to_string());

    // Initialize plugin hooks
    let mut hooks = PluginHooks::from_project(&project, ghc_version);
    if let Some(ref mut h) = hooks {
        if let Err(e) = h.initialize() {
            output.verbose(&format!("Plugin initialization warning: {}", e));
        }
    }

    // Run pre-run hooks
    if let Some(ref mut h) = hooks {
        if !h.run_pre_hook(HookEvent::PreRun, output) {
            output.error("Pre-run hook failed");
            return Ok(6); // Hook failure exit code
        }
    }

    output.status("Running", run_target);

    // Collect toolchain bin directories for PATH
    let mut toolchain_bin_dirs = Vec::new();
    if let Some(ghc_path) = toolchain.ghc.status.path() {
        if let Some(parent) = ghc_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }
    if let Some(cabal_path) = toolchain.cabal.status.path() {
        if let Some(parent) = cabal_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }

    let build_dir = project.cabal_build_dir();

    let exit_code = cabal_build::run(
        &project.root,
        &build_dir,
        &args,
        package.as_deref(),
        target.as_deref(),
        &toolchain_bin_dirs,
        output,
    )
    .await?;

    // Run post-run hooks
    if let Some(ref mut h) = hooks {
        h.run_post_hook(HookEvent::PostRun, output);
    }

    Ok(exit_code)
}

/// Start a REPL.
pub async fn repl(
    backend_override: Option<CompilerBackend>,
    policy: AutoInstallPolicy,
    output: &Output,
) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    let backend = backend_override.unwrap_or(project.manifest.compiler.backend);

    if backend == CompilerBackend::Bhc {
        return run_bhc_repl(&project, output).await;
    }

    // Check toolchain requirements
    let mut toolchain = Toolchain::detect().await;
    if let Err(e) = ensure_toolchain(
        &toolchain,
        project.manifest.toolchain.ghc.as_deref(),
        project.manifest.toolchain.cabal.as_deref(),
        policy,
    )
    .await
    {
        output.print_error(&e);
        return Ok(4); // Toolchain error exit code
    }

    // Re-detect toolchain after potential installation to get updated paths
    if !toolchain.ghc.status.is_found() || !toolchain.cabal.status.is_found() {
        toolchain = Toolchain::detect().await;
    }

    output.status("Starting", "REPL");

    // Collect toolchain bin directories for PATH
    let mut toolchain_bin_dirs = Vec::new();
    if let Some(ghc_path) = toolchain.ghc.status.path() {
        if let Some(parent) = ghc_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }
    if let Some(cabal_path) = toolchain.cabal.status.path() {
        if let Some(parent) = cabal_path.parent() {
            toolchain_bin_dirs.push(parent.to_path_buf());
        }
    }

    let build_dir = project.cabal_build_dir();

    let exit_code = cabal_build::repl(&project.root, &build_dir, &toolchain_bin_dirs).await?;
    Ok(exit_code)
}

/// Run a project using the BHC backend.
async fn run_bhc_run(
    project: &Project,
    args: Vec<String>,
    package: Option<String>,
    target: Option<String>,
    output: &Output,
) -> Result<i32> {
    use hx_bhc::{BhcBackend, generate_bhc_manifest};
    use hx_compiler::{CompilerBackend as Backend, RunOptions as CompilerRunOptions};

    output.status("Running", &format!("{} (BHC)", project.name()));

    // Generate bhc.toml from hx.toml
    match generate_bhc_manifest(&project.root, &project.manifest) {
        Ok(path) => {
            output.verbose(&format!("Generated BHC manifest at {}", path.display()));
        }
        Err(e) => {
            output.error(&format!("Failed to generate BHC manifest: {}", e));
            return Ok(3);
        }
    }

    // Create BHC backend with project configuration
    let bhc_config = &project.manifest.compiler.bhc;
    let backend = BhcBackend::new().with_config(bhc_config.clone());

    // Check if BHC is available
    let status = backend.detect().await;
    match status {
        Ok(hx_compiler::CompilerStatus::Available { version, .. }) => {
            output.verbose(&format!("BHC version: {}", version));
        }
        Ok(hx_compiler::CompilerStatus::NotInstalled) => {
            output.error("BHC is not installed");
            output.info("Install BHC with: hx toolchain install --bhc latest");
            return Ok(4);
        }
        Ok(hx_compiler::CompilerStatus::VersionMismatch {
            required,
            installed,
        }) => {
            output.warn(&format!(
                "BHC version mismatch: required {}, installed {}",
                required, installed
            ));
        }
        Err(e) => {
            output.error(&format!("Failed to detect BHC: {}", e));
            return Ok(4);
        }
    }

    // Run options
    let run_options = CompilerRunOptions {
        args,
        package,
        target,
        verbose: output.is_verbose(),
    };

    match backend.run(&project.root, &run_options, output).await {
        Ok(result) => Ok(result.exit_code),
        Err(e) => {
            output.error(&format!("BHC run failed: {}", e));
            Ok(5)
        }
    }
}

/// Run a project using a native BHC build (hx owns the build graph).
///
/// Builds the project (and its dependencies) to a native executable, then
/// executes that binary with the program arguments. This is how dependency code
/// runs under BHC: the interpreter path (`run_bhc_run`) only has interface
/// information for imported packages, not their compiled bodies.
async fn run_bhc_native_run(
    project: &Project,
    args: Vec<String>,
    target: Option<String>,
    output: &Output,
) -> Result<i32> {
    // Build the native executable (deps + project) via the build command.
    let code =
        crate::commands::build::run_bhc_native_build(project, false, None, target, output).await?;
    if code != 0 {
        return Ok(code);
    }

    let exe = project
        .root
        .join(".hx/bhc-native-build")
        .join(project.name());
    if !exe.exists() {
        output.error(&format!(
            "native build did not produce an executable at {}",
            exe.display()
        ));
        return Ok(5);
    }

    output.status("Running", &format!("{} (BHC native)", project.name()));
    let status = std::process::Command::new(&exe)
        .args(&args)
        .current_dir(&project.root)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run {}: {e}", exe.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Start a BHC REPL session.
async fn run_bhc_repl(project: &Project, output: &Output) -> Result<i32> {
    use hx_bhc::native::BhcCompilerConfig;
    use hx_bhc::repl::start_bhc_repl;

    output.status("Starting", "BHC REPL");

    let bhc = match BhcCompilerConfig::detect().await {
        Ok(config) => config
            .with_profile(project.manifest.compiler.bhc.profile)
            .with_tensor_fusion(project.manifest.compiler.bhc.tensor_fusion),
        Err(e) => {
            output.error(&format!("BHC not available: {}", e));
            output.info("Install BHC with: hx toolchain install --bhc latest");
            return Ok(4);
        }
    };

    let src_dirs: Vec<std::path::PathBuf> = project
        .manifest
        .build
        .src_dirs
        .iter()
        .map(std::path::PathBuf::from)
        .collect();

    match start_bhc_repl(&project.root, &bhc, &src_dirs).await {
        Ok(code) => Ok(code),
        Err(e) => {
            output.error(&format!("BHC REPL failed: {}", e));
            Ok(1)
        }
    }
}
