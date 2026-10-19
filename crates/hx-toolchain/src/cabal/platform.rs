//! Platform detection for Cabal binary downloads.

use crate::ghc::Platform;

/// Get the Cabal platform suffix for download URLs.
///
/// Cabal binaries use slightly different naming than GHC, and the scheme
/// changed in 3.18: earlier releases name the build distribution
/// (`cabal-install-3.16.1.0-x86_64-linux-ubuntu20_04.tar.xz`), 3.18 and later
/// publish a single build per target (`cabal-install-3.18.2.0-x86_64-linux-unknown.tar.xz`).
pub fn cabal_platform_suffix(version: &str, platform: Platform) -> &'static str {
    if uses_target_triple_names(version) {
        return match platform {
            Platform::X86_64Linux => "x86_64-linux-unknown",
            Platform::Aarch64Linux => "aarch64-linux-unknown",
            Platform::X86_64Darwin => "x86_64-apple-darwin",
            Platform::Aarch64Darwin => "aarch64-apple-darwin",
            Platform::X86_64Windows => "x86_64-mingw64",
        };
    }
    match platform {
        // Linux uses distribution-specific builds, we use the most compatible ones
        Platform::X86_64Linux => "x86_64-linux-ubuntu20_04",
        Platform::Aarch64Linux => "aarch64-linux-deb11",
        // macOS
        Platform::X86_64Darwin => "x86_64-darwin",
        Platform::Aarch64Darwin => "aarch64-darwin",
        // Windows
        Platform::X86_64Windows => "x86_64-windows",
    }
}

/// Whether this Cabal release uses the per-target asset names introduced in 3.18.
fn uses_target_triple_names(version: &str) -> bool {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    (major, minor) >= (3, 18)
}

/// Get the archive extension for Cabal binaries on this platform.
pub fn cabal_archive_extension(platform: Platform) -> &'static str {
    match platform {
        Platform::X86_64Windows => "zip",
        _ => "xz", // tar.xz for Unix platforms
    }
}

/// Construct the Cabal download URL for a version and platform.
///
/// URL pattern: `https://downloads.haskell.org/cabal/cabal-install-{version}/cabal-install-{version}-{platform}.tar.{ext}`
pub fn cabal_download_url(version: &str, platform: Platform) -> String {
    let suffix = cabal_platform_suffix(version, platform);
    let ext = cabal_archive_extension(platform);

    // Windows uses .zip, others use .tar.xz
    if matches!(platform, Platform::X86_64Windows) {
        format!(
            "https://downloads.haskell.org/cabal/cabal-install-{version}/cabal-install-{version}-{suffix}.{ext}",
            version = version,
            suffix = suffix,
            ext = ext
        )
    } else {
        format!(
            "https://downloads.haskell.org/cabal/cabal-install-{version}/cabal-install-{version}-{suffix}.tar.{ext}",
            version = version,
            suffix = suffix,
            ext = ext
        )
    }
}

/// Construct the expected archive filename.
pub fn cabal_archive_filename(version: &str, platform: Platform) -> String {
    let suffix = cabal_platform_suffix(version, platform);
    let ext = cabal_archive_extension(platform);

    if matches!(platform, Platform::X86_64Windows) {
        format!(
            "cabal-install-{version}-{suffix}.{ext}",
            version = version,
            suffix = suffix,
            ext = ext
        )
    } else {
        format!(
            "cabal-install-{version}-{suffix}.tar.{ext}",
            version = version,
            suffix = suffix,
            ext = ext
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cabal_platform_suffix() {
        assert_eq!(
            cabal_platform_suffix("3.16.1.0", Platform::X86_64Linux),
            "x86_64-linux-ubuntu20_04"
        );
        assert_eq!(
            cabal_platform_suffix("3.16.1.0", Platform::Aarch64Darwin),
            "aarch64-darwin"
        );
        assert_eq!(
            cabal_platform_suffix("3.16.1.0", Platform::X86_64Windows),
            "x86_64-windows"
        );
    }

    #[test]
    fn test_cabal_platform_suffix_3_18() {
        assert_eq!(
            cabal_platform_suffix("3.18.2.0", Platform::X86_64Linux),
            "x86_64-linux-unknown"
        );
        assert_eq!(
            cabal_platform_suffix("3.18.2.0", Platform::Aarch64Linux),
            "aarch64-linux-unknown"
        );
        assert_eq!(
            cabal_platform_suffix("3.18.1.0", Platform::Aarch64Darwin),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            cabal_download_url("3.18.2.0", Platform::X86_64Windows),
            "https://downloads.haskell.org/cabal/cabal-install-3.18.2.0/cabal-install-3.18.2.0-x86_64-mingw64.zip"
        );
        assert_eq!(
            cabal_platform_suffix("3.20.0.0", Platform::X86_64Darwin),
            "x86_64-apple-darwin"
        );
    }

    #[test]
    fn test_cabal_archive_extension() {
        assert_eq!(cabal_archive_extension(Platform::X86_64Linux), "xz");
        assert_eq!(cabal_archive_extension(Platform::X86_64Darwin), "xz");
        assert_eq!(cabal_archive_extension(Platform::X86_64Windows), "zip");
    }

    #[test]
    fn test_cabal_download_url() {
        let url = cabal_download_url("3.12.1.0", Platform::X86_64Darwin);
        assert_eq!(
            url,
            "https://downloads.haskell.org/cabal/cabal-install-3.12.1.0/cabal-install-3.12.1.0-x86_64-darwin.tar.xz"
        );

        let url = cabal_download_url("3.12.1.0", Platform::Aarch64Linux);
        assert_eq!(
            url,
            "https://downloads.haskell.org/cabal/cabal-install-3.12.1.0/cabal-install-3.12.1.0-aarch64-linux-deb11.tar.xz"
        );

        let url = cabal_download_url("3.12.1.0", Platform::X86_64Windows);
        assert_eq!(
            url,
            "https://downloads.haskell.org/cabal/cabal-install-3.12.1.0/cabal-install-3.12.1.0-x86_64-windows.zip"
        );
    }

    #[test]
    fn test_cabal_archive_filename() {
        let name = cabal_archive_filename("3.12.1.0", Platform::X86_64Darwin);
        assert_eq!(name, "cabal-install-3.12.1.0-x86_64-darwin.tar.xz");

        let name = cabal_archive_filename("3.12.1.0", Platform::X86_64Windows);
        assert_eq!(name, "cabal-install-3.12.1.0-x86_64-windows.zip");
    }
}
