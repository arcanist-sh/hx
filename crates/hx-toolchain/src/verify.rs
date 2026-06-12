//! Download integrity verification.
//!
//! Verifies SHA-256 checksums of downloaded toolchain archives against the
//! checksum manifests published alongside the archives (e.g. `SHA256SUMS` on
//! downloads.haskell.org, or `<file>.sha256` release assets).

use hx_core::{Error, Result};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tracing::{debug, warn};

/// Escape hatch: allow installs when the publisher provides no checksum.
pub const ALLOW_UNVERIFIED_ENV: &str = "HX_ALLOW_UNVERIFIED_DOWNLOADS";

/// Whether the user explicitly allowed unverified downloads.
pub fn unverified_allowed() -> bool {
    std::env::var(ALLOW_UNVERIFIED_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// URL of the `SHA256SUMS` manifest in the same directory as `download_url`.
pub fn sums_url_for(download_url: &str) -> Option<String> {
    download_url
        .rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/SHA256SUMS"))
}

/// Parse a checksum manifest line: `<hex>  <filename>` (binary-mode `*` and
/// `./` prefixes tolerated). Returns the digest if the line matches `file_name`.
fn digest_for_line(line: &str, file_name: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    let hash = parts.next()?;
    let name = parts.next().unwrap_or(file_name);
    let name = name.trim_start_matches('*').trim_start_matches("./");
    if name == file_name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hash.to_ascii_lowercase())
    } else {
        None
    }
}

async fn fetch_manifest(client: &Client, url: &str) -> Result<Option<String>> {
    debug!("Fetching checksum manifest from {}", url);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::config(format!("Failed to fetch checksum manifest {url}: {e}")))?;
    if !response.status().is_success() {
        debug!(
            "No checksum manifest at {} (HTTP {})",
            url,
            response.status()
        );
        return Ok(None);
    }
    let body = response
        .text()
        .await
        .map_err(|e| Error::config(format!("Failed to read checksum manifest {url}: {e}")))?;
    Ok(Some(body))
}

/// Look up the expected SHA-256 for `file_name`, trying the `SHA256SUMS`
/// manifest next to `download_url`, then a `<download_url>.sha256` sidecar.
pub async fn fetch_expected_sha256(
    client: &Client,
    download_url: &str,
    file_name: &str,
) -> Result<Option<String>> {
    if let Some(sums_url) = sums_url_for(download_url)
        && let Some(body) = fetch_manifest(client, &sums_url).await?
        && let Some(digest) = body.lines().find_map(|l| digest_for_line(l, file_name))
    {
        return Ok(Some(digest));
    }

    let sidecar_url = format!("{download_url}.sha256");
    if let Some(body) = fetch_manifest(client, &sidecar_url).await?
        && let Some(digest) = body.lines().find_map(|l| digest_for_line(l, file_name))
    {
        return Ok(Some(digest));
    }

    Ok(None)
}

/// Compute the SHA-256 of a file (streaming).
pub fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| Error::Io {
        message: "Failed to open file for checksum verification".into(),
        path: Some(path.to_path_buf()),
        source: e,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| Error::Io {
            message: "Failed to read file for checksum verification".into(),
            path: Some(path.to_path_buf()),
            source: e,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verify a file against an expected SHA-256 digest.
pub fn verify_file_sha256(path: &Path, expected: &str, label: &str) -> Result<()> {
    let actual = sha256_of_file(path)?;
    if actual != expected.to_ascii_lowercase() {
        return Err(Error::config(format!(
            "Checksum mismatch for {label} ({path})\n  expected: {expected}\n  actual:   {actual}\n\nThe download may be corrupted or tampered with.\nfix: Delete the file and run the install again.",
            path = path.display(),
        )));
    }
    debug!("Verified SHA-256 of {}", path.display());
    Ok(())
}

/// Handle a missing published checksum: error unless the user opted out.
pub fn require_checksum_or_opt_out(label: &str, url: &str) -> Result<()> {
    if unverified_allowed() {
        warn!(
            "No published checksum found for {} ({}); proceeding unverified because {} is set",
            label, url, ALLOW_UNVERIFIED_ENV
        );
        return Ok(());
    }
    Err(Error::config(format!(
        "No published SHA-256 checksum found for {label}\n  url: {url}\n\nRefusing to install an unverified download.\nfix: Retry later, or set {ALLOW_UNVERIFIED_ENV}=1 to proceed without verification."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_sums_url_for() {
        assert_eq!(
            sums_url_for("https://example.com/dir/file.tar.xz").as_deref(),
            Some("https://example.com/dir/SHA256SUMS")
        );
    }

    #[test]
    fn test_digest_for_line() {
        let hash = "a".repeat(64);
        assert_eq!(
            digest_for_line(&format!("{hash}  ./file.tar.xz"), "file.tar.xz"),
            Some(hash.clone())
        );
        assert_eq!(
            digest_for_line(&format!("{hash} *file.tar.xz"), "file.tar.xz"),
            Some(hash.clone())
        );
        assert_eq!(
            digest_for_line(&format!("{hash}  other.tar.xz"), "file.tar.xz"),
            None
        );
        assert_eq!(
            digest_for_line("not-a-hash  file.tar.xz", "file.tar.xz"),
            None
        );
    }

    #[test]
    fn test_sha256_of_file_and_verify() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello").unwrap();
        let digest = sha256_of_file(f.path()).unwrap();
        assert_eq!(
            digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(verify_file_sha256(f.path(), &digest, "test").is_ok());
        assert!(verify_file_sha256(f.path(), &"0".repeat(64), "test").is_err());
    }
}
