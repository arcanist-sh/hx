//! Cabal version information and known versions.

use serde::{Deserialize, Serialize};

/// Known stable Cabal versions.
///
/// This list is updated with each hx release and contains tested,
/// working versions. Users can still install versions not in this list.
pub static KNOWN_CABAL_VERSIONS: &[&str] = &[
    // Cabal 3.18.x
    "3.18.2.0", "3.18.1.0", //
    // Cabal 3.16.x
    "3.16.1.0", "3.16.0.0", //
    // Cabal 3.14.x
    "3.14.2.0", "3.14.1.1", "3.14.1.0", //
    // Cabal 3.12.x
    "3.12.1.0", //
    // Cabal 3.10.x
    "3.10.3.0", "3.10.2.1", "3.10.2.0", "3.10.1.0", //
    // Cabal 3.8.x
    "3.8.1.0", //
    // Cabal 3.6.x
    "3.6.2.0",
];

/// The recommended Cabal version for new projects.
///
/// Tracks ghcup's recommended release.
pub static RECOMMENDED_CABAL_VERSION: &str = "3.16.1.0";

/// Cabal version with metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CabalVersion {
    /// The version string (e.g., "3.12.1.0").
    pub version: String,
    /// Whether this is the recommended version.
    pub is_recommended: bool,
    /// Whether this version is installed.
    pub is_installed: bool,
}

impl CabalVersion {
    /// Create a new CabalVersion.
    pub fn new(version: impl Into<String>) -> Self {
        let version = version.into();
        let is_recommended = version == RECOMMENDED_CABAL_VERSION;

        Self {
            version,
            is_recommended,
            is_installed: false,
        }
    }

    /// Mark this version as installed.
    pub fn with_installed(mut self, installed: bool) -> Self {
        self.is_installed = installed;
        self
    }

    /// Get the major.minor.patch version (e.g., "3.12.1" from "3.12.1.0").
    pub fn major_minor_patch(&self) -> String {
        let parts: Vec<&str> = self.version.split('.').collect();
        if parts.len() >= 3 {
            format!("{}.{}.{}", parts[0], parts[1], parts[2])
        } else {
            self.version.clone()
        }
    }

    /// Check if this version matches a requirement.
    ///
    /// - "3.12" matches any 3.12.x.x version
    /// - "3.12.1" matches any 3.12.1.x version
    /// - "3.12.1.0" matches only 3.12.1.0
    pub fn matches(&self, requirement: &str) -> bool {
        if requirement == self.version {
            return true;
        }

        // Check if version starts with requirement followed by a dot
        self.version.starts_with(requirement)
            && self.version.chars().nth(requirement.len()) == Some('.')
    }
}

impl std::fmt::Display for CabalVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.version)
    }
}

impl Ord for CabalVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare version parts numerically
        let self_parts: Vec<u32> = self
            .version
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();
        let other_parts: Vec<u32> = other
            .version
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        // Reverse order for newest first
        other_parts.cmp(&self_parts)
    }
}

impl PartialOrd for CabalVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Get all known Cabal versions.
pub fn known_versions() -> Vec<CabalVersion> {
    KNOWN_CABAL_VERSIONS
        .iter()
        .map(|v| CabalVersion::new(*v))
        .collect()
}

/// Check if a version string is in the known list.
pub fn is_known_version(version: &str) -> bool {
    KNOWN_CABAL_VERSIONS.contains(&version)
}

/// Validate a Cabal version string format.
///
/// Cabal versions have 4 components: major.minor.patch.revision (e.g., 3.12.1.0)
pub fn is_valid_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 || parts.len() > 4 {
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
        assert!(versions.iter().any(|v| v.version == "3.12.1.0"));
    }

    #[test]
    fn test_is_known_version() {
        assert!(is_known_version("3.12.1.0"));
        assert!(is_known_version("3.10.3.0"));
        assert!(!is_known_version("1.0.0.0"));
    }

    #[test]
    fn test_cabal_version_new() {
        let v = CabalVersion::new(RECOMMENDED_CABAL_VERSION);
        assert_eq!(v.version, RECOMMENDED_CABAL_VERSION);
        assert!(v.is_recommended);
        assert!(!CabalVersion::new("3.12.1.0").is_recommended);
        assert!(!v.is_installed);
    }

    #[test]
    fn test_major_minor_patch() {
        let v = CabalVersion::new("3.12.1.0");
        assert_eq!(v.major_minor_patch(), "3.12.1");

        let v = CabalVersion::new("3.10.2.1");
        assert_eq!(v.major_minor_patch(), "3.10.2");
    }

    #[test]
    fn test_matches() {
        let v = CabalVersion::new("3.12.1.0");
        assert!(v.matches("3.12.1.0"));
        assert!(v.matches("3.12.1"));
        assert!(v.matches("3.12"));
        assert!(!v.matches("3.10"));
        assert!(!v.matches("3.12.2"));
    }

    #[test]
    fn test_is_valid_version() {
        assert!(is_valid_version("3.12.1.0"));
        assert!(is_valid_version("3.10.2.1"));
        assert!(is_valid_version("3.12"));
        assert!(!is_valid_version("3"));
        assert!(!is_valid_version("3.12.1.0.0"));
        assert!(!is_valid_version("abc"));
        assert!(!is_valid_version("3.x.1.0"));
    }

    #[test]
    fn test_version_ordering() {
        let mut versions = [
            CabalVersion::new("3.10.3.0"),
            CabalVersion::new("3.12.1.0"),
            CabalVersion::new("3.8.1.0"),
        ];
        versions.sort();

        // Should be sorted newest first
        assert_eq!(versions[0].version, "3.12.1.0");
        assert_eq!(versions[1].version, "3.10.3.0");
        assert_eq!(versions[2].version, "3.8.1.0");
    }
}
