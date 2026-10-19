//! Host description used to pick a bindist from ghcup metadata.
//!
//! ghcup keys downloads by architecture (`A_64`, `A_ARM64`), operating system
//! (`Darwin`, `Windows`, `Linux_<Distro>`), and on Linux by a range over the
//! distribution's version. This module turns `/etc/os-release` and the C
//! library into that description.

use crate::ghc::Platform;
use std::collections::HashMap;
use std::path::Path;

/// CPU architecture, as ghcup names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    /// x86_64 (`A_64`).
    X86_64,
    /// aarch64 (`A_ARM64`).
    Aarch64,
}

impl Arch {
    /// The key ghcup uses for this architecture.
    pub fn key(self) -> &'static str {
        match self {
            Self::X86_64 => "A_64",
            Self::Aarch64 => "A_ARM64",
        }
    }
}

/// A Linux distribution, reduced to what bindist selection needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinuxDistro {
    /// ghcup's distribution name without the `Linux_` prefix (`Debian`,
    /// `Ubuntu`, `Alpine`, ...). `None` means ghcup has no builds specific
    /// to it and the generic `Linux_UnknownLinux` build applies.
    pub family: Option<&'static str>,
    /// The distribution's `VERSION_ID`, when it is the distribution's own
    /// version. Left empty when the family was inferred from `ID_LIKE` or
    /// the C library, since a derivative's version numbers say nothing about
    /// its parent's.
    pub version: Option<String>,
}

/// Operating system of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostOs {
    /// macOS.
    Darwin,
    /// Windows.
    Windows,
    /// Linux, with the detected distribution.
    Linux(LinuxDistro),
}

/// The machine a download is selected for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// CPU architecture.
    pub arch: Arch,
    /// Operating system (and distribution on Linux).
    pub os: HostOs,
}

impl Host {
    /// Describe `platform`, detecting the Linux distribution of this machine.
    ///
    /// Only meaningful when `platform` is the platform hx runs on; for any
    /// other platform the distribution of this machine is irrelevant, so use
    /// [`Host::with_distro`].
    pub fn detect(platform: Platform) -> Self {
        let distro = matches!(platform, Platform::X86_64Linux | Platform::Aarch64Linux)
            .then(detect_linux_distro)
            .unwrap_or_default();
        Self::with_distro(platform, distro)
    }

    /// Describe `platform` with an explicit Linux distribution (ignored for
    /// non-Linux platforms).
    pub fn with_distro(platform: Platform, distro: LinuxDistro) -> Self {
        let (arch, os) = match platform {
            Platform::X86_64Linux => (Arch::X86_64, HostOs::Linux(distro)),
            Platform::Aarch64Linux => (Arch::Aarch64, HostOs::Linux(distro)),
            Platform::X86_64Darwin => (Arch::X86_64, HostOs::Darwin),
            Platform::Aarch64Darwin => (Arch::Aarch64, HostOs::Darwin),
            Platform::X86_64Windows => (Arch::X86_64, HostOs::Windows),
        };
        Self { arch, os }
    }
}

/// `os-release` `ID` values and the ghcup distribution they map to.
///
/// Mirrors ghcup's own distribution detection.
const DISTRO_IDS: &[(&str, &str)] = &[
    ("debian", "Debian"),
    ("ubuntu", "Ubuntu"),
    ("linuxmint", "Mint"),
    ("fedora", "Fedora"),
    ("centos", "CentOS"),
    ("rhel", "RedHat"),
    ("alpine", "Alpine"),
    ("amzn", "AmazonLinux"),
    ("rocky", "Rocky"),
    ("void", "Void"),
    ("gentoo", "Gentoo"),
    ("exherbo", "Exherbo"),
    ("opensuse", "OpenSUSE"),
    ("opensuse-leap", "OpenSUSE"),
    ("opensuse-tumbleweed", "OpenSUSE"),
    ("sles", "OpenSUSE"),
    ("suse", "OpenSUSE"),
];

fn family_for_id(id: &str) -> Option<&'static str> {
    let id = id.to_ascii_lowercase();
    DISTRO_IDS
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, family)| *family)
}

/// Parse `os-release` contents into key/value pairs (quotes removed).
pub fn parse_os_release(contents: &str) -> HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}

/// Work out the distribution from `os-release` contents and the C library.
///
/// The `ID` is tried first and keeps its `VERSION_ID`. Failing that, each
/// `ID_LIKE` entry is tried in order without a version, so Pop!_OS gets
/// Ubuntu's default build rather than one picked by Pop's own version
/// number. A musl system that is not otherwise recognized is treated as
/// Alpine, since the Alpine builds are the musl builds.
pub fn linux_distro_from(os_release: Option<&str>, musl: bool) -> LinuxDistro {
    let fields = os_release.map(parse_os_release).unwrap_or_default();
    let version = fields.get("VERSION_ID").filter(|v| !v.is_empty()).cloned();

    if let Some(family) = fields.get("ID").and_then(|id| family_for_id(id)) {
        return LinuxDistro {
            family: Some(family),
            version,
        };
    }

    let like = fields
        .get("ID_LIKE")
        .into_iter()
        .flat_map(|v| v.split_whitespace())
        .find_map(family_for_id);
    if let Some(family) = like
        && (!musl || family == "Alpine")
    {
        return LinuxDistro {
            family: Some(family),
            version: None,
        };
    }

    if musl {
        return LinuxDistro {
            family: Some("Alpine"),
            version: None,
        };
    }

    LinuxDistro::default()
}

/// Detect the Linux distribution of this machine.
pub fn detect_linux_distro() -> LinuxDistro {
    let os_release = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok());
    linux_distro_from(os_release.as_deref(), is_musl_system(Path::new("/")))
}

/// Whether the system C library is musl: a musl dynamic loader is present
/// and no glibc loader is. Checking both keeps glibc systems that merely have
/// `musl-tools` installed on glibc builds.
fn is_musl_system(root: &Path) -> bool {
    let has_loader = |dirs: &[&str], prefix: &str| {
        dirs.iter().any(|dir| {
            std::fs::read_dir(root.join(dir))
                .map(|entries| {
                    entries
                        .flatten()
                        .any(|e| e.file_name().to_string_lossy().starts_with(prefix))
                })
                .unwrap_or(false)
        })
    };
    has_loader(&["lib"], "ld-musl-") && !has_loader(&["lib", "lib64"], "ld-linux")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBIAN_12: &str = r#"PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"
NAME="Debian GNU/Linux"
VERSION_ID="12"
VERSION="12 (bookworm)"
VERSION_CODENAME=bookworm
ID=debian
HOME_URL="https://www.debian.org/"
"#;

    const UBUNTU_2404: &str = r#"PRETTY_NAME="Ubuntu 24.04.1 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
VERSION="24.04.1 LTS (Noble Numbat)"
ID=ubuntu
ID_LIKE=debian
UBUNTU_CODENAME=noble
"#;

    const FEDORA_40: &str = r#"NAME="Fedora Linux"
VERSION="40 (Workstation Edition)"
ID=fedora
VERSION_ID=40
PLATFORM_ID="platform:f40"
"#;

    const ARCH: &str = r#"NAME="Arch Linux"
PRETTY_NAME="Arch Linux"
ID=arch
BUILD_ID=rolling
"#;

    const ALPINE_320: &str = r#"NAME="Alpine Linux"
ID=alpine
VERSION_ID=3.20.3
PRETTY_NAME="Alpine Linux v3.20"
"#;

    const POP_OS: &str = r#"NAME="Pop!_OS"
VERSION="22.04 LTS"
ID=pop
ID_LIKE="ubuntu debian"
VERSION_ID="22.04"
"#;

    #[test]
    fn test_parse_os_release() {
        let fields = parse_os_release(UBUNTU_2404);
        assert_eq!(fields["ID"], "ubuntu");
        assert_eq!(fields["VERSION_ID"], "24.04");
        assert_eq!(fields["PRETTY_NAME"], "Ubuntu 24.04.1 LTS");
    }

    #[test]
    fn test_known_distributions_keep_their_version() {
        let debian = linux_distro_from(Some(DEBIAN_12), false);
        assert_eq!(debian.family, Some("Debian"));
        assert_eq!(debian.version.as_deref(), Some("12"));

        let ubuntu = linux_distro_from(Some(UBUNTU_2404), false);
        assert_eq!(ubuntu.family, Some("Ubuntu"));
        assert_eq!(ubuntu.version.as_deref(), Some("24.04"));

        let fedora = linux_distro_from(Some(FEDORA_40), false);
        assert_eq!(fedora.family, Some("Fedora"));
        assert_eq!(fedora.version.as_deref(), Some("40"));

        let alpine = linux_distro_from(Some(ALPINE_320), true);
        assert_eq!(alpine.family, Some("Alpine"));
        assert_eq!(alpine.version.as_deref(), Some("3.20.3"));
    }

    #[test]
    fn test_unknown_distributions() {
        assert_eq!(linux_distro_from(Some(ARCH), false), LinuxDistro::default());
        assert_eq!(linux_distro_from(None, false), LinuxDistro::default());
        assert_eq!(
            linux_distro_from(Some("garbage without equals"), false),
            LinuxDistro::default()
        );
    }

    #[test]
    fn test_id_like_drops_the_version() {
        let pop = linux_distro_from(Some(POP_OS), false);
        assert_eq!(pop.family, Some("Ubuntu"));
        assert_eq!(pop.version, None);
    }

    #[test]
    fn test_musl_means_alpine() {
        let musl_arch = linux_distro_from(Some(ARCH), true);
        assert_eq!(musl_arch.family, Some("Alpine"));
        assert_eq!(musl_arch.version, None);

        let musl_unknown = linux_distro_from(None, true);
        assert_eq!(musl_unknown.family, Some("Alpine"));
    }

    #[test]
    fn test_is_musl_system() {
        let root = tempfile::tempdir().unwrap();
        let lib = root.path().join("lib");
        std::fs::create_dir(&lib).unwrap();
        assert!(!is_musl_system(root.path()));

        std::fs::write(lib.join("ld-musl-x86_64.so.1"), "").unwrap();
        assert!(is_musl_system(root.path()));

        // glibc system with musl-tools installed
        let lib64 = root.path().join("lib64");
        std::fs::create_dir(&lib64).unwrap();
        std::fs::write(lib64.join("ld-linux-x86-64.so.2"), "").unwrap();
        assert!(!is_musl_system(root.path()));
    }

    #[test]
    fn test_host_for_platforms() {
        let host = Host::with_distro(Platform::Aarch64Darwin, LinuxDistro::default());
        assert_eq!(host.arch, Arch::Aarch64);
        assert_eq!(host.os, HostOs::Darwin);

        let host = Host::with_distro(Platform::X86_64Windows, LinuxDistro::default());
        assert_eq!(host.os, HostOs::Windows);

        let distro = linux_distro_from(Some(DEBIAN_12), false);
        let host = Host::with_distro(Platform::X86_64Linux, distro.clone());
        assert_eq!(host.arch.key(), "A_64");
        assert_eq!(host.os, HostOs::Linux(distro));
    }
}
