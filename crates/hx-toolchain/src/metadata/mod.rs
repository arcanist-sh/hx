//! ghcup release metadata.
//!
//! ghcup publishes a YAML file listing every GHC, Cabal and HLS release with
//! its tags (`Recommended`, `Latest`, `old`, ...) and, per architecture,
//! operating system and distribution version, the bindist URL, the directory
//! it unpacks into, and its SHA-256. hx reads that file instead of keeping
//! its own tables, so new releases, renamed assets and per-distribution
//! builds reach hx without a new hx release.
//!
//! The metadata is an optimization, never a requirement: every caller falls
//! back to the built-in version tables and URL scheme when it cannot be
//! fetched or has no entry for what is asked.
//!
//! See [`load`] for fetching and caching, and [`GhcupMetadata::download`]
//! for bindist selection.

mod fetch;
mod host;
mod range;

pub use fetch::{
    DEFAULT_METADATA_URL, LoadOptions, METADATA_TTL, METADATA_URL_ENV, OFFLINE_ENV, cache_file,
    is_offline, load, load_with, metadata_url,
};
pub use host::{
    Arch, Host, HostOs, LinuxDistro, detect_linux_distro, linux_distro_from, parse_os_release,
};

use crate::cabal::{KNOWN_CABAL_VERSIONS, RECOMMENDED_CABAL_VERSION};
use crate::ghc::{KNOWN_GHC_VERSIONS, Platform, RECOMMENDED_GHC_VERSION};
use std::cmp::Ordering;
use tracing::debug;
use yaml_rust2::{Yaml, YamlLoader};

/// A tool described by the metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    /// The Glasgow Haskell Compiler.
    Ghc,
    /// cabal-install.
    Cabal,
    /// haskell-language-server.
    Hls,
}

impl Tool {
    /// The key ghcup uses under `ghcupDownloads`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Ghc => "GHC",
            Self::Cabal => "Cabal",
            Self::Hls => "HLS",
        }
    }
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

/// Error parsing ghcup metadata.
#[derive(Debug, thiserror::Error)]
#[error("invalid ghcup metadata: {0}")]
pub struct MetadataError(String);

/// One downloadable bindist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    /// Download URL (`dlUri`).
    pub url: String,
    /// Directory the archive unpacks into (`dlSubdir`), when given as a
    /// plain path. ghcup also allows a regex here; those are dropped.
    pub subdir: Option<String>,
    /// Expected SHA-256 of the archive (`dlHash`), lowercase hex.
    pub sha256: Option<String>,
    /// File name to save the download as (`dlOutput`), when the URL's own
    /// file name is not the archive name.
    pub output: Option<String>,
}

/// Downloads for one architecture and operating system, keyed by
/// distribution version range (or `unknown_versioning`), in file order.
#[derive(Debug, Clone)]
struct PlatformDownloads {
    arch: String,
    os: String,
    entries: Vec<(String, Download)>,
}

impl PlatformDownloads {
    fn unversioned(&self) -> Option<&Download> {
        self.entries
            .iter()
            .find(|(key, _)| key == UNKNOWN_VERSIONING)
            .map(|(_, d)| d)
    }

    fn for_version(&self, version: &str) -> Option<&Download> {
        self.entries
            .iter()
            .filter(|(key, _)| key != UNKNOWN_VERSIONING)
            .find(|(key, _)| range::range_matches(key, version) == Some(true))
            .map(|(_, d)| d)
    }
}

const UNKNOWN_VERSIONING: &str = "unknown_versioning";
const UNKNOWN_LINUX: &str = "Linux_UnknownLinux";

/// One release of a tool.
#[derive(Debug, Clone)]
pub struct Release {
    /// Version string, e.g. `9.10.3`.
    pub version: String,
    /// ghcup tags (`Recommended`, `Latest`, `old`, `base-4.20.1.0`, ...).
    pub tags: Vec<String>,
    platforms: Vec<PlatformDownloads>,
}

impl Release {
    fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Whether ghcup recommends this release.
    pub fn is_recommended(&self) -> bool {
        self.has_tag("Recommended")
    }

    /// Whether this is the latest stable release.
    pub fn is_latest(&self) -> bool {
        self.has_tag("Latest")
    }

    /// Whether ghcup marks this release as old (hidden from default listings).
    pub fn is_old(&self) -> bool {
        self.has_tag("old")
    }

    /// Whether this is a prerelease or nightly build.
    pub fn is_prerelease(&self) -> bool {
        self.tags.iter().any(|t| {
            matches!(
                t.as_str(),
                "Prerelease" | "LatestPrerelease" | "Nightly" | "LatestNightly"
            )
        })
    }

    /// Pick the bindist for `host`, the way ghcup does: the distribution's
    /// build for a matching version range, then its unversioned build, then
    /// the generic `Linux_UnknownLinux` build.
    pub fn download_for(&self, host: &Host) -> Option<&Download> {
        let arch = host.arch.key();
        let platform = |os: &str| self.platforms.iter().find(|p| p.arch == arch && p.os == os);
        match &host.os {
            HostOs::Darwin => platform("Darwin")?.unversioned(),
            HostOs::Windows => platform("Windows")?.unversioned(),
            HostOs::Linux(distro) => {
                if let Some(family) = distro.family
                    && let Some(p) = platform(&format!("Linux_{family}"))
                {
                    let ranged = distro.version.as_deref().and_then(|v| p.for_version(v));
                    if let Some(download) = ranged.or_else(|| p.unversioned()) {
                        return Some(download);
                    }
                }
                platform(UNKNOWN_LINUX)?.unversioned()
            }
        }
    }
}

/// Parsed ghcup metadata: releases of GHC, Cabal and HLS.
#[derive(Debug, Clone, Default)]
pub struct GhcupMetadata {
    ghc: Vec<Release>,
    cabal: Vec<Release>,
    hls: Vec<Release>,
}

impl GhcupMetadata {
    /// Parse the contents of a `ghcup-<schema>.yaml` file.
    ///
    /// Only GHC, Cabal and HLS are read; other tools, install specs and
    /// tool requirements are ignored. Fails if there are no GHC releases,
    /// which catches HTML error pages and truncated downloads.
    pub fn parse(contents: &str) -> Result<Self, MetadataError> {
        let docs = YamlLoader::load_from_str(contents).map_err(|e| MetadataError(e.to_string()))?;
        let root = docs
            .first()
            .ok_or_else(|| MetadataError("empty document".into()))?;
        let downloads = &root["ghcupDownloads"];
        if downloads.is_badvalue() {
            return Err(MetadataError("missing ghcupDownloads".into()));
        }

        let metadata = Self {
            ghc: parse_releases(&downloads[Tool::Ghc.key()]),
            cabal: parse_releases(&downloads[Tool::Cabal.key()]),
            hls: parse_releases(&downloads[Tool::Hls.key()]),
        };
        if metadata.ghc.is_empty() {
            return Err(MetadataError("no GHC releases".into()));
        }
        Ok(metadata)
    }

    /// All releases of `tool`, newest first.
    pub fn releases(&self, tool: Tool) -> &[Release] {
        match tool {
            Tool::Ghc => &self.ghc,
            Tool::Cabal => &self.cabal,
            Tool::Hls => &self.hls,
        }
    }

    /// The release of `tool` with exactly this version.
    pub fn release(&self, tool: Tool, version: &str) -> Option<&Release> {
        self.releases(tool).iter().find(|r| r.version == version)
    }

    /// The version ghcup tags `Recommended`.
    pub fn recommended(&self, tool: Tool) -> Option<&str> {
        self.releases(tool)
            .iter()
            .find(|r| r.is_recommended())
            .map(|r| r.version.as_str())
    }

    /// The version ghcup tags `Latest`.
    pub fn latest(&self, tool: Tool) -> Option<&str> {
        self.releases(tool)
            .iter()
            .find(|r| r.is_latest())
            .map(|r| r.version.as_str())
    }

    /// The bindist of `tool` `version` for `host`, if the metadata has one.
    pub fn download(&self, tool: Tool, version: &str, host: &Host) -> Option<&Download> {
        self.release(tool, version)?.download_for(host)
    }
}

fn scalar_string(node: &Yaml) -> Option<String> {
    match node {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

fn parse_releases(tool: &Yaml) -> Vec<Release> {
    let Some(versions) = tool["toolVersions"].as_hash() else {
        return Vec::new();
    };
    let mut releases: Vec<Release> = versions
        .iter()
        .filter_map(|(version, info)| {
            Some(Release {
                version: scalar_string(version)?,
                tags: info["viTags"]
                    .as_vec()
                    .map(|tags| tags.iter().filter_map(scalar_string).collect())
                    .unwrap_or_default(),
                platforms: parse_platforms(&info["viArch"]),
            })
        })
        .collect();
    releases.sort_by(|a, b| compare_release_versions(&b.version, &a.version));
    releases
}

fn parse_platforms(arches: &Yaml) -> Vec<PlatformDownloads> {
    let Some(arches) = arches.as_hash() else {
        return Vec::new();
    };
    let mut platforms = Vec::new();
    for (arch, oses) in arches {
        let (Some(arch), Some(oses)) = (scalar_string(arch), oses.as_hash()) else {
            continue;
        };
        for (os, ranges) in oses {
            let (Some(os), Some(ranges)) = (scalar_string(os), ranges.as_hash()) else {
                continue;
            };
            let entries = ranges
                .iter()
                .filter_map(|(range, dl)| Some((scalar_string(range)?, parse_download(dl)?)))
                .collect();
            platforms.push(PlatformDownloads {
                arch: arch.clone(),
                os,
                entries,
            });
        }
    }
    platforms
}

fn parse_download(node: &Yaml) -> Option<Download> {
    let url = node["dlUri"].as_str()?.to_string();
    let sha256 = node["dlHash"]
        .as_str()
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|h| h.to_ascii_lowercase());
    Some(Download {
        url,
        // A `RegexDir` mapping is not a path; leave it to the fallback search.
        subdir: node["dlSubdir"].as_str().map(str::to_string),
        sha256,
        output: node["dlOutput"].as_str().map(str::to_string),
    })
}

/// Order release versions numerically (`9.10.1` > `9.8.4`); a suffixed
/// version (`3.6.2.0-p1`) sorts after the plain one.
fn compare_release_versions(a: &str, b: &str) -> Ordering {
    let split = |v: &str| {
        let (base, suffix) = v.split_once('-').unwrap_or((v, ""));
        let parts: Vec<u64> = base.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        (parts, suffix.to_string())
    };
    let (a_parts, a_suffix) = split(a);
    let (b_parts, b_suffix) = split(b);
    range::compare_versions(&a_parts, &b_parts).then_with(|| a_suffix.cmp(&b_suffix))
}

/// Compare two dotted versions numerically.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    compare_release_versions(a, b)
}

/// The archive to download for an install, wherever it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDownload {
    /// Download URL.
    pub url: String,
    /// File name to store the archive under.
    pub file_name: String,
    /// Directory the archive unpacks into, when known.
    pub subdir: Option<String>,
    /// Expected SHA-256, when the metadata provides one. When absent, the
    /// publisher's `SHA256SUMS` is consulted instead.
    pub sha256: Option<String>,
    /// Whether this came from ghcup metadata (as opposed to the built-in
    /// URL scheme).
    pub from_metadata: bool,
}

impl ResolvedDownload {
    /// A download built from hx's own URL scheme.
    pub fn built_in(url: String) -> Self {
        Self {
            file_name: url_file_name(&url),
            url,
            subdir: None,
            sha256: None,
            from_metadata: false,
        }
    }

    fn from_metadata(download: &Download) -> Self {
        Self {
            file_name: download
                .output
                .clone()
                .unwrap_or_else(|| url_file_name(&download.url)),
            url: download.url.clone(),
            subdir: download.subdir.clone(),
            sha256: download.sha256.clone(),
            from_metadata: true,
        }
    }
}

fn url_file_name(url: &str) -> String {
    url.rsplit('/').next().unwrap_or(url).to_string()
}

/// Archive formats hx can unpack for each tool.
fn is_supported_archive(tool: Tool, url: &str) -> bool {
    match tool {
        Tool::Ghc => url.ends_with(".tar.xz"),
        Tool::Cabal | Tool::Hls => url.ends_with(".tar.xz") || url.ends_with(".zip"),
    }
}

/// Choose the archive for `tool` `version`: the metadata's bindist for
/// `host` when there is one hx can unpack, otherwise `built_in_url`.
pub fn resolve_download(
    metadata: Option<&GhcupMetadata>,
    tool: Tool,
    version: &str,
    host: Option<&Host>,
    built_in_url: String,
) -> ResolvedDownload {
    metadata
        .zip(host)
        .and_then(|(m, h)| m.download(tool, version, h))
        .filter(|d| is_supported_archive(tool, &d.url))
        .map(ResolvedDownload::from_metadata)
        .unwrap_or_else(|| ResolvedDownload::built_in(built_in_url))
}

/// Choose the archive to install `tool` `version` for `platform`, loading
/// metadata as configured by the environment.
///
/// Distribution detection describes this machine, so metadata is only
/// consulted when `platform` is the platform hx runs on; for any other
/// platform, and whenever metadata is unavailable or silent, the result is
/// `built_in_url`.
pub async fn resolve_install_download(
    tool: Tool,
    version: &str,
    platform: Platform,
    built_in_url: String,
) -> ResolvedDownload {
    if Platform::current() != Some(platform) {
        return ResolvedDownload::built_in(built_in_url);
    }
    let host = Host::detect(platform);
    let metadata = load().await;
    let resolved = resolve_download(
        metadata.as_deref(),
        tool,
        version,
        Some(&host),
        built_in_url,
    );
    debug!(
        "{} {} archive: {} (from {})",
        tool,
        version,
        resolved.url,
        if resolved.from_metadata {
            "ghcup metadata"
        } else {
            "built-in URL scheme"
        }
    );
    resolved
}

/// The version to recommend for `tool`: ghcup's `Recommended` release when
/// metadata is available, otherwise hx's built-in default.
///
/// Only used for suggestions and defaults; versions pinned in `hx.toml`
/// always take precedence.
pub fn recommended_version(metadata: Option<&GhcupMetadata>, tool: Tool) -> Option<String> {
    metadata
        .and_then(|m| m.recommended(tool))
        .map(str::to_string)
        .or_else(|| match tool {
            Tool::Ghc => Some(RECOMMENDED_GHC_VERSION.to_string()),
            Tool::Cabal => Some(RECOMMENDED_CABAL_VERSION.to_string()),
            Tool::Hls => None,
        })
}

/// A version as shown by `hx toolchain list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionListing {
    /// Version string.
    pub version: String,
    /// Tagged `Recommended`.
    pub recommended: bool,
    /// Tagged `Latest`.
    pub latest: bool,
    /// Tagged `old`.
    pub old: bool,
    /// A prerelease or nightly.
    pub prerelease: bool,
}

/// Known versions of `tool`, newest first: from the metadata when available,
/// otherwise from the built-in tables.
pub fn version_listing(metadata: Option<&GhcupMetadata>, tool: Tool) -> Vec<VersionListing> {
    if let Some(m) = metadata {
        return m
            .releases(tool)
            .iter()
            .map(|r| VersionListing {
                version: r.version.clone(),
                recommended: r.is_recommended(),
                latest: r.is_latest(),
                old: r.is_old(),
                prerelease: r.is_prerelease(),
            })
            .collect();
    }

    let (known, recommended): (&[&str], &str) = match tool {
        Tool::Ghc => (KNOWN_GHC_VERSIONS, RECOMMENDED_GHC_VERSION),
        Tool::Cabal => (KNOWN_CABAL_VERSIONS, RECOMMENDED_CABAL_VERSION),
        Tool::Hls => (&[], ""),
    };
    let mut versions: Vec<&str> = known.to_vec();
    versions.sort_by(|a, b| compare_release_versions(b, a));
    versions
        .iter()
        .enumerate()
        .map(|(i, v)| VersionListing {
            version: v.to_string(),
            recommended: *v == recommended,
            latest: i == 0,
            old: false,
            prerelease: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../fixtures/ghcup-0.1.0-excerpt.yaml");

    fn fixture() -> GhcupMetadata {
        GhcupMetadata::parse(FIXTURE).expect("fixture parses")
    }

    fn linux(platform: Platform, family: Option<&'static str>, version: Option<&str>) -> Host {
        Host::with_distro(
            platform,
            LinuxDistro {
                family,
                version: version.map(str::to_string),
            },
        )
    }

    fn url_of(m: &GhcupMetadata, tool: Tool, version: &str, host: &Host) -> String {
        m.download(tool, version, host)
            .map(|d| d.url.rsplit('/').next().unwrap().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn test_parses_releases_newest_first() {
        let m = fixture();
        let ghc: Vec<_> = m
            .releases(Tool::Ghc)
            .iter()
            .map(|r| r.version.as_str())
            .collect();
        assert_eq!(ghc, ["9.14.1", "9.10.3", "9.8.1", "7.10.3"]);
        let cabal: Vec<_> = m
            .releases(Tool::Cabal)
            .iter()
            .map(|r| r.version.as_str())
            .collect();
        assert_eq!(cabal, ["3.18.1.0", "3.16.1.0", "3.6.2.0-p1", "3.6.2.0"]);
        assert_eq!(m.releases(Tool::Hls)[0].version, "2.14.0.0");
    }

    #[test]
    fn test_tags() {
        let m = fixture();
        assert_eq!(m.recommended(Tool::Ghc), Some("9.10.3"));
        assert_eq!(m.latest(Tool::Ghc), Some("9.14.1"));
        assert_eq!(m.recommended(Tool::Cabal), Some("3.16.1.0"));
        assert_eq!(m.latest(Tool::Cabal), Some("3.18.1.0"));
        assert_eq!(m.recommended(Tool::Hls), Some("2.14.0.0"));
        assert!(m.release(Tool::Ghc, "7.10.3").unwrap().is_old());
        assert!(!m.release(Tool::Ghc, "9.10.3").unwrap().is_old());
    }

    #[test]
    fn test_aliases_are_resolved() {
        let m = fixture();
        // Linux_UnknownLinux is `*ghc-9103-64-rocky8`, and Linux_RedHat aliases it.
        let host = linux(Platform::X86_64Linux, None, None);
        let d = m.download(Tool::Ghc, "9.10.3", &host).unwrap();
        assert!(d.url.ends_with("ghc-9.10.3-x86_64-rocky8-linux.tar.xz"));
        assert_eq!(d.subdir.as_deref(), Some("ghc-9.10.3-x86_64-unknown-linux"));
        assert_eq!(
            d.sha256.as_deref(),
            Some("d0504f094b827e64653f8e8d7b4b3029bf7ff026f5091c0b22366dcd1b032f2d")
        );
    }

    #[test]
    fn test_matcher_debian() {
        let m = fixture();
        let deb12 = linux(Platform::X86_64Linux, Some("Debian"), Some("12"));
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &deb12),
            "ghc-9.10.3-x86_64-deb12-linux.tar.xz"
        );
        let deb10 = linux(Platform::X86_64Linux, Some("Debian"), Some("10"));
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &deb10),
            "ghc-9.10.3-x86_64-deb10-linux.tar.xz"
        );
        // Debian testing has no VERSION_ID: unknown_versioning
        let testing = linux(Platform::X86_64Linux, Some("Debian"), None);
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &testing),
            "ghc-9.10.3-x86_64-deb12-linux.tar.xz"
        );
    }

    #[test]
    fn test_matcher_ubuntu_2404() {
        let m = fixture();
        let host = linux(Platform::X86_64Linux, Some("Ubuntu"), Some("24.04"));
        // No range covers 24.04, so Ubuntu's unknown_versioning build
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &host),
            "ghc-9.10.3-x86_64-ubuntu20_04-linux.tar.xz"
        );
        let old = linux(Platform::X86_64Linux, Some("Ubuntu"), Some("18.04"));
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &old),
            "ghc-9.10.3-x86_64-ubuntu18_04-linux.tar.xz"
        );
    }

    #[test]
    fn test_matcher_fedora_40() {
        let m = fixture();
        let host = linux(Platform::X86_64Linux, Some("Fedora"), Some("40"));
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &host),
            "ghc-9.10.3-x86_64-fedora38-linux.tar.xz"
        );
    }

    #[test]
    fn test_matcher_arch_and_unknown() {
        let m = fixture();
        let host = linux(Platform::X86_64Linux, None, None);
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &host),
            "ghc-9.10.3-x86_64-rocky8-linux.tar.xz"
        );
        let host = linux(Platform::Aarch64Linux, None, None);
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &host),
            "ghc-9.10.3-aarch64-deb10-linux.tar.xz"
        );
        // A distribution ghcup knows but this release has no build for
        let gentoo = linux(Platform::X86_64Linux, Some("Gentoo"), Some("2.15"));
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &gentoo),
            "ghc-9.10.3-x86_64-rocky8-linux.tar.xz"
        );
    }

    #[test]
    fn test_matcher_alpine_320() {
        let m = fixture();
        let host = linux(Platform::X86_64Linux, Some("Alpine"), Some("3.20.3"));
        let d = m.download(Tool::Ghc, "9.10.3", &host).unwrap();
        assert!(d.url.ends_with("ghc-9.10.3-x86_64-alpine3_18-linux.tar.xz"));
        let host = linux(Platform::X86_64Linux, Some("Alpine"), Some("3.14"));
        assert_eq!(
            url_of(&m, Tool::Ghc, "9.10.3", &host),
            "ghc-9.10.3-x86_64-alpine3_12-linux.tar.xz"
        );
        let host = linux(Platform::Aarch64Linux, Some("Alpine"), None);
        let d = m.download(Tool::Ghc, "9.10.3", &host).unwrap();
        assert_eq!(d.subdir.as_deref(), Some("ghc-9.10.3-aarch64-alpine-linux"));
        let host = linux(Platform::X86_64Linux, Some("Alpine"), Some("3.20.3"));
        assert_eq!(
            url_of(&m, Tool::Cabal, "3.16.1.0", &host),
            "cabal-install-3.16.1.0-x86_64-linux-musl-static.tar.xz"
        );
    }

    #[test]
    fn test_matcher_darwin_and_windows() {
        let m = fixture();
        let mac = Host::with_distro(Platform::Aarch64Darwin, LinuxDistro::default());
        let d = m.download(Tool::Cabal, "3.18.1.0", &mac).unwrap();
        assert!(
            d.url
                .ends_with("cabal-install-3.18.1.0-aarch64-apple-darwin.tar.xz")
        );
        assert_eq!(d.subdir, None);
        let win = Host::with_distro(Platform::X86_64Windows, LinuxDistro::default());
        assert_eq!(
            url_of(&m, Tool::Cabal, "3.18.1.0", &win),
            "cabal-install-3.18.1.0-x86_64-mingw64.zip"
        );
        // 7.10.3 was never built for Apple Silicon
        assert!(m.download(Tool::Ghc, "7.10.3", &mac).is_none());
    }

    #[test]
    fn test_dl_output_names_the_archive() {
        let m = fixture();
        let host = Host::with_distro(Platform::X86_64Darwin, LinuxDistro::default());
        let d = m.download(Tool::Ghc, "9.8.1", &host).unwrap();
        assert_eq!(d.output.as_deref(), Some("ghc-9.8.1-x86_64-darwin.tar.xz"));
        let resolved = resolve_download(Some(&m), Tool::Ghc, "9.8.1", Some(&host), "unused".into());
        assert!(
            resolved
                .url
                .ends_with("ghc-9.8.1-x86_64-apple-darwin.tar.xz")
        );
        assert_eq!(resolved.file_name, "ghc-9.8.1-x86_64-darwin.tar.xz");
    }

    #[test]
    fn test_regex_subdir_is_dropped() {
        let yaml = r#"
ghcupDownloads:
  GHC:
    toolVersions:
      9.4.8:
        viTags: [Latest]
        viArch:
          A_64:
            Darwin:
              unknown_versioning:
                dlUri: https://example.invalid/ghc-9.4.8.tar.xz
                dlSubdir:
                  RegexDir: ghc-.*
                dlHash: NOTAHASH
"#;
        let m = GhcupMetadata::parse(yaml).unwrap();
        let host = Host::with_distro(Platform::X86_64Darwin, LinuxDistro::default());
        let d = m.download(Tool::Ghc, "9.4.8", &host).unwrap();
        assert_eq!(d.subdir, None);
        assert_eq!(d.sha256, None);
        assert_eq!(m.latest(Tool::Ghc), Some("9.4.8"));
        assert_eq!(m.recommended(Tool::Ghc), None);
        assert!(m.releases(Tool::Cabal).is_empty());
    }

    #[test]
    fn test_resolve_download_falls_back() {
        let m = fixture();
        let host = Host::with_distro(Platform::Aarch64Darwin, LinuxDistro::default());
        let built_in = "https://example.invalid/x/ghc-9.12.3-aarch64-apple-darwin.tar.xz";

        // Version missing from metadata
        let r = resolve_download(Some(&m), Tool::Ghc, "9.12.3", Some(&host), built_in.into());
        assert_eq!(r, ResolvedDownload::built_in(built_in.into()));
        assert_eq!(r.file_name, "ghc-9.12.3-aarch64-apple-darwin.tar.xz");
        assert!(!r.from_metadata);

        // No metadata at all
        let r = resolve_download(None, Tool::Ghc, "9.10.3", Some(&host), built_in.into());
        assert!(!r.from_metadata);

        // No host (installing for another platform)
        let r = resolve_download(Some(&m), Tool::Ghc, "9.10.3", None, built_in.into());
        assert!(!r.from_metadata);

        // Present
        let r = resolve_download(Some(&m), Tool::Ghc, "9.10.3", Some(&host), built_in.into());
        assert!(r.from_metadata);
        assert_eq!(r.file_name, "ghc-9.10.3-aarch64-apple-darwin.tar.xz");
        assert_eq!(
            r.sha256.as_deref(),
            Some("9f50ddd87be5cb994c719402778d6c7fdd341934fd4fbc0fcc3ecb40d49f860c")
        );
    }

    #[test]
    fn test_resolve_download_skips_unsupported_archives() {
        let m = fixture();
        let host = linux(Platform::X86_64Linux, Some("Debian"), Some("12"));
        // 7.10.3 only has .tar.bz2 bindists
        let r = resolve_download(
            Some(&m),
            Tool::Ghc,
            "7.10.3",
            Some(&host),
            "b/fallback.tar.xz".into(),
        );
        assert!(!r.from_metadata);
    }

    #[test]
    fn test_recommended_version_fallback() {
        let m = fixture();
        assert_eq!(
            recommended_version(Some(&m), Tool::Ghc).as_deref(),
            Some("9.10.3")
        );
        assert_eq!(
            recommended_version(None, Tool::Ghc).as_deref(),
            Some(RECOMMENDED_GHC_VERSION)
        );
        assert_eq!(
            recommended_version(None, Tool::Cabal).as_deref(),
            Some(RECOMMENDED_CABAL_VERSION)
        );
        assert_eq!(recommended_version(None, Tool::Hls), None);
    }

    #[test]
    fn test_version_listing() {
        let m = fixture();
        let listing = version_listing(Some(&m), Tool::Ghc);
        assert_eq!(listing[0].version, "9.14.1");
        assert!(listing[0].latest);
        assert!(
            listing
                .iter()
                .any(|l| l.version == "9.10.3" && l.recommended)
        );
        assert!(listing.iter().any(|l| l.version == "7.10.3" && l.old));

        let fallback = version_listing(None, Tool::Ghc);
        assert_eq!(fallback.len(), KNOWN_GHC_VERSIONS.len());
        assert!(fallback[0].latest);
        assert!(
            fallback
                .iter()
                .any(|l| l.version == RECOMMENDED_GHC_VERSION && l.recommended)
        );
        assert!(
            fallback
                .windows(2)
                .all(|w| compare_versions(&w[0].version, &w[1].version) == Ordering::Greater)
        );
    }

    #[test]
    fn test_rejects_non_metadata() {
        assert!(GhcupMetadata::parse("<html>rate limited</html>").is_err());
        assert!(GhcupMetadata::parse("ghcupDownloads: {}").is_err());
        assert!(GhcupMetadata::parse("key: [unterminated").is_err());
        assert!(GhcupMetadata::parse("").is_err());
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("9.10.1", "9.8.4"), Ordering::Greater);
        assert_eq!(compare_versions("3.6.2.0-p1", "3.6.2.0"), Ordering::Greater);
        assert_eq!(compare_versions("9.10.3", "9.10.3"), Ordering::Equal);
    }
}
