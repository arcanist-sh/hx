//! Self-update functionality.

use anyhow::{Context, Result};
use hx_ui::Output;

const REPO_OWNER: &str = "raskell-io";
const REPO_NAME: &str = "hx";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Check for updates or upgrade to latest version.
pub async fn run(check_only: bool, target_version: Option<String>, output: &Output) -> Result<i32> {
    if check_only {
        check_for_updates(output).await
    } else {
        upgrade(target_version, output).await
    }
}

async fn check_for_updates(output: &Output) -> Result<i32> {
    output.status("Checking", "for updates...");

    let latest = get_latest_version().await?;

    if latest == CURRENT_VERSION {
        output.status(
            "Up to date",
            &format!("hx {} is the latest version", CURRENT_VERSION),
        );
        Ok(0)
    } else if is_newer(&latest, CURRENT_VERSION) {
        output.info(&format!(
            "Update available: {} -> {}",
            CURRENT_VERSION, latest
        ));
        output.info("Run `hx upgrade` to install");
        Ok(0)
    } else {
        output.status(
            "Up to date",
            &format!(
                "hx {} is newer than latest release {}",
                CURRENT_VERSION, latest
            ),
        );
        Ok(0)
    }
}

async fn upgrade(target_version: Option<String>, output: &Output) -> Result<i32> {
    let target = match target_version {
        Some(v) => v,
        None => {
            output.status("Checking", "for latest version...");
            get_latest_version().await?
        }
    };

    if target == CURRENT_VERSION {
        output.status(
            "Up to date",
            &format!("hx {} is already installed", CURRENT_VERSION),
        );
        return Ok(0);
    }

    output.status(
        "Upgrading",
        &format!("hx {} -> {}", CURRENT_VERSION, target),
    );

    // Download and verify the release archive against its published .sha256
    // before replacing the running binary (TLS alone is not integrity proof
    // of the release artifact).
    let triple = get_target();
    let ext = if triple.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    let archive_name = format!("hx-v{}-{}.{}", target, triple, ext);
    let url = format!(
        "https://github.com/{}/{}/releases/download/v{}/{}",
        REPO_OWNER, REPO_NAME, target, archive_name
    );

    let client = reqwest::Client::builder()
        .user_agent(format!("hx/{}", CURRENT_VERSION))
        .build()?;

    output.status("Downloading", &archive_name);
    let archive_bytes = client
        .get(&url)
        .send()
        .await
        .context("Failed to download release archive")?
        .error_for_status()
        .context("Release archive not found for this version/platform")?
        .bytes()
        .await
        .context("Failed to read release archive")?;

    output.status("Verifying", "SHA-256 checksum...");
    let checksum_body = client
        .get(format!("{}.sha256", url))
        .send()
        .await
        .context("Failed to download release checksum")?
        .error_for_status()
        .context("No .sha256 checksum published for this release; refusing unverified upgrade")?
        .text()
        .await
        .context("Failed to read release checksum")?;

    let expected = checksum_body
        .split_whitespace()
        .next()
        .context("Malformed .sha256 file")?
        .to_ascii_lowercase();

    let actual = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&archive_bytes))
    };

    if actual != expected {
        anyhow::bail!(
            "Checksum mismatch for {}\n  expected: {}\n  actual:   {}\n\nThe download may be corrupted or tampered with; aborting upgrade.",
            archive_name,
            expected,
            actual
        );
    }

    // Extract and replace the current executable
    let tmp_dir = tempfile::Builder::new()
        .prefix("hx-upgrade-")
        .tempdir()
        .context("Failed to create temp directory")?;
    let archive_path = tmp_dir.path().join(&archive_name);
    std::fs::write(&archive_path, &archive_bytes).context("Failed to write archive")?;

    self_update::Extract::from_source(&archive_path)
        .extract_into(tmp_dir.path())
        .context("Failed to extract release archive")?;

    let bin_name = if cfg!(windows) { "hx.exe" } else { "hx" };
    let new_exe =
        find_file(tmp_dir.path(), bin_name).context("hx binary not found in release archive")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_exe, std::fs::Permissions::from_mode(0o755))
            .context("Failed to set executable permissions")?;
    }

    let current_exe = std::env::current_exe().context("Failed to locate current executable")?;
    // The swap temp must live next to the destination so the rename is atomic
    let replace_tmp = current_exe.with_extension("replace.tmp");
    self_update::Move::from_source(&new_exe)
        .replace_using_temp(&replace_tmp)
        .to_dest(&current_exe)
        .context("Failed to replace current executable")?;

    output.status(
        "Upgraded",
        &format!("Successfully upgraded to hx {}", target),
    );

    Ok(0)
}

/// Recursively find a file by name.
fn find_file(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

async fn get_latest_version() -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        REPO_OWNER, REPO_NAME
    );

    let client = reqwest::Client::builder()
        .user_agent(format!("hx/{}", CURRENT_VERSION))
        .build()?;

    let response: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .context("Failed to fetch release info")?
        .json()
        .await
        .context("Failed to parse release info")?;

    let tag = response["tag_name"]
        .as_str()
        .context("No tag_name in release")?;

    // Strip leading 'v' if present
    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

fn is_newer(a: &str, b: &str) -> bool {
    // Simple semver comparison
    let parse = |s: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = s.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };

    parse(a) > parse(b)
}

fn get_target() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    match (arch, os) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu".to_string(),
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu".to_string(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_string(),
        ("aarch64", "macos") => "aarch64-apple-darwin".to_string(),
        ("x86_64", "windows") => "x86_64-pc-windows-msvc".to_string(),
        _ => format!("{}-unknown-{}", arch, os),
    }
}
