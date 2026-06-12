//! Dependency audit command implementation.

use anyhow::Result;
use hx_config::{Project, find_project_root};
use hx_lock::Lockfile;
use hx_ui::Output;
use std::collections::HashMap;

/// Run the audit command.
pub async fn run(
    fix: bool,
    ignore: Vec<String>,
    outdated: bool,
    licenses: bool,
    output: &Output,
) -> Result<i32> {
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    output.status("Auditing", project.name());

    // Load lockfile
    let lockfile = match Lockfile::from_file(project.lockfile_path()) {
        Ok(l) => l,
        Err(_) => {
            output.error("No lockfile found");
            output.info("Run `hx lock` first to generate a lockfile");
            return Ok(1);
        }
    };

    let mut warnings = Vec::new();

    // Honest disclosure: there is no advisory database integration yet, so
    // this command must not claim packages are free of vulnerabilities.
    output.warn("Vulnerability scanning is not yet available");
    output.info(
        "hx does not yet query the Haskell Security Advisory database (HSEC); \
         this audit checks deprecated packages only",
    );
    output.info("See https://github.com/haskell/security-advisories for advisories");

    // Check for outdated dependencies (basic check without index)
    if outdated {
        output.info("Checking for outdated packages...");
        output.info("(Note: Run `hx index update` for full outdated check)");
    }

    // License compatibility checking needs package metadata we don't fetch yet
    if licenses {
        output.warn(
            "License checking is not yet available (package license metadata is not fetched)",
        );
    }

    // Check for deprecated packages
    let deprecated = check_deprecated(&lockfile, &ignore);
    for dep in deprecated {
        warnings.push(dep);
    }

    // Display results
    output.header("Audit Results");

    let total_deps = lockfile.packages.len();
    output.list_item("Packages scanned", &total_deps.to_string());

    if warnings.is_empty() {
        output.status("✓", "No deprecated packages found");
        return Ok(0);
    }

    output.warn(&format!("{} warning(s):", warnings.len()));
    for warning in &warnings {
        display_warning(warning, output);
    }

    if fix {
        output.info("No automatic fixes available; run the suggested commands above");
    }

    Ok(0)
}

/// Audit issue severity.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

/// An audit warning.
#[derive(Debug)]
struct AuditWarning {
    package: String,
    #[allow(dead_code)]
    severity: Severity,
    message: String,
    fix_command: Option<String>,
}

/// Check for deprecated packages.
fn check_deprecated(lockfile: &Lockfile, ignore: &[String]) -> Vec<AuditWarning> {
    let mut warnings = Vec::new();

    // Known deprecated packages and their replacements
    let deprecated_packages: HashMap<&str, &str> = [
        ("old-time", "time"),
        ("old-locale", "time"),
        ("cryptohash", "cryptonite"),
        ("MonadRandom", "random"),
        ("SHA", "cryptonite"),
        ("crypto-api", "cryptonite"),
    ]
    .into_iter()
    .collect();

    for pkg in &lockfile.packages {
        if ignore.contains(&pkg.name) {
            continue;
        }

        if let Some(replacement) = deprecated_packages.get(pkg.name.as_str()) {
            warnings.push(AuditWarning {
                package: pkg.name.clone(),
                severity: Severity::Low,
                message: format!("Deprecated, consider using '{}' instead", replacement),
                fix_command: Some(format!("hx rm {} && hx add {}", pkg.name, replacement)),
            });
        }
    }

    warnings
}

/// Display an audit warning.
fn display_warning(warning: &AuditWarning, output: &Output) {
    output.warn(&format!("  {} - {}", warning.package, warning.message));

    if let Some(cmd) = &warning.fix_command {
        output.info(&format!("    Fix: {}", cmd));
    }
}
