//! BHC (Basel Haskell Compiler) detection and installation.
//!
//! This module provides:
//! - BHC version detection
//! - BHC binary download and installation
//! - Version management

use futures_util::StreamExt;
use hx_ui::{Progress, Spinner};
use reqwest::Client;
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tar::Archive;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Error type for BHC operations.
#[derive(Debug, Error)]
pub enum BhcError {
    #[error("BHC not found in PATH")]
    NotFound,

    #[error("failed to parse BHC version: {0}")]
    VersionParse(String),

    #[error("BHC version {0} is not known")]
    UnknownVersion(String),

    #[error("download failed: {0}")]
    Download(String),

    #[error("installation failed: {0}")]
    Install(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("network error: {0}")]
    Network(String),
}

/// Result type for BHC operations.
pub type Result<T> = std::result::Result<T, BhcError>;

/// BHC version information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BhcVersion {
    /// Major version number.
    pub major: u32,
    /// Minor version number.
    pub minor: u32,
    /// Patch version number.
    pub patch: u32,
}

impl BhcVersion {
    /// Parse a version string like "0.1.0".
    pub fn parse(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.trim().split('.').collect();
        if parts.len() != 3 {
            return Err(BhcError::VersionParse(s.to_string()));
        }

        let major = parts[0]
            .parse()
            .map_err(|_| BhcError::VersionParse(s.to_string()))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| BhcError::VersionParse(s.to_string()))?;
        let patch = parts[2]
            .parse()
            .map_err(|_| BhcError::VersionParse(s.to_string()))?;

        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for BhcVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Known BHC versions published at `arcanist-sh/bhc`.
pub const KNOWN_BHC_VERSIONS: &[&str] = &["0.2.0", "0.2.1", "0.2.2", "0.2.3"];

/// Recommended BHC version.
pub const RECOMMENDED_BHC_VERSION: &str = "0.2.3";

/// Check if a version string is a known BHC version.
pub fn is_known_version(version: &str) -> bool {
    KNOWN_BHC_VERSIONS.contains(&version)
}

/// Check if a version string is valid (parseable).
pub fn is_valid_version(version: &str) -> bool {
    BhcVersion::parse(version).is_ok()
}

/// Get the list of known BHC versions.
pub fn known_versions() -> Vec<&'static str> {
    KNOWN_BHC_VERSIONS.to_vec()
}

/// Installed BHC information.
#[derive(Debug, Clone)]
pub struct InstalledBhc {
    /// Version string.
    pub version: String,
    /// Path to the BHC executable.
    pub path: PathBuf,
    /// Whether this is the active version.
    pub active: bool,
}

/// Detect BHC in the system PATH.
pub fn detect() -> Option<InstalledBhc> {
    let bhc_path = which::which("bhc").ok()?;

    // Get version
    let output = Command::new(&bhc_path).arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let version_str = String::from_utf8_lossy(&output.stdout);
    let version = parse_version_output(&version_str)?;

    Some(InstalledBhc {
        version,
        path: bhc_path,
        active: true,
    })
}

/// Parse BHC version output to extract version string.
fn parse_version_output(output: &str) -> Option<String> {
    // Try to find version pattern in output
    // Expected format: "BHC version X.Y.Z" or "bhc X.Y.Z"
    for word in output.split_whitespace() {
        if word.chars().next().is_some_and(|c| c.is_ascii_digit())
            && BhcVersion::parse(word).is_ok()
        {
            return Some(word.to_string());
        }
    }

    // Try splitting by "version"
    if let Some(rest) = output.split("version").nth(1) {
        let version = rest.split_whitespace().next()?;
        if BhcVersion::parse(version).is_ok() {
            return Some(version.to_string());
        }
    }

    None
}

/// Get the default BHC installation directory.
pub fn bhc_install_dir() -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join(".bhc"))
        .unwrap_or_else(|| PathBuf::from(".bhc"))
}

/// Get the path where a specific BHC version would be installed.
pub fn version_install_path(version: &str) -> PathBuf {
    bhc_install_dir().join("versions").join(version)
}

/// List installed BHC versions.
pub fn list_installed() -> Vec<InstalledBhc> {
    let install_dir = bhc_install_dir().join("versions");

    if !install_dir.exists() {
        return vec![];
    }

    let active = detect().map(|b| b.version);

    std::fs::read_dir(&install_dir)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let version = e.file_name().to_string_lossy().to_string();
                    let bhc_exe = bhc_binary_in(&e.path());

                    if bhc_exe.exists() && is_valid_version(&version) {
                        Some(InstalledBhc {
                            version: version.clone(),
                            path: bhc_exe,
                            active: active.as_ref() == Some(&version),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Download URL for a BHC version and platform.
///
/// BHC releases live at `arcanist-sh/bhc`; each release tags `v<version>` and
/// attaches per-platform archives named `bhc-<platform>.tar.gz` (the version is
/// not repeated in the asset filename).
pub fn bhc_download_url(version: &str, platform: &str) -> String {
    format!(
        "https://github.com/arcanist-sh/bhc/releases/download/v{}/bhc-{}.tar.gz",
        version, platform
    )
}

/// Get the platform identifier for the current system.
pub fn current_platform() -> String {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "x86_64-linux".to_string();

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "aarch64-linux".to_string();

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "x86_64-darwin".to_string();

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "aarch64-darwin".to_string();

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "x86_64-windows".to_string();

    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    return format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
}

/// Set the active BHC version.
pub async fn set_active(version: &str) -> Result<()> {
    let install_path = version_install_path(version);
    let bhc_exe = bhc_binary_in(&install_path);

    if !bhc_exe.exists() {
        return Err(BhcError::NotFound);
    }

    let bin_dir = bhc_install_dir().join("bin");
    std::fs::create_dir_all(&bin_dir)?;

    // Symlink ~/.bhc/bin/bhc to the active version's binary. BHC locates its
    // runtime archives as a sibling `lib/` of the *invoked* path (it does not
    // resolve the symlink on macOS), so we must place a `lib/` symlink next to
    // the binary symlink as well — otherwise the activated `bhc` runs in a
    // degraded mode that cannot find its standard library.
    let link_path = if cfg!(windows) {
        bin_dir.join("bhc.exe")
    } else {
        bin_dir.join("bhc")
    };
    let lib_link = bin_dir.join("lib");
    let lib_target = install_path.join("lib");

    // Remove existing links if any.
    let _ = std::fs::remove_file(&link_path);
    let _ = std::fs::remove_file(&lib_link);

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&bhc_exe, &link_path)?;
        if lib_target.is_dir() {
            std::os::unix::fs::symlink(&lib_target, &lib_link)?;
        }
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(&bhc_exe, &link_path)?;
        if lib_target.is_dir() {
            let _ = std::os::windows::fs::symlink_dir(&lib_target, &lib_link);
        }
    }

    info!("Set BHC {} as active", version);
    Ok(())
}

/// Remove an installed BHC version.
pub async fn remove_bhc(version: &str) -> Result<()> {
    let install_path = version_install_path(version);

    if !install_path.exists() {
        return Err(BhcError::NotFound);
    }

    std::fs::remove_dir_all(&install_path)?;
    info!("Removed BHC {}", version);
    Ok(())
}

/// Options for installing BHC.
#[derive(Debug, Clone)]
pub struct BhcInstallOptions {
    /// BHC version to install.
    pub version: String,
    /// Whether to set as active after installation.
    pub set_active: bool,
    /// Force reinstall even if already installed.
    pub force: bool,
    /// HTTP timeout in seconds.
    pub timeout: u64,
}

impl BhcInstallOptions {
    /// Create new install options for a specific version.
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            set_active: false,
            force: false,
            timeout: 120,
        }
    }

    /// Set whether to make this the active version.
    pub fn with_set_active(mut self, set_active: bool) -> Self {
        self.set_active = set_active;
        self
    }

    /// Set whether to force reinstall.
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }
}

/// Result of a BHC installation.
#[derive(Debug)]
pub struct BhcInstallResult {
    /// The installed BHC entry.
    pub installed: InstalledBhc,
    /// Whether this was a fresh install or already existed.
    pub was_cached: bool,
}

/// Download and install BHC.
pub async fn install_bhc(options: &BhcInstallOptions) -> Result<BhcInstallResult> {
    // Validate version
    if !is_known_version(&options.version) {
        return Err(BhcError::UnknownVersion(options.version.clone()));
    }

    let install_path = version_install_path(&options.version);

    // Check if already installed
    if !options.force && install_path.exists() {
        let bhc_exe = bhc_binary_in(&install_path);

        if bhc_exe.exists() {
            info!("BHC {} is already installed", options.version);
            return Ok(BhcInstallResult {
                installed: InstalledBhc {
                    version: options.version.clone(),
                    path: bhc_exe,
                    active: false,
                },
                was_cached: true,
            });
        }
    }

    info!(
        "Installing BHC {} for {}",
        options.version,
        current_platform()
    );

    // Create directories
    let base_dir = bhc_install_dir();
    let downloads_dir = base_dir.join("downloads");
    fs::create_dir_all(&downloads_dir).map_err(BhcError::Io)?;
    fs::create_dir_all(&install_path).map_err(BhcError::Io)?;

    // Download the archive. The asset filename matches the release (no version
    // in the name) so it lines up with the published SHA256SUMS manifest.
    let platform = current_platform();
    let archive_name = format!("bhc-{}.tar.gz", platform);
    let archive_path = downloads_dir.join(&archive_name);
    let url = bhc_download_url(&options.version, &platform);

    download_bhc_archive(&url, &archive_path, &options.version, options.timeout).await?;

    // Extract the archive
    extract_bhc_archive(&archive_path, &install_path, &options.version)?;

    // Verify installation
    verify_bhc_installation(&install_path, &options.version).await?;

    let bhc_exe = bhc_binary_in(&install_path);

    let installed = InstalledBhc {
        version: options.version.clone(),
        path: bhc_exe,
        active: false,
    };

    // Set as active if requested
    if options.set_active {
        set_active(&options.version).await?;
    }

    info!("BHC {} installed successfully", options.version);

    Ok(BhcInstallResult {
        installed,
        was_cached: false,
    })
}

/// Download a BHC archive with progress display.
///
/// The archive's SHA-256 is verified against the checksum manifest published
/// alongside the release; previously downloaded archives are re-verified
/// before being reused.
async fn download_bhc_archive(url: &str, dest: &Path, version: &str, timeout: u64) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(timeout))
        .build()
        .map_err(|e| BhcError::Network(format!("Failed to create HTTP client: {}", e)))?;

    let label = format!("BHC {}", version);
    let archive_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let expected = crate::verify::fetch_expected_sha256(&client, url, archive_name)
        .await
        .map_err(|e| BhcError::Network(e.to_string()))?;
    if expected.is_none() {
        crate::verify::require_checksum_or_opt_out(&label, url)
            .map_err(|e| BhcError::Download(e.to_string()))?;
    }

    // Reuse a previously downloaded archive only if it passes verification
    if dest.exists() {
        match &expected {
            Some(hash) => {
                if crate::verify::verify_file_sha256(dest, hash, &label).is_ok() {
                    debug!("Reusing verified archive: {}", dest.display());
                    return Ok(());
                }
                warn!(
                    "Cached archive failed checksum verification, re-downloading: {}",
                    dest.display()
                );
                fs::remove_file(dest).map_err(BhcError::Io)?;
            }
            None => {
                debug!(
                    "Archive already downloaded (unverified): {}",
                    dest.display()
                );
                return Ok(());
            }
        }
    }

    let spinner = Spinner::new(format!("Downloading BHC {}...", version));

    debug!("Downloading from {}", url);
    let response =
        client.get(url).send().await.map_err(|e| {
            BhcError::Download(format!("Failed to download BHC {}: {}", version, e))
        })?;

    if !response.status().is_success() {
        spinner.finish_error(format!("Failed to download BHC {}", version));
        return Err(BhcError::Download(format!(
            "BHC {} download failed: HTTP {}. This version may not be available for your platform.",
            version,
            response.status()
        )));
    }

    let total_size = response.content_length().unwrap_or(0);
    spinner.finish_clear();

    // Show progress bar if we know the size
    let progress = if total_size > 0 {
        Some(Progress::new(
            total_size,
            format!("Downloading BHC {}", version),
        ))
    } else {
        None
    };

    // Download to temp file first
    let temp_path = dest.with_extension("tmp");
    let mut file = File::create(&temp_path).map_err(BhcError::Io)?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| BhcError::Download(format!("Download interrupted: {}", e)))?;
        file.write_all(&chunk).map_err(BhcError::Io)?;
        downloaded += chunk.len() as u64;
        if let Some(ref pb) = progress {
            pb.set_position(downloaded);
        }
    }

    if let Some(pb) = progress {
        pb.finish(format!(
            "Downloaded BHC {} ({:.1} MB)",
            version,
            downloaded as f64 / 1_000_000.0
        ));
    }

    // Verify before moving into place
    if let Some(hash) = &expected
        && let Err(e) = crate::verify::verify_file_sha256(&temp_path, hash, &label)
    {
        let _ = fs::remove_file(&temp_path);
        return Err(BhcError::Download(e.to_string()));
    }

    // Rename temp file to final location
    fs::rename(&temp_path, dest).map_err(BhcError::Io)?;

    Ok(())
}

/// Extract a tar.gz archive.
fn extract_bhc_archive(archive_path: &Path, dest_dir: &Path, version: &str) -> Result<()> {
    use flate2::read::GzDecoder;

    let spinner = Spinner::new(format!("Extracting BHC {}...", version));

    debug!(
        "Extracting {} to {}",
        archive_path.display(),
        dest_dir.display()
    );

    // First pass: determine whether every entry sits under a single top-level
    // directory (older layout, e.g. "bhc-0.2.0-aarch64-darwin/…"). If so, strip
    // it; the current release tarballs place `bhc` and `lib/` at the top level
    // with no wrapper, and stripping unconditionally would drop the binary.
    let strip_prefix = archive_has_single_root(archive_path)?;

    let file = File::open(archive_path).map_err(BhcError::Io)?;
    let reader = BufReader::new(file);
    let decoder = GzDecoder::new(reader);
    let mut archive = Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| BhcError::Install(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| BhcError::Install(e.to_string()))?;
        let path = entry.path().map_err(|e| BhcError::Install(e.to_string()))?;

        let stripped: PathBuf = if strip_prefix {
            path.components().skip(1).collect()
        } else {
            path.to_path_buf()
        };
        if stripped.as_os_str().is_empty() {
            continue;
        }

        // Per-entry unpack does not validate paths like Archive::unpack does;
        // reject anything that could escape the destination directory.
        if !stripped
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return Err(BhcError::Install(format!(
                "Refusing to extract archive entry with unsafe path: {}",
                path.display()
            )));
        }

        let dest_path = dest_dir.join(&stripped);

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(BhcError::Io)?;
        }

        entry.unpack(&dest_path).map_err(|e| {
            BhcError::Install(format!("Failed to extract {}: {}", stripped.display(), e))
        })?;
    }

    // Keep BHC's native flat layout (`<install>/bhc` + `<install>/lib/`): the
    // compiler locates its runtime libraries as a *sibling* `lib/` of the
    // binary, so the executable must NOT be moved into a `bin/` subdirectory.
    spinner.finish_success(format!("Extracted BHC {}", version));
    Ok(())
}

/// Resolve the BHC executable within an install directory.
///
/// Release tarballs place the binary at `<dir>/bhc` (flat, with a sibling
/// `lib/`). Older/managed installs may use `<dir>/bin/bhc`; both are accepted,
/// flat first.
pub fn bhc_binary_in(dir: &Path) -> PathBuf {
    let exe = if cfg!(windows) { "bhc.exe" } else { "bhc" };
    let flat = dir.join(exe);
    if flat.exists() {
        return flat;
    }
    let in_bin = dir.join("bin").join(exe);
    if in_bin.exists() {
        return in_bin;
    }
    flat
}

/// Whether every entry in the archive lives under one common top-level
/// directory (so that directory should be stripped on extraction).
fn archive_has_single_root(archive_path: &Path) -> Result<bool> {
    use flate2::read::GzDecoder;

    let file = File::open(archive_path).map_err(BhcError::Io)?;
    let decoder = GzDecoder::new(BufReader::new(file));
    let mut archive = Archive::new(decoder);

    let mut root: Option<std::ffi::OsString> = None;
    for entry in archive
        .entries()
        .map_err(|e| BhcError::Install(e.to_string()))?
    {
        let entry = entry.map_err(|e| BhcError::Install(e.to_string()))?;
        let path = entry.path().map_err(|e| BhcError::Install(e.to_string()))?;
        match path.components().next() {
            Some(std::path::Component::Normal(first)) => match &root {
                Some(r) if r != first => return Ok(false),
                Some(_) => {}
                None => root = Some(first.to_os_string()),
            },
            _ => return Ok(false),
        }
    }
    Ok(root.is_some())
}

/// Verify BHC installation by running bhc --version.
async fn verify_bhc_installation(install_dir: &Path, expected_version: &str) -> Result<()> {
    let bhc_path = bhc_binary_in(install_dir);

    if !bhc_path.exists() {
        return Err(BhcError::Install(format!(
            "BHC binary not found at {}. Installation may have failed.",
            bhc_path.display()
        )));
    }

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bhc_path).map_err(BhcError::Io)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bhc_path, perms).map_err(BhcError::Io)?;
    }

    let output = Command::new(&bhc_path)
        .arg("--version")
        .output()
        .map_err(BhcError::Io)?;

    if !output.status.success() {
        return Err(BhcError::Install(
            "BHC verification failed: could not get version".into(),
        ));
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    debug!(
        "Verified BHC {} at {} (output: {})",
        expected_version,
        bhc_path.display(),
        version_output.trim()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parse() {
        let v = BhcVersion::parse("0.1.0").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);
        assert_eq!(v.to_string(), "0.1.0");
    }

    #[test]
    fn test_version_parse_invalid() {
        assert!(BhcVersion::parse("0.1").is_err());
        assert!(BhcVersion::parse("abc").is_err());
        assert!(BhcVersion::parse("0.1.0.0").is_err());
    }

    #[test]
    fn test_known_versions() {
        assert!(is_known_version("0.2.3"));
        assert!(is_known_version("0.2.0"));
        assert!(!is_known_version("0.1.0"));
        assert!(!is_known_version("99.99.99"));
    }

    #[test]
    fn test_download_url_format() {
        // arcanist-sh/bhc; version in the tag, NOT in the asset filename.
        assert_eq!(
            bhc_download_url("0.2.3", "x86_64-linux"),
            "https://github.com/arcanist-sh/bhc/releases/download/v0.2.3/bhc-x86_64-linux.tar.gz"
        );
    }

    #[test]
    fn test_parse_version_output() {
        assert_eq!(
            parse_version_output("BHC version 0.1.0"),
            Some("0.1.0".to_string())
        );
        assert_eq!(parse_version_output("bhc 0.2.0"), Some("0.2.0".to_string()));
    }
}
