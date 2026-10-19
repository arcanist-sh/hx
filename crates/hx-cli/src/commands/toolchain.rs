//! Toolchain command implementation.

use crate::cli::ToolchainCommands;
use anyhow::Result;
use hx_cache::toolchain_dir;
use hx_config::{Manifest, find_project_root};
use hx_solver::bhc_platform::find_platform_for_bhc;
use hx_toolchain::metadata::{self, Tool, VersionListing};
use hx_toolchain::{
    BhcInstallOptions, GhcSource, InstallStrategy, RECOMMENDED_BHC_VERSION,
    RECOMMENDED_CABAL_VERSION, RECOMMENDED_GHC_VERSION, SmartCabalInstallOptions,
    SmartInstallOptions, Toolchain, ToolchainManifest, create_symlinks, install, install_bhc,
    remove_ghc, set_active,
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
    let mode = if installed_only {
        ListMode::Installed
    } else if available {
        ListMode::Available
    } else {
        ListMode::Default
    };

    // Installed versions need no metadata; everything else is read from
    // ghcup's metadata, or hx's built-in tables when it is unavailable.
    let metadata = match mode {
        ListMode::Installed => None,
        _ => metadata::load().await,
    };

    let ghc_installed: Vec<&str> = manifest.ghc.iter().map(|g| g.version.as_str()).collect();
    let cabal_installed: Vec<&str> = manifest.cabal.iter().map(|c| c.version.as_str()).collect();
    let sections = [
        (
            Tool::Ghc,
            "GHC",
            ghc_installed,
            manifest.active_ghc.as_deref(),
        ),
        (
            Tool::Cabal,
            "Cabal",
            cabal_installed,
            manifest.active_cabal.as_deref(),
        ),
    ];

    for (tool, name, installed, active) in sections {
        let listing = metadata::version_listing(metadata.as_deref(), tool);
        let rows = version_rows(&listing, &installed, active, mode);
        let header = match mode {
            ListMode::Available => format!("Available {} versions", name),
            ListMode::Installed => format!("Installed {} versions (hx-managed)", name),
            ListMode::Default => format!("{} versions", name),
        };
        output.header(&header);

        if rows.is_empty() {
            output.info(&format!("No {} versions installed via hx", name));
            if let Some(recommended) = metadata::recommended_version(metadata.as_deref(), tool) {
                let command = match tool {
                    Tool::Cabal => format!("hx toolchain install --cabal {}", recommended),
                    _ => format!("hx toolchain install {}", recommended),
                };
                output.info(&format!("Install one with: {}", command));
            }
            continue;
        }

        for row in rows {
            output.list_item(&row.version, &row.describe());
        }
    }

    if mode != ListMode::Installed {
        output.info("");
        if metadata.is_some() {
            output.info("Versions and tags from ghcup metadata");
        } else {
            output.info("ghcup metadata unavailable; showing hx's built-in version list");
        }
        if mode == ListMode::Default {
            output.info("Use --available to see all versions");
        }
    }

    Ok(0)
}

/// Which versions `hx toolchain list` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListMode {
    /// Installed versions plus the recommended and latest releases.
    Default,
    /// Every current release (ghcup's `old` and prereleases hidden) plus
    /// anything installed.
    Available,
    /// Only installed versions.
    Installed,
}

/// One line of `hx toolchain list`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VersionRow {
    version: String,
    installed: bool,
    active: bool,
    recommended: bool,
    latest: bool,
}

impl VersionRow {
    fn describe(&self) -> String {
        let mut labels = Vec::new();
        if self.active {
            labels.push("active");
        } else if self.installed {
            labels.push("installed");
        }
        if self.recommended {
            labels.push("recommended");
        }
        if self.latest {
            labels.push("latest");
        }
        let icon = if self.installed {
            Style::success("✓").to_string()
        } else if self.recommended || self.latest {
            Style::info("○").to_string()
        } else {
            Style::dim("○").to_string()
        };
        if labels.is_empty() {
            icon
        } else {
            format!("{} ({})", icon, labels.join(", "))
        }
    }
}

/// Build the rows for one tool, newest first.
fn version_rows(
    listing: &[VersionListing],
    installed: &[&str],
    active: Option<&str>,
    mode: ListMode,
) -> Vec<VersionRow> {
    let mut rows: Vec<VersionRow> = listing
        .iter()
        .filter(|l| {
            let is_installed = installed.contains(&l.version.as_str());
            match mode {
                ListMode::Installed => is_installed,
                ListMode::Default => is_installed || l.recommended || l.latest,
                ListMode::Available => is_installed || !(l.old || l.prerelease),
            }
        })
        .map(|l| VersionRow {
            version: l.version.clone(),
            installed: installed.contains(&l.version.as_str()),
            active: active == Some(l.version.as_str()),
            recommended: l.recommended,
            latest: l.latest,
        })
        .collect();

    // Installed versions the listing does not know (custom or withdrawn builds)
    for version in installed {
        if !rows.iter().any(|r| r.version == *version) {
            rows.push(VersionRow {
                version: version.to_string(),
                installed: true,
                active: active == Some(*version),
                recommended: false,
                latest: false,
            });
        }
    }

    rows.sort_by(|a, b| metadata::compare_versions(&b.version, &a.version));
    rows
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

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(version: &str, recommended: bool, latest: bool, old: bool) -> VersionListing {
        VersionListing {
            version: version.to_string(),
            recommended,
            latest,
            old,
            prerelease: false,
        }
    }

    fn sample() -> Vec<VersionListing> {
        vec![
            listing("9.14.1", false, true, false),
            listing("9.12.2", false, false, false),
            listing("9.10.3", true, false, false),
            listing("9.8.4", false, false, false),
            listing("8.10.7", false, false, true),
        ]
    }

    fn versions(rows: &[VersionRow]) -> Vec<&str> {
        rows.iter().map(|r| r.version.as_str()).collect()
    }

    #[test]
    fn test_default_shows_installed_recommended_latest() {
        let rows = version_rows(&sample(), &["9.8.4"], Some("9.8.4"), ListMode::Default);
        assert_eq!(versions(&rows), ["9.14.1", "9.10.3", "9.8.4"]);
        assert!(rows[0].latest && !rows[0].installed);
        assert!(rows[1].recommended);
        assert!(rows[2].active && rows[2].installed);
    }

    #[test]
    fn test_available_hides_old_unless_installed() {
        let rows = version_rows(&sample(), &[], None, ListMode::Available);
        assert_eq!(versions(&rows), ["9.14.1", "9.12.2", "9.10.3", "9.8.4"]);

        let rows = version_rows(&sample(), &["8.10.7"], None, ListMode::Available);
        assert!(versions(&rows).contains(&"8.10.7"));
    }

    #[test]
    fn test_installed_only_includes_unknown_versions() {
        let rows = version_rows(
            &sample(),
            &["9.10.3", "9.12.3"],
            Some("9.12.3"),
            ListMode::Installed,
        );
        assert_eq!(versions(&rows), ["9.12.3", "9.10.3"]);
        assert!(rows[0].active);
        assert!(rows[1].recommended && rows[1].installed);
    }

    #[test]
    fn test_describe_labels() {
        let rows = version_rows(&sample(), &["9.10.3"], Some("9.10.3"), ListMode::Default);
        let recommended = rows.iter().find(|r| r.version == "9.10.3").unwrap();
        assert!(recommended.describe().contains("(active, recommended)"));
        let latest = rows.iter().find(|r| r.version == "9.14.1").unwrap();
        assert!(latest.describe().contains("(latest)"));
    }
}
