//! Version parsing and comparison for Haskell Package Versioning Policy (PVP).
//!
//! PVP versions have the format: A.B.C.D where:
//! - A.B is the major version (breaking changes)
//! - C is the minor version (non-breaking additions)
//! - D is the patch version (bug fixes)

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// A PVP-compliant version number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Version {
    /// Version components (e.g., [1, 2, 3, 0] for "1.2.3.0")
    pub components: Vec<u32>,
}

impl Version {
    /// Create a new version from components.
    pub fn new(components: Vec<u32>) -> Self {
        Self { components }
    }

    /// Get the major version (first two components in PVP).
    pub fn major(&self) -> (u32, u32) {
        (
            self.components.first().copied().unwrap_or(0),
            self.components.get(1).copied().unwrap_or(0),
        )
    }

    /// Get the minor version (third component).
    pub fn minor(&self) -> u32 {
        self.components.get(2).copied().unwrap_or(0)
    }

    /// Get the patch version (fourth component).
    pub fn patch(&self) -> u32 {
        self.components.get(3).copied().unwrap_or(0)
    }

    /// Check if this version is compatible with another under PVP.
    /// Compatible means same major version (A.B).
    pub fn is_pvp_compatible(&self, other: &Version) -> bool {
        self.major() == other.major()
    }
}

impl FromStr for Version {
    type Err = VersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(VersionParseError::Empty);
        }

        let components: Result<Vec<u32>, _> = s.split('.').map(|c| c.parse::<u32>()).collect();

        match components {
            Ok(c) if c.is_empty() => Err(VersionParseError::Empty),
            Ok(c) => Ok(Version { components: c }),
            Err(_) => Err(VersionParseError::InvalidComponent(s.to_string())),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.components.iter().map(|c| c.to_string()).collect();
        write!(f, "{}", parts.join("."))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare component by component
        let max_len = self.components.len().max(other.components.len());
        for i in 0..max_len {
            let a = self.components.get(i).copied().unwrap_or(0);
            let b = other.components.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Error parsing a version string.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VersionParseError {
    #[error("empty version string")]
    Empty,
    #[error("invalid version component: {0}")]
    InvalidComponent(String),
}

/// A version constraint that can match versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionConstraint {
    /// Any version matches
    Any,
    /// Exact version: == 1.2.3
    Exact(Version),
    /// Greater than: > 1.2.3
    GreaterThan(Version),
    /// Greater than or equal: >= 1.2.3
    GreaterThanOrEqual(Version),
    /// Less than: < 1.2.3
    LessThan(Version),
    /// Less than or equal: <= 1.2.3
    LessThanOrEqual(Version),
    /// PVP caret: ^>= 1.2.3 (means >= 1.2.3 && < 1.3)
    Caret(Version),
    /// Conjunction: constraint && constraint
    And(Box<VersionConstraint>, Box<VersionConstraint>),
    /// Disjunction: constraint || constraint
    Or(Box<VersionConstraint>, Box<VersionConstraint>),
}

impl VersionConstraint {
    /// Check if a version satisfies this constraint.
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            VersionConstraint::Any => true,
            VersionConstraint::Exact(v) => version == v,
            VersionConstraint::GreaterThan(v) => version > v,
            VersionConstraint::GreaterThanOrEqual(v) => version >= v,
            VersionConstraint::LessThan(v) => version < v,
            VersionConstraint::LessThanOrEqual(v) => version <= v,
            VersionConstraint::Caret(v) => {
                // ^>= 1.2.3 means >= 1.2.3 && < 1.3
                let (major_a, major_b) = v.major();
                let (test_a, test_b) = version.major();

                // Must be >= the base version
                if version < v {
                    return false;
                }

                // Must have same major version (A.B in PVP)
                major_a == test_a && major_b == test_b
            }
            VersionConstraint::And(a, b) => a.matches(version) && b.matches(version),
            VersionConstraint::Or(a, b) => a.matches(version) || b.matches(version),
        }
    }

    /// Create an AND constraint.
    pub fn and(self, other: VersionConstraint) -> VersionConstraint {
        VersionConstraint::And(Box::new(self), Box::new(other))
    }

    /// Create an OR constraint.
    pub fn or(self, other: VersionConstraint) -> VersionConstraint {
        VersionConstraint::Or(Box::new(self), Box::new(other))
    }
}

impl fmt::Display for VersionConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionConstraint::Any => write!(f, "*"),
            VersionConstraint::Exact(v) => write!(f, "=={}", v),
            VersionConstraint::GreaterThan(v) => write!(f, ">{}", v),
            VersionConstraint::GreaterThanOrEqual(v) => write!(f, ">={}", v),
            VersionConstraint::LessThan(v) => write!(f, "<{}", v),
            VersionConstraint::LessThanOrEqual(v) => write!(f, "<={}", v),
            VersionConstraint::Caret(v) => write!(f, "^>={}", v),
            VersionConstraint::And(a, b) => write!(f, "{} && {}", a, b),
            VersionConstraint::Or(a, b) => write!(f, "{} || {}", a, b),
        }
    }
}

/// Parse a version constraint string.
///
/// Supports:
/// - `>= 1.2.3`
/// - `< 2.0`
/// - `== 1.2.3.4`
/// - `^>= 1.2.3`
/// - `>= 1.0 && < 2.0`
/// - `>= 1.0 || < 0.5`
pub fn parse_constraint(s: &str) -> Result<VersionConstraint, ConstraintParseError> {
    let s = s.trim();

    if s.is_empty() || s == "-any" {
        return Ok(VersionConstraint::Any);
    }

    // Handle OR (lower precedence)
    if let Some(idx) = s.find("||") {
        let left = parse_constraint(&s[..idx])?;
        let right = parse_constraint(&s[idx + 2..])?;
        return Ok(left.or(right));
    }

    // Handle AND
    if let Some(idx) = s.find("&&") {
        let left = parse_constraint(&s[..idx])?;
        let right = parse_constraint(&s[idx + 2..])?;
        return Ok(left.and(right));
    }

    // Handle single constraint
    parse_single_constraint(s)
}

fn parse_single_constraint(s: &str) -> Result<VersionConstraint, ConstraintParseError> {
    let s = s.trim();

    // Operators longest-first so `^>=`/`>=`/`<=` win over `>`/`<`.
    type OpCtor = fn(Version) -> VersionConstraint;
    let ops: [(&str, OpCtor); 6] = [
        ("^>=", VersionConstraint::Caret),
        (">=", VersionConstraint::GreaterThanOrEqual),
        ("<=", VersionConstraint::LessThanOrEqual),
        ("==", VersionConstraint::Exact),
        (">", VersionConstraint::GreaterThan),
        ("<", VersionConstraint::LessThan),
    ];

    for (op, ctor) in ops {
        let Some(rest) = s.strip_prefix(op) else {
            continue;
        };
        let rest = rest.trim();

        // Cabal wildcard (== only): `== A.B.*` => `>= A.B && < A.(B+1)`.
        if op == "==" && let Some(prefix) = rest.strip_suffix(".*") {
            let lower: Version = prefix.trim().parse()?;
            let upper = wildcard_upper_bound(&lower);
            return Ok(VersionConstraint::GreaterThanOrEqual(lower)
                .and(VersionConstraint::LessThan(upper)));
        }

        // Cabal set notation: `op {v1, v2, ...}` => `op v1 || op v2 || ...`
        // (e.g. `base ^>= {4.14, 4.17}`). A missing closing brace is tolerated.
        if let Some(inner) = rest.strip_prefix('{') {
            let inner = inner.strip_suffix('}').unwrap_or(inner);
            let mut acc: Option<VersionConstraint> = None;
            for part in inner.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let c = ctor(part.parse()?);
                acc = Some(match acc {
                    Some(a) => a.or(c),
                    None => c,
                });
            }
            return acc.ok_or_else(|| ConstraintParseError::InvalidFormat(s.to_string()));
        }

        return Ok(ctor(rest.parse()?));
    }

    // No operator - might be just a version (exact match) or invalid
    Err(ConstraintParseError::InvalidFormat(s.to_string()))
}

/// Compute the exclusive upper bound for a Cabal wildcard constraint.
///
/// `== A.B.*` matches any version with prefix `A.B`, i.e. `>= A.B && < A.(B+1)`.
/// The upper bound increments the last component of the wildcard prefix.
fn wildcard_upper_bound(lower: &Version) -> Version {
    let mut components = lower.components.clone();
    if let Some(last) = components.last_mut() {
        *last += 1;
    }
    Version::new(components)
}

/// Error parsing a version constraint.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConstraintParseError {
    #[error("invalid constraint format: {0}")]
    InvalidFormat(String),
    #[error("invalid version: {0}")]
    InvalidVersion(#[from] VersionParseError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        let v: Version = "1.2.3".parse().unwrap();
        assert_eq!(v.components, vec![1, 2, 3]);
        assert_eq!(v.to_string(), "1.2.3");

        let v: Version = "1.2.3.4".parse().unwrap();
        assert_eq!(v.components, vec![1, 2, 3, 4]);
        assert_eq!(v.major(), (1, 2));
        assert_eq!(v.minor(), 3);
        assert_eq!(v.patch(), 4);
    }

    #[test]
    fn test_version_comparison() {
        let v1: Version = "1.2.3".parse().unwrap();
        let v2: Version = "1.2.4".parse().unwrap();
        let v3: Version = "1.3.0".parse().unwrap();
        let v4: Version = "2.0.0".parse().unwrap();

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
        assert!(v1 < v4);

        let v5: Version = "1.2.3.0".parse().unwrap();
        assert_eq!(v1.cmp(&v5), Ordering::Equal);
    }

    #[test]
    fn test_constraint_exact() {
        let c = parse_constraint("== 1.2.3").unwrap();
        let v1: Version = "1.2.3".parse().unwrap();
        let v2: Version = "1.2.4".parse().unwrap();

        assert!(c.matches(&v1));
        assert!(!c.matches(&v2));
    }

    #[test]
    fn test_constraint_range() {
        let c = parse_constraint(">= 1.0 && < 2.0").unwrap();

        assert!(c.matches(&"1.0".parse().unwrap()));
        assert!(c.matches(&"1.5".parse().unwrap()));
        assert!(c.matches(&"1.9.9".parse().unwrap()));
        assert!(!c.matches(&"0.9".parse().unwrap()));
        assert!(!c.matches(&"2.0".parse().unwrap()));
    }

    #[test]
    fn test_constraint_caret() {
        let c = parse_constraint("^>= 1.2.3").unwrap();

        // Must be >= 1.2.3
        assert!(!c.matches(&"1.2.2".parse().unwrap()));
        assert!(c.matches(&"1.2.3".parse().unwrap()));
        assert!(c.matches(&"1.2.4".parse().unwrap()));

        // Must have same major (1.2.x)
        assert!(!c.matches(&"1.3.0".parse().unwrap()));
        assert!(!c.matches(&"2.0.0".parse().unwrap()));
    }

    #[test]
    fn test_constraint_or() {
        let c = parse_constraint(">= 2.0 || < 1.0").unwrap();

        assert!(c.matches(&"0.5".parse().unwrap()));
        assert!(!c.matches(&"1.5".parse().unwrap()));
        assert!(c.matches(&"2.0".parse().unwrap()));
        assert!(c.matches(&"3.0".parse().unwrap()));
    }

    #[test]
    fn test_constraint_any() {
        let c = parse_constraint("-any").unwrap();
        assert!(c.matches(&"1.0".parse().unwrap()));
        assert!(c.matches(&"999.0".parse().unwrap()));
    }

    #[test]
    fn test_constraint_wildcard() {
        // `== 0.5.*` desugars to `>= 0.5 && < 0.6`.
        let c = parse_constraint("==0.5.*").unwrap();
        assert!(!c.matches(&"0.4.9".parse().unwrap()));
        assert!(c.matches(&"0.5".parse().unwrap()));
        assert!(c.matches(&"0.5.7".parse().unwrap()));
        assert!(!c.matches(&"0.6".parse().unwrap()));

        // `== 1.24.*` desugars to `>= 1.24 && < 1.25`.
        let c = parse_constraint("== 1.24.*").unwrap();
        assert!(c.matches(&"1.24.0".parse().unwrap()));
        assert!(!c.matches(&"1.25".parse().unwrap()));
    }

    #[test]
    fn test_constraint_set_notation() {
        // `^>= {4.14, 4.17}` desugars to `^>=4.14 || ^>=4.17`.
        let c = parse_constraint("^>= {4.14, 4.17}").unwrap();
        assert!(c.matches(&"4.14.0".parse().unwrap()));
        assert!(c.matches(&"4.17.2".parse().unwrap()));
        // Between the two caret ranges (>=4.15 but not ^4.14 or ^4.17): excluded.
        assert!(!c.matches(&"4.15.0".parse().unwrap()));
        // A missing closing brace is tolerated.
        let c2 = parse_constraint("== {1.0, 2.0").unwrap();
        assert!(c2.matches(&"1.0".parse().unwrap()));
        assert!(c2.matches(&"2.0".parse().unwrap()));
        assert!(!c2.matches(&"1.5".parse().unwrap()));
    }

    #[test]
    fn test_constraint_wildcard_disjunction() {
        // A real Hackage pattern: `== 1.0.* || == 1.2.*`.
        let c = parse_constraint("== 1.0.* || == 1.2.*").unwrap();
        assert!(c.matches(&"1.0.3".parse().unwrap()));
        assert!(!c.matches(&"1.1.0".parse().unwrap()));
        assert!(c.matches(&"1.2.5".parse().unwrap()));
        assert!(!c.matches(&"1.3.0".parse().unwrap()));
    }
}
