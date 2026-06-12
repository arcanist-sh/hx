//! Cabal binary download and installation.
//!
//! Downloads Cabal binaries directly from downloads.haskell.org and extracts them
//! to the hx toolchain directory. Unlike GHC, Cabal binaries are pre-built and
//! don't require configure/make.

use crate::cabal::{cabal_archive_filename, cabal_download_url, is_valid_version};
use crate::ghc::{InstallSource, InstalledCabal, Platform, ToolchainManifest};
use crate::verify;
use futures_util::StreamExt;
use hx_core::{CommandRunner, Error, Result};
use hx_ui::{Progress, Spinner};
use reqwest::Client;
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Options for downloading and installing Cabal.
#[derive(Debug, Clone)]
pub struct CabalDownloadOptions {
    /// Cabal version to install.
    pub version: String,
    /// Target platform (auto-detected if None).
    pub platform: Option<Platform>,
    /// Base toolchain directory.
    pub toolchain_dir: PathBuf,
    /// Whether to set as active after installation.
    pub set_active: bool,
    /// Force reinstall even if already installed.
    pub force: bool,
    /// HTTP timeout in seconds.
    pub timeout: u64,
}

impl Default for CabalDownloadOptions {
    fn default() -> Self {
        Self {
            version: String::new(),
            platform: None,
            toolchain_dir: PathBuf::new(),
            timeout: 120, // 2 minutes - Cabal downloads are smaller than GHC
            set_active: false,
            force: false,
        }
    }
}

/// Result of a Cabal installation.
#[derive(Debug)]
pub struct CabalInstallResult {
    /// The installed Cabal entry.
    pub installed: InstalledCabal,
    /// Whether this was a fresh install or already existed.
    pub was_cached: bool,
}

/// Download and install Cabal.
pub async fn download_and_install_cabal(
    options: &CabalDownloadOptions,
) -> Result<CabalInstallResult> {
    // Validate version format
    if !is_valid_version(&options.version) {
        return Err(Error::config(format!(
            "Invalid Cabal version format: {}. Expected format like 3.12.1.0",
            options.version
        )));
    }

    // Detect platform
    let platform = options
        .platform
        .unwrap_or_else(|| Platform::current().expect("Unsupported platform"));

    // Check if already installed
    let manifest = ToolchainManifest::load(&options.toolchain_dir)?;
    if !options.force && manifest.is_cabal_installed(&options.version) {
        info!("Cabal {} is already installed", options.version);
        let installed = manifest.get_cabal(&options.version).unwrap().clone();
        return Ok(CabalInstallResult {
            installed,
            was_cached: true,
        });
    }

    info!(
        "Installing Cabal {} for {}",
        options.version,
        platform.display_name()
    );

    // Create directories
    let cabal_dir = options.toolchain_dir.join("cabal");
    let install_dir = cabal_dir.join(&options.version);
    let download_dir = options.toolchain_dir.join("downloads");
    fs::create_dir_all(&download_dir).map_err(|e| Error::Io {
        message: "Failed to create downloads directory".into(),
        path: Some(download_dir.clone()),
        source: e,
    })?;

    // Download the archive
    let archive_name = cabal_archive_filename(&options.version, platform);
    let archive_path = download_dir.join(&archive_name);
    let url = cabal_download_url(&options.version, platform);

    download_cabal_archive(&url, &archive_path, &options.version, options.timeout).await?;

    // Extract the archive
    extract_cabal_archive(&archive_path, &install_dir, &options.version, platform)?;

    // Verify installation
    verify_cabal_installation(&install_dir, &options.version).await?;

    // Update manifest
    let installed =
        InstalledCabal::new(&options.version, &install_dir).with_source(InstallSource::Direct);

    let mut manifest = ToolchainManifest::load(&options.toolchain_dir)?;
    manifest.add_cabal(installed.clone());

    if options.set_active {
        manifest.set_active_cabal(&options.version)?;
    }

    manifest.save(&options.toolchain_dir)?;

    info!("Cabal {} installed successfully", options.version);

    Ok(CabalInstallResult {
        installed,
        was_cached: false,
    })
}

/// Download a Cabal archive with progress display.
///
/// The archive's SHA-256 is verified against the `SHA256SUMS` manifest
/// published alongside it; previously downloaded archives are re-verified
/// before being reused.
async fn download_cabal_archive(url: &str, dest: &Path, version: &str, timeout: u64) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(timeout))
        .build()
        .map_err(|e| Error::config(format!("Failed to create HTTP client: {}", e)))?;

    let label = format!("Cabal {}", version);
    let archive_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let expected = verify::fetch_expected_sha256(&client, url, archive_name).await?;
    if expected.is_none() {
        verify::require_checksum_or_opt_out(&label, url)?;
    }

    // Reuse a previously downloaded archive only if it passes verification
    if dest.exists() {
        match &expected {
            Some(hash) => {
                if verify::verify_file_sha256(dest, hash, &label).is_ok() {
                    debug!("Reusing verified archive: {}", dest.display());
                    return Ok(());
                }
                warn!(
                    "Cached archive failed checksum verification, re-downloading: {}",
                    dest.display()
                );
                fs::remove_file(dest).map_err(|e| Error::Io {
                    message: "Failed to remove corrupted cached archive".into(),
                    path: Some(dest.to_path_buf()),
                    source: e,
                })?;
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

    let spinner = Spinner::new(format!("Downloading Cabal {}...", version));

    debug!("Downloading from {}", url);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::config(format!("Failed to download Cabal {}: {}", version, e)))?;

    if !response.status().is_success() {
        spinner.finish_error(format!("Failed to download Cabal {}", version));
        return Err(Error::config(format!(
            "Cabal {} download failed: HTTP {}. This version may not be available for your platform.",
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
            format!("Downloading Cabal {}", version),
        ))
    } else {
        None
    };

    // Download to temp file first
    let temp_path = dest.with_extension("tmp");
    let mut file = File::create(&temp_path).map_err(|e| Error::Io {
        message: "Failed to create download file".into(),
        path: Some(temp_path.clone()),
        source: e,
    })?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::config(format!("Download interrupted: {}", e)))?;
        file.write_all(&chunk).map_err(|e| Error::Io {
            message: "Failed to write download data".into(),
            path: Some(temp_path.clone()),
            source: e,
        })?;
        downloaded += chunk.len() as u64;
        if let Some(ref pb) = progress {
            pb.set_position(downloaded);
        }
    }

    if let Some(pb) = progress {
        pb.finish(format!(
            "Downloaded Cabal {} ({:.1} MB)",
            version,
            downloaded as f64 / 1_000_000.0
        ));
    }

    // Verify before moving into place
    if let Some(hash) = &expected
        && let Err(e) = verify::verify_file_sha256(&temp_path, hash, &label)
    {
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }

    // Rename temp file to final location
    fs::rename(&temp_path, dest).map_err(|e| Error::Io {
        message: "Failed to finalize download".into(),
        path: Some(dest.to_path_buf()),
        source: e,
    })?;

    Ok(())
}

/// Extract a Cabal archive.
fn extract_cabal_archive(
    archive_path: &Path,
    install_dir: &Path,
    version: &str,
    platform: Platform,
) -> Result<()> {
    let spinner = Spinner::new(format!("Extracting Cabal {}...", version));

    debug!(
        "Extracting {} to {}",
        archive_path.display(),
        install_dir.display()
    );

    // Create install directory with bin subdirectory
    let bin_dir = install_dir.join("bin");
    fs::create_dir_all(&bin_dir).map_err(|e| Error::Io {
        message: "Failed to create install directory".into(),
        path: Some(bin_dir.clone()),
        source: e,
    })?;

    if matches!(platform, Platform::X86_64Windows) {
        // Windows uses .zip
        extract_zip(archive_path, &bin_dir)?;
    } else {
        // Unix uses .tar.xz
        extract_tar_xz(archive_path, &bin_dir)?;
    }

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let cabal_path = bin_dir.join("cabal");
        if cabal_path.exists() {
            fs::set_permissions(&cabal_path, fs::Permissions::from_mode(0o755)).map_err(|e| {
                Error::Io {
                    message: "Failed to set executable permissions".into(),
                    path: Some(cabal_path),
                    source: e,
                }
            })?;
        }
    }

    spinner.finish_success(format!("Extracted Cabal {}", version));
    Ok(())
}

/// Extract a tar.xz archive.
fn extract_tar_xz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    use tar::Archive;
    use xz2::read::XzDecoder;

    let file = File::open(archive_path).map_err(|e| Error::Io {
        message: "Failed to open archive".into(),
        path: Some(archive_path.to_path_buf()),
        source: e,
    })?;

    let reader = BufReader::new(file);
    let decoder = XzDecoder::new(reader);
    let mut archive = Archive::new(decoder);

    // Cabal archives contain just the binary, extract directly to bin dir
    archive.unpack(dest_dir).map_err(|e| Error::Io {
        message: format!("Failed to extract archive: {}", e),
        path: Some(archive_path.to_path_buf()),
        source: e,
    })?;

    Ok(())
}

/// Extract a zip archive.
fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = File::open(archive_path).map_err(|e| Error::Io {
        message: "Failed to open archive".into(),
        path: Some(archive_path.to_path_buf()),
        source: e,
    })?;

    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| Error::config(format!("Failed to read zip archive: {}", e)))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| Error::config(format!("Failed to read zip entry: {}", e)))?;

        // enclosed_name() rejects entries that would escape the destination
        // (absolute paths or ".." components)
        let Some(safe_name) = file.enclosed_name() else {
            return Err(Error::config(format!(
                "Refusing to extract zip entry with unsafe path: {}",
                file.name()
            )));
        };
        let outpath = dest_dir.join(safe_name);

        if file.is_dir() {
            fs::create_dir_all(&outpath).map_err(|e| Error::Io {
                message: "Failed to create directory".into(),
                path: Some(outpath.clone()),
                source: e,
            })?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::Io {
                    message: "Failed to create parent directory".into(),
                    path: Some(parent.to_path_buf()),
                    source: e,
                })?;
            }
            let mut outfile = File::create(&outpath).map_err(|e| Error::Io {
                message: "Failed to create file".into(),
                path: Some(outpath.clone()),
                source: e,
            })?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| Error::Io {
                message: "Failed to write file".into(),
                path: Some(outpath),
                source: e,
            })?;
        }
    }

    Ok(())
}

/// Verify Cabal installation by running cabal --version.
async fn verify_cabal_installation(install_dir: &Path, expected_version: &str) -> Result<()> {
    let cabal_path = install_dir.join("bin").join(cabal_binary_name());

    if !cabal_path.exists() {
        return Err(Error::config(format!(
            "Cabal binary not found at {}. Installation may have failed.",
            cabal_path.display()
        )));
    }

    let runner = CommandRunner::new();
    let output = runner
        .run(cabal_path.to_str().unwrap(), ["--numeric-version"])
        .await?;

    if !output.success() {
        return Err(Error::config(
            "Cabal verification failed: could not get version",
        ));
    }

    let installed_version = output.stdout.trim();
    debug!(
        "Verified Cabal {} at {}",
        installed_version,
        cabal_path.display()
    );

    // Cabal --numeric-version returns just the version without revision
    // e.g., "3.12.1.0" -> "3.12.1"
    // So we compare the major.minor.patch parts
    let expected_parts: Vec<&str> = expected_version.split('.').take(3).collect();
    let installed_parts: Vec<&str> = installed_version.split('.').take(3).collect();

    if expected_parts != installed_parts {
        return Err(Error::config(format!(
            "Version mismatch: expected {}, got {}",
            expected_version, installed_version
        )));
    }

    Ok(())
}

/// Get the Cabal binary name for the current platform.
fn cabal_binary_name() -> &'static str {
    if cfg!(windows) { "cabal.exe" } else { "cabal" }
}

/// Remove an installed Cabal version.
pub fn remove_cabal(toolchain_dir: &Path, version: &str) -> Result<bool> {
    let mut manifest = ToolchainManifest::load(toolchain_dir)?;

    if let Some(installed) = manifest.remove_cabal(version) {
        // Remove the installation directory
        if installed.install_path.exists() {
            fs::remove_dir_all(&installed.install_path).map_err(|e| Error::Io {
                message: format!("Failed to remove Cabal {}", version),
                path: Some(installed.install_path.clone()),
                source: e,
            })?;
        }

        manifest.save(toolchain_dir)?;
        info!("Removed Cabal {}", version);
        Ok(true)
    } else {
        Ok(false)
    }
}

/// List installed Cabal versions.
pub fn list_installed(toolchain_dir: &Path) -> Result<Vec<InstalledCabal>> {
    let manifest = ToolchainManifest::load(toolchain_dir)?;
    Ok(manifest.cabal)
}

/// Get the active Cabal version.
pub fn get_active_cabal(toolchain_dir: &Path) -> Result<Option<InstalledCabal>> {
    let manifest = ToolchainManifest::load(toolchain_dir)?;
    Ok(manifest.active_cabal().cloned())
}

/// Set the active Cabal version.
pub fn set_active_cabal(toolchain_dir: &Path, version: &str) -> Result<()> {
    let mut manifest = ToolchainManifest::load(toolchain_dir)?;
    manifest.set_active_cabal(version)?;
    manifest.save(toolchain_dir)?;
    info!("Set Cabal {} as active", version);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_options_default() {
        let options = CabalDownloadOptions::default();
        assert_eq!(options.timeout, 120);
        assert!(!options.set_active);
        assert!(!options.force);
    }

    #[test]
    fn test_cabal_binary_name() {
        let name = cabal_binary_name();
        if cfg!(windows) {
            assert_eq!(name, "cabal.exe");
        } else {
            assert_eq!(name, "cabal");
        }
    }

    #[test]
    fn test_installed_cabal_paths() {
        let cabal = InstalledCabal::new("3.12.1.0", "/home/user/.hx/toolchains/cabal/3.12.1.0");

        let bin_dir = cabal.bin_dir();
        assert!(bin_dir.ends_with("bin"));

        let cabal_path = cabal.cabal_path();
        assert!(cabal_path.ends_with("cabal") || cabal_path.ends_with("cabal.exe"));
    }
}
