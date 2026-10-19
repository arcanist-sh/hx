//! Toolchain version checking and auto-installation.

use crate::detect::{ToolStatus, Toolchain};
use crate::install::{self, SmartCabalInstallOptions, SmartInstallOptions};
use hx_core::{Error, Fix, Result, Version};
use std::io::{self, IsTerminal, Write};
use tracing::info;

/// Result of checking toolchain requirements.
#[derive(Debug, Clone)]
pub struct ToolchainCheck {
    /// GHC version mismatch (expected, found)
    pub ghc_mismatch: Option<(String, Option<String>)>,
    /// Cabal version mismatch (expected, found)
    pub cabal_mismatch: Option<(String, Option<String>)>,
    /// Whether ghcup is available (for fallback)
    pub has_ghcup: bool,
}

impl ToolchainCheck {
    /// Check if there are any mismatches.
    pub fn has_mismatch(&self) -> bool {
        self.ghc_mismatch.is_some() || self.cabal_mismatch.is_some()
    }

    /// Check if mismatches can be auto-fixed.
    ///
    /// We can always auto-fix via direct download, ghcup is optional fallback.
    pub fn can_auto_fix(&self) -> bool {
        self.has_mismatch()
    }

    /// Get a description of the mismatches.
    pub fn mismatch_summary(&self) -> String {
        let mut parts = Vec::new();

        if let Some((expected, found)) = &self.ghc_mismatch {
            let found_str = found
                .as_ref()
                .map(|v| v.as_str())
                .unwrap_or("not installed");
            parts.push(format!("GHC {} (found: {})", expected, found_str));
        }

        if let Some((expected, found)) = &self.cabal_mismatch {
            let found_str = found
                .as_ref()
                .map(|v| v.as_str())
                .unwrap_or("not installed");
            parts.push(format!("Cabal {} (found: {})", expected, found_str));
        }

        parts.join(", ")
    }
}

/// Check toolchain requirements against detected versions.
pub fn check_requirements(
    toolchain: &Toolchain,
    required_ghc: Option<&str>,
    required_cabal: Option<&str>,
) -> ToolchainCheck {
    let ghc_mismatch = required_ghc.and_then(|required| {
        check_version_match(&toolchain.ghc.status, required)
            .map(|found| (required.to_string(), found))
    });

    let cabal_mismatch = required_cabal.and_then(|required| {
        check_version_match(&toolchain.cabal.status, required)
            .map(|found| (required.to_string(), found))
    });

    ToolchainCheck {
        ghc_mismatch,
        cabal_mismatch,
        has_ghcup: toolchain.has_ghcup(),
    }
}

/// Check if a tool version matches the requirement.
/// Returns Some(found_version) if there's a mismatch, None if it matches.
fn check_version_match(status: &ToolStatus, required: &str) -> Option<Option<String>> {
    match status {
        ToolStatus::Found { version, .. } => {
            // Parse required version
            let required_ver: Version = required.parse().ok()?;

            // Check if major.minor match, and patch if specified in required
            // If required is "9.8" (patch=0), match any 9.8.x
            // If required is "9.8.2", match exactly 9.8.2
            let patch_matches = if required.matches('.').count() < 2 {
                // Only major.minor specified (e.g., "9.8")
                true
            } else {
                version.patch == required_ver.patch
            };

            if version.major == required_ver.major
                && version.minor == required_ver.minor
                && patch_matches
            {
                None // Matches
            } else {
                Some(Some(version.to_string())) // Mismatch
            }
        }
        ToolStatus::NotFound => Some(None), // Not installed
        ToolStatus::VersionUnknown { .. } => None, // Can't check, assume ok
    }
}

/// Policy for auto-installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoInstallPolicy {
    /// Never auto-install, just warn
    Never,
    /// Prompt the user for confirmation
    #[default]
    Prompt,
    /// Automatically install without prompting
    Always,
}

/// Ensure the toolchain matches requirements, optionally installing missing versions.
///
/// When `required_ghc` or `required_cabal` are specified (from hx.toml), the effective
/// policy defaults to `Always` unless explicitly set to `Never`. This enables seamless
/// project-specific toolchain bootstrapping - just `hx build` in a project with hx.toml
/// and the correct toolchain versions will be downloaded automatically.
pub async fn ensure_toolchain(
    toolchain: &Toolchain,
    required_ghc: Option<&str>,
    required_cabal: Option<&str>,
    policy: AutoInstallPolicy,
) -> Result<()> {
    let check = check_requirements(toolchain, required_ghc, required_cabal);

    if !check.has_mismatch() {
        return Ok(());
    }

    info!("Toolchain mismatch detected: {}", check.mismatch_summary());

    // If project specifies versions, default to auto-install
    // This enables seamless bootstrapping: `hx build` just works
    let effective_policy = if required_ghc.is_some() || required_cabal.is_some() {
        match policy {
            AutoInstallPolicy::Never => AutoInstallPolicy::Never,
            _ => AutoInstallPolicy::Always,
        }
    } else {
        policy
    };

    match effective_policy {
        AutoInstallPolicy::Never => {
            return Err(Error::ToolchainMismatch {
                tool: "toolchain".to_string(),
                expected: check.mismatch_summary(),
                found: "auto-install disabled".to_string(),
                fixes: build_install_fixes(&check),
            });
        }
        AutoInstallPolicy::Prompt => {
            if !confirm_install(&check)? {
                return Err(Error::ToolchainMismatch {
                    tool: "toolchain".to_string(),
                    expected: check.mismatch_summary(),
                    found: "installation declined".to_string(),
                    fixes: build_install_fixes(&check),
                });
            }
        }
        AutoInstallPolicy::Always => {
            // Proceed with installation
        }
    }

    // Install missing/mismatched tools using smart installation
    // This tries direct download first, then falls back to ghcup
    if let Some((version, _)) = &check.ghc_mismatch {
        let options = SmartInstallOptions::new(version).with_set_active(true);
        install::install_ghc_smart(&options).await?;
    }

    if let Some((version, _)) = &check.cabal_mismatch {
        let options = SmartCabalInstallOptions::new(version).with_set_active(true);
        install::install_cabal_smart(&options).await?;
    }

    Ok(())
}

/// Build fix suggestions for a toolchain check.
fn build_install_fixes(check: &ToolchainCheck) -> Vec<Fix> {
    let mut fixes = Vec::new();

    if let Some((version, _)) = &check.ghc_mismatch {
        fixes.push(Fix::with_command(
            format!("Install GHC {}", version),
            format!("hx toolchain install --ghc {}", version),
        ));
    }

    if let Some((version, _)) = &check.cabal_mismatch {
        fixes.push(Fix::with_command(
            format!("Install Cabal {}", version),
            format!("hx toolchain install --cabal {}", version),
        ));
    }

    fixes
}

/// Prompt the user to confirm installation.
fn confirm_install(check: &ToolchainCheck) -> Result<bool> {
    // Check if we're in a non-interactive environment
    if std::env::var("CI").is_ok() || !io::stdin().is_terminal() {
        // Non-interactive: don't prompt, just fail
        return Ok(false);
    }

    eprint!(
        "\nToolchain mismatch: {}\nInstall missing toolchain components? [Y/n] ",
        check.mismatch_summary()
    );
    io::stderr().flush().map_err(|e| Error::Io {
        message: "failed to flush stderr".to_string(),
        path: None,
        source: e,
    })?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(|e| Error::Io {
        message: "failed to read input".to_string(),
        path: None,
        source: e,
    })?;

    let input = input.trim().to_lowercase();
    Ok(input.is_empty() || input == "y" || input == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::DetectedTool;
    use std::path::PathBuf;

    fn make_tool(name: &str, version: Option<&str>) -> DetectedTool {
        let status = match version {
            Some(v) => ToolStatus::Found {
                version: v.parse().unwrap(),
                path: PathBuf::from("/usr/bin").join(name),
            },
            None => ToolStatus::NotFound,
        };
        DetectedTool {
            name: name.to_string(),
            status,
            source: None,
        }
    }

    #[test]
    fn test_check_requirements_match() {
        let toolchain = Toolchain {
            ghc: make_tool("ghc", Some("9.8.2")),
            cabal: make_tool("cabal", Some("3.12.1.0")),
            ghcup: make_tool("ghcup", Some("0.1.30.0")),
            hls: make_tool("hls", Some("2.9.0.0")),
        };

        let check = check_requirements(&toolchain, Some("9.8.2"), Some("3.12.1.0"));
        assert!(!check.has_mismatch());
    }

    #[test]
    fn test_check_requirements_ghc_mismatch() {
        let toolchain = Toolchain {
            ghc: make_tool("ghc", Some("9.6.4")),
            cabal: make_tool("cabal", Some("3.12.1.0")),
            ghcup: make_tool("ghcup", Some("0.1.30.0")),
            hls: make_tool("hls", Some("2.9.0.0")),
        };

        let check = check_requirements(&toolchain, Some("9.8.2"), None);
        assert!(check.has_mismatch());
        assert!(check.ghc_mismatch.is_some());
        assert!(check.cabal_mismatch.is_none());
    }

    #[test]
    fn test_check_requirements_not_installed() {
        let toolchain = Toolchain {
            ghc: make_tool("ghc", None),
            cabal: make_tool("cabal", Some("3.12.1.0")),
            ghcup: make_tool("ghcup", Some("0.1.30.0")),
            hls: make_tool("hls", None),
        };

        let check = check_requirements(&toolchain, Some("9.8.2"), None);
        assert!(check.has_mismatch());
        assert!(check.ghc_mismatch.is_some());
        let (expected, found) = check.ghc_mismatch.unwrap();
        assert_eq!(expected, "9.8.2");
        assert!(found.is_none());
    }

    #[test]
    fn test_check_requirements_minor_version_mismatch() {
        let toolchain = Toolchain {
            ghc: make_tool("ghc", Some("9.8.1")),
            cabal: make_tool("cabal", Some("3.12.1.0")),
            ghcup: make_tool("ghcup", Some("0.1.30.0")),
            hls: make_tool("hls", Some("2.9.0.0")),
        };

        // 9.8.1 should match 9.8 (no patch specified)
        let check = check_requirements(&toolchain, Some("9.8"), None);
        assert!(!check.has_mismatch());

        // 9.8.1 should NOT match 9.8.2 (patch mismatch)
        let check = check_requirements(&toolchain, Some("9.8.2"), None);
        assert!(check.has_mismatch());
    }
}
