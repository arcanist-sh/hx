//! Stackage snapshot management commands.

use anyhow::Result;
use hx_config::{MANIFEST_FILENAME, Manifest, Project, find_project_root};
use hx_solver::{SnapshotId, get_latest_lts, get_latest_nightly, load_snapshot};
use hx_ui::Output;

use crate::cli::StackageCommands;

/// Run the stackage command.
pub async fn run(command: StackageCommands, output: &Output) -> Result<i32> {
    match command {
        StackageCommands::List {
            lts,
            nightly,
            limit,
        } => list(lts, nightly, limit, output).await,
        StackageCommands::Info { snapshot, packages } => info(&snapshot, packages, output).await,
        StackageCommands::Set { snapshot } => set(&snapshot, output).await,
    }
}

/// List available Stackage snapshots.
async fn list(lts_only: bool, nightly_only: bool, limit: usize, output: &Output) -> Result<i32> {
    output.status("Fetching", "Stackage snapshot information...");

    // Get latest snapshots
    let show_lts = !nightly_only;
    let show_nightly = !lts_only;

    if show_lts {
        output.info("LTS Snapshots (Long-Term Support):");
        match get_latest_lts().await {
            Ok(latest) => {
                // Show the latest and some previous versions
                let major = latest.major.unwrap_or(22);
                let minor = latest.minor.unwrap_or(0);

                let mut count = 0;
                for m in (0..=minor).rev() {
                    if count >= limit {
                        break;
                    }
                    let snapshot_id = format!("lts-{}.{}", major, m);

                    // Try to load to get GHC version
                    if let Ok(id) = SnapshotId::parse(&snapshot_id) {
                        if let Ok(snap) = load_snapshot(&id, None).await {
                            output.info(&format!(
                                "  {} (GHC {}, {} packages)",
                                snapshot_id,
                                snap.metadata.ghc_version,
                                snap.packages.len()
                            ));
                        } else {
                            output.info(&format!("  {}", snapshot_id));
                        }
                    }
                    count += 1;
                }

                // Show previous major versions
                if count < limit && major > 19 {
                    for prev_major in (19..major).rev() {
                        if count >= limit {
                            break;
                        }
                        output.info(&format!(
                            "  lts-{} (use 'hx stackage info lts-{}' for details)",
                            prev_major, prev_major
                        ));
                        count += 1;
                    }
                }
            }
            Err(e) => {
                output.warn(&format!("Could not fetch latest LTS: {}", e));
                output.info("  Try: lts-24, lts-23, lts-22");
            }
        }
    }

    if show_nightly {
        if show_lts {
            output.info("");
        }
        output.info("Nightly Snapshots:");
        match get_latest_nightly().await {
            Ok(latest) => {
                if let Some(date) = &latest.date {
                    output.info(&format!("  nightly-{} (latest)", date));
                } else {
                    output.info("  nightly (latest)");
                }
                output.info("  Use 'hx stackage info nightly' for details");
            }
            Err(e) => {
                output.warn(&format!("Could not fetch latest nightly: {}", e));
                output.info("  Try: nightly, nightly-2026-10-19");
            }
        }
    }

    output.info("");
    output.info("Set a snapshot with: hx stackage set <snapshot>");

    Ok(0)
}

/// Show information about a specific snapshot.
async fn info(snapshot_str: &str, show_packages: bool, output: &Output) -> Result<i32> {
    output.status("Loading", &format!("snapshot {}...", snapshot_str));

    let snapshot_id = match SnapshotId::parse(snapshot_str) {
        Ok(id) => id,
        Err(e) => {
            output.error(&format!(
                "Invalid snapshot identifier '{}': {}",
                snapshot_str, e
            ));
            output.info("Examples: lts-24.64, lts-24, nightly-2026-10-19, nightly");
            return Ok(1);
        }
    };

    let snapshot = match load_snapshot(&snapshot_id, None).await {
        Ok(snap) => snap,
        Err(e) => {
            output.error(&format!("Failed to load snapshot: {}", e));
            return Ok(1);
        }
    };

    output.info(&format!("Snapshot: {}", snapshot_str));
    output.info(&format!("GHC Version: {}", snapshot.metadata.ghc_version));
    output.info(&format!("Packages: {}", snapshot.packages.len()));

    if let Some(created) = &snapshot.metadata.created {
        output.info(&format!("Created: {}", created));
    }

    if show_packages {
        output.info("");
        output.info("Packages:");

        // Sort packages alphabetically
        let mut packages: Vec<_> = snapshot.packages.values().collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name));

        for pkg in packages {
            output.info(&format!("  {} {}", pkg.name, pkg.version));
        }
    } else {
        output.info("");
        output.info("Use --packages to see all packages in this snapshot");
    }

    output.info("");
    output.info(&format!(
        "To use this snapshot: hx stackage set {}",
        snapshot_str
    ));

    Ok(0)
}

/// Set the Stackage snapshot for the current project.
async fn set(snapshot_str: &str, output: &Output) -> Result<i32> {
    // Validate the snapshot identifier
    let snapshot_id = match SnapshotId::parse(snapshot_str) {
        Ok(id) => id,
        Err(e) => {
            output.error(&format!(
                "Invalid snapshot identifier '{}': {}",
                snapshot_str, e
            ));
            output.info("Examples: lts-24.64, lts-24, nightly-2026-10-19, nightly");
            return Ok(1);
        }
    };

    // Try to load the snapshot to validate it exists
    output.status("Validating", &format!("snapshot {}...", snapshot_str));
    let snapshot = match load_snapshot(&snapshot_id, None).await {
        Ok(snap) => snap,
        Err(e) => {
            output.error(&format!("Failed to load snapshot: {}", e));
            output.info("The snapshot may not exist or there may be a network issue.");
            return Ok(1);
        }
    };

    // Find project root
    let project_root = match find_project_root(".") {
        Ok(root) => root,
        Err(_) => {
            output.error("No hx project found");
            output.info("Run 'hx init' to create a new project");
            return Ok(1);
        }
    };

    // Load and modify manifest
    let manifest_path = project_root.join(MANIFEST_FILENAME);
    let mut manifest = Manifest::from_file(&manifest_path)?;

    // Update the snapshot
    let old_snapshot = manifest.stackage.snapshot.clone();
    manifest.stackage.snapshot = Some(snapshot_str.to_string());

    // Write back
    manifest.to_file(&manifest_path)?;

    if let Some(old) = old_snapshot {
        output.success_summary(
            &format!(
                "Changed snapshot from {} to {} (GHC {})",
                old, snapshot_str, snapshot.metadata.ghc_version
            ),
            std::time::Duration::from_secs(0),
        );
    } else {
        output.success_summary(
            &format!(
                "Set snapshot to {} (GHC {})",
                snapshot_str, snapshot.metadata.ghc_version
            ),
            std::time::Duration::from_secs(0),
        );
    }

    output.info("");
    output.info("Run 'hx lock' to update dependencies with the new snapshot");

    // Warn if GHC version mismatch
    let project = Project::load(&project_root)?;
    if let Some(ref project_ghc) = project.manifest.toolchain.ghc {
        if *project_ghc != snapshot.metadata.ghc_version {
            output.warn(&format!(
                "Project uses GHC {} but snapshot uses GHC {}",
                project_ghc, snapshot.metadata.ghc_version
            ));
            output.info(&format!(
                "Consider updating [toolchain].ghc to \"{}\" in hx.toml",
                snapshot.metadata.ghc_version
            ));
        }
    }

    Ok(0)
}
