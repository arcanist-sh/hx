//! Version parsing and comparison.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// A semantic version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// Fourth component of 4-part versions like cabal's `3.12.1.0`.
    ///
    /// Kept separate from `patch`: folding it in (e.g. `patch * 100 + fourth`)
    /// makes distinct versions like 3.12.1.150 and 3.12.2.50 compare equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fourth: Option<u32>,
    pub pre: Option<String>,
}

impl Version {
    /// Create a new version.
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            fourth: None,
            pre: None,
        }
    }

    /// Create a new 4-part version (e.g. cabal's 3.12.1.0).
    pub fn new4(major: u32, minor: u32, patch: u32, fourth: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            fourth: Some(fourth),
            pre: None,
        }
    }

    /// Check if this version is compatible with another (same major.minor).
    pub fn is_compatible_with(&self, other: &Version) -> bool {
        self.major == other.major && self.minor == other.minor
    }

    /// Parse version from tool output like "ghc 9.8.2" or "cabal 3.12.1.0".
    pub fn parse_from_output(output: &str) -> Option<Self> {
        // Try to find a version pattern in the output
        let version_pattern = regex_lite::Regex::new(r"(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?").ok()?;

        if let Some(captures) = version_pattern.captures(output) {
            let major: u32 = captures.get(1)?.as_str().parse().ok()?;
            let minor: u32 = captures.get(2)?.as_str().parse().ok()?;
            let patch: u32 = captures.get(3)?.as_str().parse().ok()?;
            // If there's a 4th component (like cabal 3.12.1.0), keep it separate
            if let Some(fourth) = captures.get(4) {
                let fourth_num: u32 = fourth.as_str().parse().ok()?;
                return Some(Self::new4(major, minor, patch, fourth_num));
            }
            return Some(Self::new(major, minor, patch));
        }

        None
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(fourth) = self.fourth {
            write!(f, ".{}", fourth)?;
        }
        if let Some(ref pre) = self.pre {
            write!(f, "-{}", pre)?;
        }
        Ok(())
    }
}

impl FromStr for Version {
    type Err = VersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        // Split off pre-release suffix
        let (version_part, pre) = if let Some(idx) = s.find('-') {
            (&s[..idx], Some(s[idx + 1..].to_string()))
        } else {
            (s, None)
        };

        let parts: Vec<&str> = version_part.split('.').collect();
        if parts.len() < 2 {
            return Err(VersionParseError::InvalidFormat(s.to_string()));
        }

        let major = parts[0]
            .parse()
            .map_err(|_| VersionParseError::InvalidNumber(parts[0].to_string()))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| VersionParseError::InvalidNumber(parts[1].to_string()))?;
        let patch = if parts.len() > 2 {
            parts[2]
                .parse()
                .map_err(|_| VersionParseError::InvalidNumber(parts[2].to_string()))?
        } else {
            0
        };
        let fourth = if parts.len() > 3 {
            Some(
                parts[3]
                    .parse()
                    .map_err(|_| VersionParseError::InvalidNumber(parts[3].to_string()))?,
            )
        } else {
            None
        };

        Ok(Version {
            major,
            minor,
            patch,
            fourth,
            pre,
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.major.cmp(&other.major) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.minor.cmp(&other.minor) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.patch.cmp(&other.patch) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // Option ordering (None < Some) keeps Ord consistent with derived Eq
        match self.fourth.cmp(&other.fourth) {
            Ordering::Equal => {}
            ord => return ord,
        }
        // Pre-release versions are less than release versions
        match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}

/// Error parsing a version string.
#[derive(Debug, thiserror::Error)]
pub enum VersionParseError {
    #[error("invalid version format: {0}")]
    InvalidFormat(String),
    #[error("invalid version number: {0}")]
    InvalidNumber(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!("9.8.2".parse::<Version>().unwrap(), Version::new(9, 8, 2));
        assert_eq!("1.0.0".parse::<Version>().unwrap(), Version::new(1, 0, 0));
    }

    #[test]
    fn test_parse_from_output() {
        assert_eq!(
            Version::parse_from_output(
                "The Glorious Glasgow Haskell Compilation System, version 9.8.2"
            ),
            Some(Version::new(9, 8, 2))
        );
        assert_eq!(
            Version::parse_from_output("cabal-install version 3.12.1.0"),
            Some(Version::new4(3, 12, 1, 0))
        );
    }

    #[test]
    fn test_four_part_versions_not_lossy() {
        let a: Version = "3.12.1.150".parse().unwrap();
        let b: Version = "3.12.2.50".parse().unwrap();
        assert_ne!(a, b);
        assert!(b > a);
        assert_eq!(a.to_string(), "3.12.1.150");
    }

    #[test]
    fn test_version_ordering() {
        assert!(Version::new(9, 8, 2) > Version::new(9, 6, 4));
        assert!(Version::new(9, 8, 2) > Version::new(9, 8, 1));
        assert!(Version::new(10, 0, 0) > Version::new(9, 99, 99));
    }
}
