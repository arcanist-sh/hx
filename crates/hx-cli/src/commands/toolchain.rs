//! Toolchain command implementation.

use crate::cli::ToolchainCommands;
use anyhow::Result;
use hx_cache::toolchain_dir;
use hx_config::{Manifest, find_project_root};
use hx_solver::bhc_platform::find_platform_for_bhc;
use hx_toolchain::{
    BhcInstallOptions, GhcSource, InstallStrategy, RECOMMENDED_BHC_VERSION,
    RECOMMENDED_CABAL_VERSION, RECOMMENDED_GHC_VERSION, SmartCabalInstallOptions,
    SmartInstallOptions, Toolchain, ToolchainManifest, create_symlinks, install, install_bhc,
    known_versions, remove_ghc, set_active,
};
use hx_ui::{Output, Style};

/// Run a toolchain subcommand.
pub async fn run(command: ToolchainCommands, output: &Output) -> Result<i32> {
    match command {
        ToolchainCommands::Status => status(output).await,
        ToolchainCommands::List {
            available,
            installed,
        } => list(available, installed, output).await,
        ToolchainCommands::Install {
            version,
            ghc,
            cabal,
            hls,
            bhc,
            set,
            force,
            ghcup,
        } => install_toolchain(version.or(ghc), cabal, hls, bhc, set, force, ghcup, output).await,
        ToolchainCommands::Remove { version, yes } => remove(&version, yes, output).await,
        ToolchainCommands::Use { version } => use_version(&version, output).await,
    }
}

async fn status(output: &Output) -> Result<i32> {
    output.status("Detecting", "toolchain");

    let toolchain = Toolchain::detect().await;

    output.header("Toolchain Status");

    // GHC - show source if hx-managed
    if let Some(version) = toolchain.ghc.status.version() {
        let source_info = match toolchain.ghc.source {
            Some(GhcSource::HxManaged) => " (hx)",
            Some(GhcSource::Ghcup) => " (ghcup)",
            Some(GhcSource::System) => " (system)",
            None => "",
        };
        output.list_item(
            "ghc",
            &format!("{} {}{}", Style::success("✓"), version, source_info),
        );
    } else if toolchain.ghc.status.is_found() {
        output.list_item("ghc", &format!("{} (version unknown)", Style::warning("?")));
    } else {
        output.list_item("ghc", &format!("{} not found", Style::error("✗")));
    }

    // Cabal
    if let Some(version) = toolchain.cabal.status.version() {
        output.list_item("cabal", &format!("{} {}", Style::success("✓"), version));
    } else if toolchain.cabal.status.is_found() {
        output.list_item(
            "cabal",
            &format!("{} (version unknown)", Style::warning("?")),
        );
    } else {
        output.list_item("cabal", &format!("{} not found", Style::error("✗")));
    }

    // GHCup
    if let Some(version) = toolchain.ghcup.status.version() {
        output.list_item("ghcup", &format!("{} {}", Style::success("✓"), version));
    } else if toolchain.ghcup.status.is_found() {
        output.list_item(
            "ghcup",
            &format!("{} (version unknown)", Style::warning("?")),
        );
    } else {
        output.list_item("ghcup", &format!("{} not found", Style::warning("○")));
    }

    // HLS
    if let Some(version) = toolchain.hls.status.version() {
        output.list_item("hls", &format!("{} {}", Style::success("✓"), version));
    } else if toolchain.hls.status.is_found() {
        output.list_item("hls", &format!("{} (version unknown)", Style::warning("?")));
    } else {
        output.list_item("hls", &format!("{} not found", Style::warning("○")));
    }

    // Show hx-managed installations
    if let Ok(tc_dir) = toolchain_dir() {
        if let Ok(manifest) = ToolchainManifest::load(&tc_dir) {
            if !manifest.ghc.is_empty() {
                output.header("hx-managed GHC versions");
                for installed in &manifest.ghc {
                    let active_marker =
                        if manifest.active_ghc.as_deref() == Some(&installed.version) {
                            " (active)"
                        } else {
                            ""
                        };
                    output.list_item(
                        &installed.version,
                        &format!("{}{}", installed.install_path.display(), active_marker),
                    );
                }
            }
        }
    }

    if toolchain.is_complete() {
        Ok(0)
    } else {
        Ok(4) // Toolchain error
    }
}

async fn list(available: bool, installed_only: bool, output: &Output) -> Result<i32> {
    let tc_dir = toolchain_dir()?;
    let manifest = ToolchainManifest::load(&tc_dir)?;
    let installed: Vec<_> = manifest.installed_versions().into_iter().collect();

    if available && !installed_only {
        // Show all known versions with installation status
        output.header("Available GHC versions");

        let versions = known_versions();
        for version in versions {
            let is_installed = installed.contains(&version.version.as_str());
            let is_active = manifest.active_ghc.as_deref() == Some(&version.version);

            let status = if is_active {
                format!("{} (active)", Style::success("✓"))
            } else if is_installed {
                format!("{} (installed)", Style::success("✓"))
            } else if version.is_recommended {
                format!("{} (recommended)", Style::info("○"))
            } else {
                Style::dim("○").to_string()
            };

            output.list_item(&version.version, &status);
        }
    } else if installed_only || installed.is_empty() {
        // Show only installed versions
        output.header("Installed GHC versions (hx-managed)");

        if manifest.ghc.is_empty() {
            output.info("No GHC versions installed via hx");
            output.info(&format!(
                "Install one with: hx toolchain install {}",
                RECOMMENDED_GHC_VERSION
            ));
        } else {
            for ghc in &manifest.ghc {
                let active_marker = if manifest.active_ghc.as_deref() == Some(&ghc.version) {
                    " *"
                } else {
                    ""
                };
                output.list_item(
                    &format!("{}{}", ghc.version, active_marker),
                    &format!(
                        "installed {} via {}",
                        ghc.installed_at.format("%Y-%m-%d"),
                        ghc.source
                    ),
                );
            }
        }
    } else {
        // Default: show installed with some available
        output.header("Installed GHC versions");

        if manifest.ghc.is_empty() {
            output.info("No GHC versions installed via hx");
        } else {
            for ghc in &manifest.ghc {
                let active_marker = if manifest.active_ghc.as_deref() == Some(&ghc.version) {
                    format!(" {}", Style::success("(active)"))
                } else {
                    String::new()
                };
                output.list_item(&ghc.version, &active_marker);
            }
        }

        output.info("");
        output.info("Use --available to see all known versions");
    }

    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn install_toolchain(
    ghc_version: Option<String>,
    cabal: Option<String>,
    hls: Option<String>,
    bhc: Option<String>,
    set_as_active: bool,
    force: bool,
    use_ghcup: bool,
    output: &Output,
) -> Result<i32> {
    let mut success = true;

    // Install GHC
    if let Some(ref version) = ghc_version {
        output.status("Installing", &format!("GHC {}", version));

        let strategy = if use_ghcup {
            InstallStrategy::Ghcup
        } else {
            InstallStrategy::Smart
        };

        let options = SmartInstallOptions::new(version)
            .with_strategy(strategy)
            .with_set_active(set_as_active)
            .with_force(force);

        if let Err(e) = install::install_ghc_smart(&options).await {
            output.print_error(&e);
            success = false;
        } else {
            output.status("Done", &format!("GHC {} installed", version));

            // Create symlinks if set as active
            if set_as_active {
                if let Ok(tc_dir) = toolchain_dir() {
                    if let Ok(Some(installed)) = hx_toolchain::get_active(&tc_dir) {
                        let _ = create_symlinks(&installed);
                    }
                }
            }
        }
    }

    // Install Cabal: direct download, falling back to ghcup
    if let Some(ref version) = cabal {
        output.status("Installing", &format!("Cabal {}", version));

        let strategy = if use_ghcup {
            InstallStrategy::Ghcup
        } else {
            InstallStrategy::Smart
        };

        let options = SmartCabalInstallOptions::new(version)
            .with_strategy(strategy)
            .with_set_active(set_as_active)
            .with_force(force);

        if let Err(e) = install::install_cabal_smart(&options).await {
            output.print_error(&e);
            success = false;
        } else {
            output.status("Done", &format!("Cabal {} installed", version));
        }
    }

    // Install HLS via ghcup (if requested)
    if let Some(ref version) = hls {
        let toolchain = Toolchain::detect().await;
        if !toolchain.has_ghcup() {
            output.error("ghcup is required to install HLS");
            output.info(&format!(
                "Install ghcup: {}",
                install::ghcup_install_command()
            ));
            success = false;
        } else {
            output.status("Installing", &format!("HLS {}", version));
            if let Err(e) = install::install_hls(version).await {
                output.print_error(&e);
                success = false;
            }
        }
    }

    // Install BHC (if requested)
    if let Some(ref version) = bhc {
        // Handle "latest" as an alias for the recommended version
        let version = if version == "latest" {
            RECOMMENDED_BHC_VERSION.to_string()
        } else {
            version.clone()
        };

        output.status("Installing", &format!("BHC {}", version));

        let options = BhcInstallOptions::new(&version)
            .with_set_active(set_as_active)
            .with_force(force);

        match install_bhc(&options).await {
            Ok(result) => {
                if result.was_cached {
                    output.status("Done", &format!("BHC {} already installed", version));
                } else {
                    output.status("Done", &format!("BHC {} installed", version));
                }

                // Auto-install GHC + Cabal when only --bhc was specified
                if ghc_version.is_none() && cabal.is_none() {
                    if let Some(platform) = find_platform_for_bhc(&version) {
                        let ghc_ver = platform.ghc_compat.to_string();
                        output.info(&format!(
                            "Also installing GHC {} (required by BHC Platform {})",
                            ghc_ver, platform.id
                        ));

                        let ghc_options = SmartInstallOptions::new(&ghc_ver)
                            .with_strategy(if use_ghcup {
                                InstallStrategy::Ghcup
                            } else {
                                InstallStrategy::Smart
                            })
                            .with_set_active(set_as_active)
                            .with_force(force);

                        if let Err(e) = install::install_ghc_smart(&ghc_options).await {
                            output.print_error(&e);
                            success = false;
                        } else {
                            output.status("Done", &format!("GHC {} installed", ghc_ver));
                            if set_as_active {
                                if let Ok(tc_dir) = toolchain_dir() {
                                    if let Ok(Some(installed)) = hx_toolchain::get_active(&tc_dir) {
                                        let _ = create_symlinks(&installed);
                                    }
                                }
                            }
                        }

                        let cabal_ver = RECOMMENDED_CABAL_VERSION;
                        output.info(&format!(
                            "Also installing Cabal {} (recommended for BHC Platform)",
                            cabal_ver
                        ));
                        if let Err(e) = install::install_cabal(cabal_ver).await {
                            output.print_error(&e);
                            success = false;
                        } else {
                            output.status("Done", &format!("Cabal {} installed", cabal_ver));
                        }
                    }
                }
            }
            Err(e) => {
                output.error(&format!("Failed to install BHC {}: {}", version, e));
                success = false;
            }
        }
    }

    // If no specific versions requested, show help
    if ghc_version.is_none() && cabal.is_none() && hls.is_none() && bhc.is_none() {
        output.warn("No version specified");
        output.info(&format!(
            "Example: hx toolchain install {}",
            RECOMMENDED_GHC_VERSION
        ));
        output.info("Or: hx toolchain install --ghc 9.8.2");
        output.info("Or: hx toolchain install --bhc latest");
        return Ok(2);
    }

    if success { Ok(0) } else { Ok(4) }
}

async fn remove(version: &str, yes: bool, output: &Output) -> Result<i32> {
    let tc_dir = toolchain_dir()?;

    // Check if version exists
    let manifest = ToolchainManifest::load(&tc_dir)?;
    if !manifest.is_installed(version) {
        output.error(&format!("GHC {} is not installed via hx", version));
        return Ok(4);
    }

    // Confirm unless --yes
    if !yes {
        output.warn(&format!(
            "This will remove GHC {} and all associated files",
            version
        ));
        // In a real implementation, you'd prompt for confirmation here
        // For now, we'll proceed
    }

    output.status("Removing", &format!("GHC {}", version));

    if remove_ghc(&tc_dir, version)? {
        output.status("Done", &format!("GHC {} removed", version));
        Ok(0)
    } else {
        output.error(&format!("Failed to remove GHC {}", version));
        Ok(4)
    }
}

async fn use_version(version: &str, output: &Output) -> Result<i32> {
    // Check for "project" keyword
    if version == "project" {
        return use_project_toolchain(output).await;
    }

    let tc_dir = toolchain_dir()?;

    // First, try hx-managed installations
    let manifest = ToolchainManifest::load(&tc_dir)?;
    if manifest.is_installed(version) {
        set_active(&tc_dir, version)?;

        // Update symlinks
        if let Ok(Some(installed)) = hx_toolchain::get_active(&tc_dir) {
            let _ = create_symlinks(&installed);
        }

        output.status("Done", &format!("Now using GHC {} (hx-managed)", version));
        return Ok(0);
    }

    // Fall back to ghcup
    let toolchain = Toolchain::detect().await;
    if !toolchain.has_ghcup() {
        output.error(&format!("GHC {} is not installed", version));
        output.info(&format!(
            "Install it with: hx toolchain install {}",
            version
        ));
        return Ok(4);
    }

    output.status("Switching", &format!("GHC to {} (via ghcup)", version));
    if let Err(e) = install::set_ghc(version).await {
        output.print_error(&e);
        return Ok(4);
    }

    output.status("Done", &format!("Now using GHC {}", version));
    Ok(0)
}

async fn use_project_toolchain(output: &Output) -> Result<i32> {
    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(_) => {
            output.error("Not in an hx project (no hx.toml found)");
            output.info("Run `hx init` to create a new project, or specify a version directly:");
            output.info("  hx toolchain use 9.8.2");
            return Ok(3);
        }
    };

    // Load manifest
    let manifest_path = project_root.join("hx.toml");
    let manifest = match Manifest::from_file(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            output.error(&format!("Failed to read hx.toml: {}", e));
            return Ok(3);
        }
    };

    let mut success = true;

    // Set GHC if specified
    if let Some(ref ghc_version) = manifest.toolchain.ghc {
        let tc_dir = toolchain_dir()?;
        let tc_manifest = ToolchainManifest::load(&tc_dir)?;

        if tc_manifest.is_installed(ghc_version) {
            // Use hx-managed version
            set_active(&tc_dir, ghc_version)?;
            if let Ok(Some(installed)) = hx_toolchain::get_active(&tc_dir) {
                let _ = create_symlinks(&installed);
            }
            output.list_item("ghc", &format!("{} (hx)", ghc_version));
        } else {
            // Fall back to ghcup
            output.status("Switching", &format!("GHC to {}", ghc_version));
            if let Err(e) = install::set_ghc(ghc_version).await {
                output.print_error(&e);
                success = false;
            } else {
                output.list_item("ghc", ghc_version);
            }
        }
    }

    // Set Cabal if specified (ghcup only)
    if let Some(ref cabal_version) = manifest.toolchain.cabal {
        output.status("Switching", &format!("Cabal to {}", cabal_version));
        if let Err(e) = install::set_cabal(cabal_version).await {
            output.print_error(&e);
            success = false;
        } else {
            output.list_item("cabal", cabal_version);
        }
    }

    // Set HLS if specified (ghcup only)
    if let Some(ref hls_version) = manifest.toolchain.hls {
        output.status("Switching", &format!("HLS to {}", hls_version));
        if let Err(e) = install::set_hls(hls_version).await {
            output.print_error(&e);
            success = false;
        } else {
            output.list_item("hls", hls_version);
        }
    }

    // Check if any toolchain was specified
    if manifest.toolchain.ghc.is_none()
        && manifest.toolchain.cabal.is_none()
        && manifest.toolchain.hls.is_none()
    {
        output.warn("No toolchain versions specified in hx.toml");
        output.info("Add a [toolchain] section to your hx.toml:");
        output.info("  [toolchain]");
        output.info("  ghc = \"9.8.2\"");
        output.info("  cabal = \"3.12.1.0\"");
        return Ok(0);
    }

    if success {
        output.status("Done", "Toolchain configured from project");
        Ok(0)
    } else {
        Ok(4)
    }
}
