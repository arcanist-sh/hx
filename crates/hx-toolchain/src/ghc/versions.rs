//! GHC version information and known versions.

use hx_core::Version;
use serde::{Deserialize, Serialize};

/// Known stable GHC versions.
///
/// This list is updated with each hx release and contains tested,
/// working versions. Users can still install versions not in this list.
pub static KNOWN_GHC_VERSIONS: &[&str] = &[
    // GHC 9.14.x
    "9.14.1", //
    // GHC 9.12.x
    "9.12.4", "9.12.3", "9.12.2", "9.12.1", //
    // GHC 9.10.x
    "9.10.3", "9.10.2", "9.10.1", //
    // GHC 9.8.x
    "9.8.4", "9.8.3", "9.8.2", "9.8.1", //
    // GHC 9.6.x
    "9.6.7", "9.6.6", "9.6.5", "9.6.4", "9.6.3", "9.6.2", "9.6.1", //
    // GHC 9.4.x
    "9.4.8", "9.4.7", "9.4.6", "9.4.5", "9.4.4", //
    // GHC 9.2.x
    "9.2.8", "9.2.7", //
    // GHC 9.0.x
    "9.0.2",
];

/// The recommended GHC version for new projects.
///
/// Tracks ghcup's recommended release and the current Stackage LTS (24).
pub static RECOMMENDED_GHC_VERSION: &str = "9.10.3";

/// GHC version with metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhcVersion {
    /// The version string (e.g., "9.8.2").
    pub version: String,
    /// Parsed version for comparison.
    #[serde(skip)]
    pub parsed: Option<Version>,
    /// Whether this is the recommended version.
    pub is_recommended: bool,
    /// Whether this version is installed.
    pub is_installed: bool,
}

impl GhcVersion {
    /// Create a new GhcVersion.
    pub fn new(version: impl Into<String>) -> Self {
        let version = version.into();
        let parsed = version.parse().ok();
        let is_recommended = version == RECOMMENDED_GHC_VERSION;

        Self {
            version,
            parsed,
            is_recommended,
            is_installed: false,
        }
    }

    /// Mark this version as installed.
    pub fn with_installed(mut self, installed: bool) -> Self {
        self.is_installed = installed;
        self
    }

    /// Get the major.minor version (e.g., "9.8" from "9.8.2").
    pub fn major_minor(&self) -> String {
        if let Some(ref parsed) = self.parsed {
            format!("{}.{}", parsed.major, parsed.minor)
        } else {
            // Fallback: split on '.' and take first two parts
            let parts: Vec<&str> = self.version.split('.').collect();
            if parts.len() >= 2 {
                format!("{}.{}", parts[0], parts[1])
            } else {
                self.version.clone()
            }
        }
    }

    /// Check if this version matches a requirement.
    ///
    /// - "9.8" matches any 9.8.x version
    /// - "9.8.2" matches only 9.8.2
    pub fn matches(&self, requirement: &str) -> bool {
        if requirement == self.version {
            return true;
        }

        // Check if requirement is just major.minor
        let req_parts: Vec<&str> = requirement.split('.').collect();
        if req_parts.len() == 2 {
            return self.major_minor() == requirement;
        }

        false
    }
}

impl std::fmt::Display for GhcVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.version)
    }
}

impl Ord for GhcVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (&self.parsed, &other.parsed) {
            (Some(a), Some(b)) => b.cmp(a), // Reverse for newest first
            _ => other.version.cmp(&self.version),
        }
    }
}

impl PartialOrd for GhcVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Get all known GHC versions.
pub fn known_versions() -> Vec<GhcVersion> {
    KNOWN_GHC_VERSIONS
        .iter()
        .map(|v| GhcVersion::new(*v))
        .collect()
}

/// Check if a version string is in the known list.
pub fn is_known_version(version: &str) -> bool {
    KNOWN_GHC_VERSIONS.contains(&version)
}

/// Validate a GHC version string format.
pub fn is_valid_version(version: &str) -> bool {
    // Must match pattern like 9.8.2 or 9.10.1
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return false;
    }

    parts.iter().all(|p| p.parse::<u32>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_versions() {
        let versions = known_versions();
        assert!(!versions.is_empty());
        assert!(versions.iter().any(|v| v.version == "9.8.2"));
    }

    #[test]
    fn test_is_known_version() {
        assert!(is_known_version("9.8.2"));
        assert!(is_known_version("9.6.4"));
        assert!(!is_known_version("1.0.0"));
    }

    #[test]
    fn test_ghc_version_new() {
        let v = GhcVersion::new(RECOMMENDED_GHC_VERSION);
        assert_eq!(v.version, RECOMMENDED_GHC_VERSION);
        assert!(v.is_recommended);
        assert!(!GhcVersion::new("9.8.2").is_recommended);
        assert!(!v.is_installed);
    }

    #[test]
    fn test_major_minor() {
        let v = GhcVersion::new("9.8.2");
        assert_eq!(v.major_minor(), "9.8");

        let v = GhcVersion::new("9.10.1");
        assert_eq!(v.major_minor(), "9.10");
    }

    #[test]
    fn test_matches() {
        let v = GhcVersion::new("9.8.2");
        assert!(v.matches("9.8.2"));
        assert!(v.matches("9.8"));
        assert!(!v.matches("9.6"));
        assert!(!v.matches("9.8.1"));
    }

    #[test]
    fn test_is_valid_version() {
        assert!(is_valid_version("9.8.2"));
        assert!(is_valid_version("9.10.1"));
        assert!(is_valid_version("9.8"));
        assert!(!is_valid_version("9"));
        assert!(!is_valid_version("9.8.2.1"));
        assert!(!is_valid_version("abc"));
        assert!(!is_valid_version("9.x.2"));
    }

    #[test]
    fn test_version_ordering() {
        let mut versions = [
            GhcVersion::new("9.6.4"),
            GhcVersion::new("9.8.2"),
            GhcVersion::new("9.10.1"),
        ];
        versions.sort();

        // Should be sorted newest first
        assert_eq!(versions[0].version, "9.10.1");
        assert_eq!(versions[1].version, "9.8.2");
        assert_eq!(versions[2].version, "9.6.4");
    }
}
