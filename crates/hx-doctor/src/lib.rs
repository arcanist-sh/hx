//! Diagnostic checks and fix suggestions for hx.
//!
//! This crate provides the `hx doctor` functionality to diagnose
//! toolchain and project issues, including BHC (Basel Haskell Compiler)
//! when configured.

use hx_config::{CompilerBackend, MANIFEST_FILENAME};
use hx_core::error::Fix;
use hx_solver::bhc_platform::{find_platform_for_bhc, latest_platform};
use hx_toolchain::{Toolchain, detect_bhc, install::ghcup_install_command};
use hx_ui::{Output, Style};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational
    Info,
    /// Warning - something is suboptimal
    Warning,
    /// Error - something is broken
    Error,
}

/// A diagnostic check result.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Severity of the issue
    pub severity: Severity,
    /// Short description
    pub message: String,
    /// Suggested fixes
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    /// Create an error diagnostic.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            fixes: vec![],
        }
    }

    /// Create a warning diagnostic.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            fixes: vec![],
        }
    }

    /// Create an info diagnostic.
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            message: message.into(),
            fixes: vec![],
        }
    }

    /// Add a fix to this diagnostic.
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fixes.push(fix);
        self
    }
}

/// Result of running doctor checks.
#[derive(Debug, Default)]
pub struct DoctorReport {
    /// All diagnostics found
    pub diagnostics: Vec<Diagnostic>,
}

impl DoctorReport {
    /// Check if there are any errors.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Check if there are any warnings.
    pub fn has_warnings(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning)
    }

    /// Add a diagnostic.
    pub fn add(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Get the count of each severity.
    pub fn counts(&self) -> (usize, usize, usize) {
        let errors = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let warnings = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        let info = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .count();
        (errors, warnings, info)
    }
}

/// Run all doctor checks.
pub async fn run_checks(project_dir: Option<&Path>, invoked_name: &str) -> DoctorReport {
    let mut report = DoctorReport::default();

    // Check whether our own name collides with another binary on PATH
    check_binary_shadowing(invoked_name, &mut report);

    // Detect toolchain
    let toolchain = Toolchain::detect().await;

    // Check ghcup
    check_ghcup(&toolchain, &mut report);

    // Check GHC
    check_ghc(&toolchain, &mut report);

    // Check Cabal
    check_cabal(&toolchain, &mut report);

    // Check HLS
    check_hls(&toolchain, &mut report);

    // Check BHC if configured
    check_bhc(project_dir, &mut report);

    // Check BHC Platform compatibility
    check_bhc_platform(&toolchain, project_dir, &mut report);

    // Check native dependencies (platform-specific)
    check_native_deps(&mut report);

    // Check cross-compilation toolchains
    check_cross_compilation(&mut report);

    // Check project if we're in one
    if let Some(dir) = project_dir {
        check_project(dir, &mut report);
    }

    report
}

/// Check whether the name hx was invoked as also resolves to another binary
/// on PATH.
///
/// Helix's editor binary is also called `hx` and ships in most distributions,
/// so a machine can easily end up with two different executables under one
/// name. Whichever comes first on PATH wins and the other becomes
/// unreachable, which fails confusingly in both directions: `hx build` opens
/// an editor on a file named `build`, or `hx foo.hs` reports an unknown
/// command. Diagnosing it means reasoning about PATH order, so say it plainly
/// instead.
fn check_binary_shadowing(invoked_name: &str, report: &mut DoctorReport) {
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let current = current.canonicalize().unwrap_or(current);

    let Ok(found) = which::which_all(invoked_name) else {
        return;
    };
    let found: Vec<PathBuf> = found.map(|p| p.canonicalize().unwrap_or(p)).collect();

    if let Some(diagnostic) = shadowing_diagnostic(invoked_name, &current, &found) {
        report.add(diagnostic);
    }
}

/// The decision half of [`check_binary_shadowing`], split out so every PATH
/// arrangement can be tested without touching the real environment.
///
/// `found` is every executable named `invoked_name` on PATH, in PATH order.
fn shadowing_diagnostic(
    invoked_name: &str,
    current: &Path,
    found: &[PathBuf],
) -> Option<Diagnostic> {
    // Not reached through PATH at all -- a build directory or an explicit
    // path. Shadowing is meaningless here, and warning would fire on every
    // `cargo run -- doctor`.
    if !found.iter().any(|p| p == current) {
        return None;
    }

    let others: Vec<&PathBuf> = found.iter().filter(|p| p.as_path() != current).collect();
    if others.is_empty() {
        return None;
    }

    let other_list = others
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    if found
        .first()
        .is_some_and(|first| first.as_path() == current)
    {
        // We win. That may well have broken someone's editor.
        Some(
            Diagnostic::warning(format!(
                "hx at {} shadows another `{invoked_name}` at {other_list}",
                current.display()
            ))
            .with_fix(Fix::new(format!(
                "If that is the Helix editor, reinstall hx under another name so both stay reachable, or move {} later in PATH",
                current.display()
            ))),
        )
    } else {
        // Something else wins. Typing our name does not reach us.
        let first = found.first()?;
        Some(
            Diagnostic::warning(format!(
                "`{invoked_name}` runs {}, not hx at {}",
                first.display(),
                current.display()
            ))
            .with_fix(Fix::new(format!(
                "Put {} earlier in PATH, or invoke hx by its full path",
                current
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| current.display().to_string())
            ))),
        )
    }
}

fn check_ghcup(toolchain: &Toolchain, report: &mut DoctorReport) {
    if !toolchain.ghcup.status.is_found() {
        report.add(
            Diagnostic::warning("ghcup not found - cannot manage toolchain versions")
                .with_fix(Fix::with_command("Install ghcup", ghcup_install_command())),
        );
    } else {
        report.add(Diagnostic::info(format!(
            "ghcup: {}",
            toolchain
                .ghcup
                .status
                .version()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "installed".to_string())
        )));
    }
}

fn check_ghc(toolchain: &Toolchain, report: &mut DoctorReport) {
    if !toolchain.ghc.status.is_found() {
        report.add(
            Diagnostic::error("ghc not found")
                .with_fix(Fix::with_command(
                    "Install GHC via ghcup",
                    "ghcup install ghc",
                ))
                .with_fix(Fix::with_command(
                    "Or use hx",
                    "hx toolchain install --ghc 9.8.2",
                )),
        );
    } else {
        report.add(Diagnostic::info(format!(
            "ghc: {}",
            toolchain
                .ghc
                .status
                .version()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "installed".to_string())
        )));
    }
}

fn check_cabal(toolchain: &Toolchain, report: &mut DoctorReport) {
    if !toolchain.cabal.status.is_found() {
        report.add(
            Diagnostic::error("cabal not found").with_fix(Fix::with_command(
                "Install Cabal via ghcup",
                "ghcup install cabal",
            )),
        );
    } else {
        report.add(Diagnostic::info(format!(
            "cabal: {}",
            toolchain
                .cabal
                .status
                .version()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "installed".to_string())
        )));
    }
}

fn check_hls(toolchain: &Toolchain, report: &mut DoctorReport) {
    if !toolchain.hls.status.is_found() {
        // Get recommended version if we have GHC
        let fix_cmd = if let Some(ghc_version) = toolchain.ghc.status.version() {
            if let Some(recommended) = hx_config::recommended_hls_for_ghc(&ghc_version.to_string())
            {
                format!("ghcup install hls {}", recommended)
            } else {
                "ghcup install hls".to_string()
            }
        } else {
            "ghcup install hls".to_string()
        };

        report.add(
            Diagnostic::warning("haskell-language-server not found - IDE features unavailable")
                .with_fix(Fix::with_command("Install HLS via ghcup", fix_cmd))
                .with_fix(Fix::with_command(
                    "Or install via hx",
                    "hx toolchain install --hls latest",
                )),
        );
    } else {
        let hls_version = toolchain
            .hls
            .status
            .version()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "installed".to_string());

        report.add(Diagnostic::info(format!("hls: {}", hls_version)));

        // Check HLS/GHC compatibility if both versions are known
        if let (Some(hls_ver), Some(ghc_ver)) = (
            toolchain.hls.status.version(),
            toolchain.ghc.status.version(),
        ) {
            let hls_str = hls_ver.to_string();
            let ghc_str = ghc_ver.to_string();

            if !hx_config::is_hls_compatible(&hls_str, &ghc_str) {
                let recommended = hx_config::recommended_hls_for_ghc(&ghc_str);
                let mut diag = Diagnostic::warning(format!(
                    "HLS {} may not be fully compatible with GHC {}",
                    hls_str, ghc_str
                ));

                if let Some(rec_version) = recommended {
                    diag = diag.with_fix(Fix::with_command(
                        format!(
                            "Install HLS {} (recommended for GHC {})",
                            rec_version, ghc_str
                        ),
                        format!(
                            "ghcup install hls {} && ghcup set hls {}",
                            rec_version, rec_version
                        ),
                    ));
                }

                report.add(diag);
            }
        }
    }
}

fn check_bhc(project_dir: Option<&Path>, report: &mut DoctorReport) {
    // Check if project uses BHC backend
    let uses_bhc = project_dir
        .and_then(|dir| hx_config::Project::load(dir).ok())
        .map(|project| project.manifest.compiler.backend == CompilerBackend::Bhc)
        .unwrap_or(false);

    // Check BHC installation
    if let Some(bhc) = detect_bhc() {
        report.add(Diagnostic::info(format!("bhc: {}", bhc.version)));

        // If project uses BHC, also check compatibility
        if uses_bhc
            && let Some(dir) = project_dir
            && let Ok(project) = hx_config::Project::load(dir)
            && let Some(ref required) = project.manifest.compiler.version
            && &bhc.version != required
        {
            report.add(
                Diagnostic::warning(format!(
                    "BHC version mismatch: required {}, found {}",
                    required, bhc.version
                ))
                .with_fix(Fix::with_command(
                    format!("Install BHC {}", required),
                    format!("hx toolchain install --bhc {}", required),
                )),
            );
        }
    } else if uses_bhc {
        // BHC is required but not installed
        report.add(
            Diagnostic::error("BHC not found (required by project)").with_fix(Fix::with_command(
                "Install BHC",
                "hx toolchain install --bhc latest",
            )),
        );
    } else {
        // BHC is optional - just note it's not installed
        report.add(Diagnostic::info(
            "bhc: not installed (optional - install for alternative compiler backend)",
        ));
    }
}

fn check_bhc_platform(
    toolchain: &Toolchain,
    project_dir: Option<&Path>,
    report: &mut DoctorReport,
) {
    let bhc = match detect_bhc() {
        Some(bhc) => bhc,
        None => return, // No BHC installed, nothing to check
    };

    // Find matching platform for installed BHC
    match find_platform_for_bhc(&bhc.version) {
        Some(platform) => {
            report.add(Diagnostic::info(format!(
                "BHC Platform: {} ({} packages)",
                platform.id, platform.package_count
            )));

            // Check GHC version matches what the platform expects
            if let Some(ghc_ver) = toolchain.ghc.status.version() {
                let ghc_str = ghc_ver.to_string();
                if ghc_str != platform.ghc_compat {
                    report.add(
                        Diagnostic::warning(format!(
                            "GHC {} does not match BHC Platform {} requirement (GHC {})",
                            ghc_str, platform.id, platform.ghc_compat
                        ))
                        .with_fix(Fix::with_command(
                            format!("Install GHC {}", platform.ghc_compat),
                            format!("hx toolchain install --ghc {}", platform.ghc_compat),
                        )),
                    );
                }
            }

            // Suggest upgrade if a newer platform exists
            if let Some(latest) = latest_platform()
                && latest.id != platform.id
            {
                report.add(
                    Diagnostic::info(format!(
                        "Newer BHC Platform available: {} (requires BHC {})",
                        latest.id, latest.bhc_version
                    ))
                    .with_fix(Fix::with_command(
                        format!("Upgrade BHC to {}", latest.bhc_version),
                        format!("hx toolchain install --bhc {}", latest.bhc_version),
                    )),
                );
            }
        }
        None => {
            report.add(
                Diagnostic::warning(format!(
                    "No BHC Platform snapshot matches BHC {}",
                    bhc.version
                ))
                .with_fix(Fix::with_command(
                    "Install a supported BHC version",
                    "hx toolchain install --bhc latest",
                )),
            );
        }
    }

    // Verify project-configured snapshot is compatible with installed BHC
    if let Some(dir) = project_dir
        && let Ok(project) = hx_config::Project::load(dir)
        && project.manifest.compiler.backend == CompilerBackend::Bhc
        && let Some(ref snapshot_id) = project.manifest.bhc_platform.snapshot
        && let Some(platform) = find_platform_for_bhc(&bhc.version)
        && platform.id != snapshot_id.as_str()
    {
        report.add(
            Diagnostic::warning(format!(
                "Project uses snapshot '{}' but installed BHC {} matches '{}'",
                snapshot_id, bhc.version, platform.id
            ))
            .with_fix(Fix::new(format!(
                "Update [bhc-platform] snapshot to \"{}\" in hx.toml",
                platform.id
            ))),
        );
    }
}

fn check_native_deps(report: &mut DoctorReport) {
    #[cfg(target_os = "linux")]
    check_native_deps_linux(report);

    #[cfg(target_os = "macos")]
    check_native_deps_macos(report);

    #[cfg(target_os = "windows")]
    check_native_deps_windows(report);
}

#[cfg(target_os = "linux")]
fn check_native_deps_linux(report: &mut DoctorReport) {
    // Check for common native libraries needed by GHC
    // Format: (pkg-config name, debian pkg, fedora pkg, arch pkg, description)
    let libs = [
        (
            "gmp",
            "libgmp-dev",
            "gmp-devel",
            "gmp",
            "GMP arithmetic library",
        ),
        (
            "zlib",
            "zlib1g-dev",
            "zlib-devel",
            "zlib",
            "zlib compression",
        ),
        (
            "ncurses",
            "libncurses-dev",
            "ncurses-devel",
            "ncurses",
            "ncurses terminal library",
        ),
        (
            "libffi",
            "libffi-dev",
            "libffi-devel",
            "libffi",
            "Foreign Function Interface",
        ),
    ];

    let mut missing_libs = Vec::new();

    for (pkg_name, deb_pkg, fedora_pkg, arch_pkg, description) in libs {
        if !check_library_linux(pkg_name) {
            missing_libs.push((pkg_name, deb_pkg, fedora_pkg, arch_pkg, description));
        }
    }

    if missing_libs.is_empty() {
        report.add(Diagnostic::info("Native libraries: all found"));
    } else {
        for (pkg_name, deb_pkg, fedora_pkg, arch_pkg, description) in missing_libs {
            report.add(
                Diagnostic::warning(format!(
                    "{} not found - {} may fail to build",
                    pkg_name, description
                ))
                .with_fix(Fix::with_command(
                    "Install on Debian/Ubuntu",
                    format!("sudo apt-get install {}", deb_pkg),
                ))
                .with_fix(Fix::with_command(
                    "Install on Fedora/RHEL",
                    format!("sudo dnf install {}", fedora_pkg),
                ))
                .with_fix(Fix::with_command(
                    "Install on Arch Linux",
                    format!("sudo pacman -S {}", arch_pkg),
                )),
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn check_library_linux(lib: &str) -> bool {
    // Try pkg-config first (most reliable)
    if let Ok(output) = Command::new("pkg-config").args(["--exists", lib]).output()
        && output.status.success()
    {
        return true;
    }

    // Try ldconfig as fallback
    if let Ok(output) = Command::new("ldconfig").args(["-p"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains(lib) {
            return true;
        }
    }

    // Check common library paths for various architectures
    let lib_paths = [
        // x86_64
        format!("/usr/lib/x86_64-linux-gnu/lib{}.so", lib),
        format!("/usr/lib64/lib{}.so", lib),
        // aarch64
        format!("/usr/lib/aarch64-linux-gnu/lib{}.so", lib),
        // Generic
        format!("/usr/lib/lib{}.so", lib),
        format!("/lib/lib{}.so", lib),
    ];

    for path in lib_paths {
        if std::path::Path::new(&path).exists() {
            return true;
        }
    }

    false
}

#[cfg(target_os = "macos")]
fn check_native_deps_macos(report: &mut DoctorReport) {
    // Check for Xcode Command Line Tools
    let has_xcode_cli = Command::new("xcode-select")
        .args(["--print-path"])
        .output()
        .is_ok_and(|o| o.status.success());

    if !has_xcode_cli {
        report.add(
            Diagnostic::warning("Xcode Command Line Tools not found - required for building")
                .with_fix(Fix::with_command(
                    "Install Xcode CLI Tools",
                    "xcode-select --install",
                )),
        );
    } else {
        report.add(Diagnostic::info("Xcode CLI Tools: installed"));
    }

    // Check if Homebrew is available
    let has_brew = Command::new("brew").arg("--version").output().is_ok();

    if !has_brew {
        report.add(
            Diagnostic::warning("Homebrew not found - recommended for installing native dependencies")
                .with_fix(Fix::with_command(
                    "Install Homebrew",
                    "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"",
                )),
        );
        return;
    }

    // Check for common dependencies
    let deps = [
        ("gmp", "GMP arithmetic library"),
        ("libffi", "Foreign Function Interface"),
    ];

    let mut all_found = true;
    for (formula, description) in deps {
        if !check_native_lib_macos(formula) {
            all_found = false;
            report.add(
                Diagnostic::warning(format!(
                    "{} not found - {} may fail to build",
                    formula, description
                ))
                .with_fix(Fix::with_command(
                    format!("Install {}", formula),
                    format!("brew install {}", formula),
                )),
            );
        }
    }

    if all_found {
        report.add(Diagnostic::info("Native libraries: all found"));
    }
}

#[cfg(target_os = "macos")]
fn check_native_lib_macos(lib: &str) -> bool {
    // Try pkg-config first
    if Command::new("pkg-config")
        .args(["--exists", lib])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return true;
    }

    // Try brew list
    if Command::new("brew")
        .args(["list", lib])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return true;
    }

    // Check common paths
    let paths = [
        format!("/opt/homebrew/lib/lib{}.dylib", lib),
        format!("/usr/local/lib/lib{}.dylib", lib),
        format!("/opt/homebrew/opt/{}/lib/lib{}.dylib", lib, lib),
        format!("/usr/local/opt/{}/lib/lib{}.dylib", lib, lib),
    ];

    for path in paths {
        if std::path::Path::new(&path).exists() {
            return true;
        }
    }

    false
}

#[cfg(target_os = "windows")]
fn check_native_deps_windows(report: &mut DoctorReport) {
    // Check for MSYS2/MinGW which provides native libs on Windows
    let msys2_paths = ["C:\\msys64", "C:\\msys32", "C:\\ghcup\\msys64"];

    let msys2_path = msys2_paths
        .iter()
        .find(|p| std::path::Path::new(p).exists());

    if let Some(path) = msys2_path {
        report.add(Diagnostic::info(format!("MSYS2: found at {}", path)));

        // Check for required libraries in MSYS2
        let mingw_lib_path = format!("{}\\mingw64\\lib", path);
        let libs = [
            ("libgmp", "GMP arithmetic library"),
            ("libffi", "Foreign Function Interface"),
            ("libz", "zlib compression"),
        ];

        for (lib, description) in libs {
            let lib_file = format!("{}\\{}.a", mingw_lib_path, lib);
            if !std::path::Path::new(&lib_file).exists() {
                report.add(
                    Diagnostic::warning(format!(
                        "{} not found in MSYS2 - {} may fail to build",
                        lib, description
                    ))
                    .with_fix(Fix::with_command(
                        format!("Install {} via MSYS2", lib),
                        format!(
                            "pacman -S mingw-w64-x86_64-{}",
                            lib.trim_start_matches("lib")
                        ),
                    )),
                );
            }
        }
    } else {
        report.add(
            Diagnostic::warning("MSYS2 not found - some packages may fail to build")
                .with_fix(Fix::new("Install MSYS2 from https://www.msys2.org/"))
                .with_fix(Fix::new("Or install via ghcup which includes MSYS2")),
        );
    }

    // Check for chocolatey as alternative package manager
    let has_choco = Command::new("choco")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());

    if has_choco {
        report.add(Diagnostic::info("Chocolatey: installed"));
    }
}

fn check_project(dir: &Path, report: &mut DoctorReport) {
    // Check for hx.toml
    let manifest_path = dir.join(MANIFEST_FILENAME);
    if !manifest_path.exists() {
        report.add(
            Diagnostic::warning(format!("{} not found", MANIFEST_FILENAME))
                .with_fix(Fix::with_command("Initialize hx project", "hx init")),
        );
    } else {
        report.add(Diagnostic::info(format!("{} found", MANIFEST_FILENAME)));
    }

    // Check for .cabal file
    let has_cabal = std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|ext| ext == "cabal"))
        })
        .unwrap_or(false);

    if !has_cabal {
        report.add(
            Diagnostic::error("no .cabal file found")
                .with_fix(Fix::with_command("Initialize project", "hx init")),
        );
    } else {
        report.add(Diagnostic::info(".cabal file found"));
    }

    // Check hie.yaml for IDE integration
    check_hie_yaml(dir, report);

    // Check Stackage configuration
    check_stackage_config(dir, report);
}

fn check_stackage_config(dir: &Path, report: &mut DoctorReport) {
    if let Ok(project) = hx_config::Project::load(dir)
        && let Some(ref snapshot) = project.manifest.stackage.snapshot
    {
        // Check if snapshot identifier looks valid
        let is_valid = snapshot.starts_with("lts-") || snapshot.starts_with("nightly");
        if !is_valid {
            report.add(
                Diagnostic::warning(format!(
                    "Invalid Stackage snapshot identifier: {}",
                    snapshot
                ))
                .with_fix(Fix::new("Use format: lts-XX.YY or nightly-YYYY-MM-DD"))
                .with_fix(Fix::with_command(
                    "List available snapshots",
                    "hx stackage list",
                )),
            );
        } else {
            report.add(Diagnostic::info(format!("Stackage snapshot: {}", snapshot)));
        }
    }
}

/// Check for cross-compilation toolchains.
pub fn check_cross_compilation(report: &mut DoctorReport) {
    // Common cross-compilation targets
    let targets = [
        ("aarch64-linux-gnu", "aarch64-linux-gnu-gcc", "ARM64 Linux"),
        ("x86_64-linux-gnu", "x86_64-linux-gnu-gcc", "x86_64 Linux"),
        (
            "aarch64-apple-darwin",
            "aarch64-apple-darwin-gcc",
            "ARM64 macOS",
        ),
    ];

    let mut found_any = false;

    for (_target, compiler, description) in targets {
        if check_cross_compiler(compiler) {
            found_any = true;
            report.add(Diagnostic::info(format!(
                "Cross-compiler for {}: available",
                description
            )));
        }
    }

    if !found_any {
        report.add(Diagnostic::info(
            "No cross-compilation toolchains detected (install if needed for --target builds)",
        ));
    }

    // Check for common cross-compilation tools
    #[cfg(target_os = "macos")]
    {
        // On macOS, check for Linux cross-compilers via Homebrew
        if !check_cross_compiler("x86_64-linux-gnu-gcc")
            && !check_cross_compiler("aarch64-linux-gnu-gcc")
        {
            report.add(
                Diagnostic::info("Linux cross-compilers not found")
                    .with_fix(Fix::with_command(
                        "Install x86_64 Linux cross-compiler",
                        "brew install x86_64-linux-gnu-gcc",
                    ))
                    .with_fix(Fix::with_command(
                        "Install ARM64 Linux cross-compiler",
                        "brew install aarch64-linux-gnu-gcc",
                    )),
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux, check for common cross-compilers
        if !check_cross_compiler("aarch64-linux-gnu-gcc") {
            report.add(Diagnostic::info("ARM64 cross-compiler not found").with_fix(
                Fix::with_command(
                    "Install on Debian/Ubuntu",
                    "sudo apt-get install gcc-aarch64-linux-gnu",
                ),
            ));
        }
    }
}

fn check_cross_compiler(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn check_hie_yaml(dir: &Path, report: &mut DoctorReport) {
    // Try to load project to check hie.yaml status
    if let Ok(project) = hx_config::Project::load(dir) {
        match hx_config::check_hie_yaml(&project) {
            hx_config::HieYamlStatus::Missing => {
                // Only suggest creating hie.yaml for workspace projects or complex setups
                if project.is_workspace() || project.has_cabal_project {
                    report.add(
                        Diagnostic::warning(
                            "hie.yaml missing - HLS may not work correctly with multi-package project",
                        )
                        .with_fix(Fix::with_command(
                            "Generate hie.yaml for IDE integration",
                            "hx ide setup",
                        )),
                    );
                }
            }
            hx_config::HieYamlStatus::Outdated => {
                report.add(
                    Diagnostic::warning("hie.yaml is outdated - workspace structure has changed")
                        .with_fix(Fix::with_command(
                            "Regenerate hie.yaml",
                            "hx ide setup --force",
                        )),
                );
            }
            hx_config::HieYamlStatus::UpToDate => {
                report.add(Diagnostic::info("hie.yaml up to date"));
            }
            hx_config::HieYamlStatus::Exists => {
                report.add(Diagnostic::info("hie.yaml found (custom configuration)"));
            }
        }
    }
}

/// Print the doctor report.
pub fn print_report(report: &DoctorReport, output: &Output) {
    output.header("Doctor Report");

    for diagnostic in &report.diagnostics {
        let prefix = match diagnostic.severity {
            Severity::Error => format!("{}", Style::error("✗")),
            Severity::Warning => format!("{}", Style::warning("⚠")),
            Severity::Info => format!("{}", Style::success("✓")),
        };

        eprintln!("  {} {}", prefix, diagnostic.message);

        for fix in &diagnostic.fixes {
            if let Some(ref cmd) = fix.command {
                eprintln!("    {} {}", Style::dim("fix:"), Style::command(cmd));
            } else {
                eprintln!("    {} {}", Style::dim("fix:"), fix.description);
            }
        }
    }

    let (errors, warnings, _) = report.counts();
    eprintln!();
    if errors > 0 {
        eprintln!(
            "{} {} error(s), {} warning(s)",
            Style::error("✗"),
            errors,
            warnings
        );
    } else if warnings > 0 {
        eprintln!("{} {} warning(s)", Style::warning("⚠"), warnings);
    } else {
        eprintln!("{} All checks passed", Style::success("✓"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn shadowing_silent_when_not_reached_through_path() {
        // Running from a build directory: the `hx` on PATH is a different
        // binary, but that is expected and not worth a warning.
        let current = p("/repo/target/debug/hx");
        let found = vec![p("/usr/bin/hx")];
        assert!(shadowing_diagnostic("hx", &current, &found).is_none());
    }

    #[test]
    fn shadowing_silent_when_we_are_the_only_match() {
        let current = p("/home/u/.local/bin/hx");
        let found = vec![current.clone()];
        assert!(shadowing_diagnostic("hx", &current, &found).is_none());
    }

    #[test]
    fn shadowing_silent_when_path_has_no_matches() {
        let current = p("/home/u/.local/bin/hx");
        assert!(shadowing_diagnostic("hx", &current, &[]).is_none());
    }

    #[test]
    fn shadowing_warns_when_we_take_precedence_over_another() {
        // hx installed ahead of Helix: their editor is now unreachable.
        let current = p("/home/u/.local/bin/hx");
        let found = vec![current.clone(), p("/usr/bin/hx")];

        let diag = shadowing_diagnostic("hx", &current, &found).expect("expected a warning");
        assert_eq!(diag.severity, Severity::Warning);
        assert!(diag.message.contains("/usr/bin/hx"), "{}", diag.message);
        assert!(diag.message.contains("shadows"), "{}", diag.message);
        assert!(!diag.fixes.is_empty());
    }

    #[test]
    fn shadowing_warns_when_another_takes_precedence_over_us() {
        // Helix wins: typing `hx` never reaches the toolchain.
        let current = p("/home/u/.local/bin/hx");
        let found = vec![p("/usr/bin/hx"), current.clone()];

        let diag = shadowing_diagnostic("hx", &current, &found).expect("expected a warning");
        assert_eq!(diag.severity, Severity::Warning);
        assert!(diag.message.contains("/usr/bin/hx"), "{}", diag.message);
        assert!(diag.message.contains("not hx at"), "{}", diag.message);
    }

    #[test]
    fn shadowing_uses_the_invoked_name_not_a_hardcoded_one() {
        let current = p("/home/u/.local/bin/hxs");
        let found = vec![current.clone(), p("/usr/bin/hxs")];

        let diag = shadowing_diagnostic("hxs", &current, &found).expect("expected a warning");
        assert!(diag.message.contains("`hxs`"), "{}", diag.message);
    }
}
