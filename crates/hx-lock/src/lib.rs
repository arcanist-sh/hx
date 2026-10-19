//! Lockfile management for hx.
//!
//! This crate handles:
//! - Reading and writing hx.lock (format v2, with backward-compatible reading
//!   of v1 lockfiles)
//! - Converting from Cabal freeze/plan
//! - Converting a lockfile into Cabal constraints for a reproducible build
//! - Fingerprint calculation

use chrono::{DateTime, Utc};
use hx_core::hash::{Digest, Sha256, to_hex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

/// Error type for lock operations.
#[derive(Debug, Error)]
pub enum LockError {
    #[error("failed to read lockfile at {path}: {source}")]
    ReadError {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse lockfile: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("failed to serialize lockfile: {0}")]
    SerializeError(#[from] toml::ser::Error),

    #[error("lockfile version mismatch: expected {expected}, found {found}")]
    VersionMismatch { expected: u32, found: u32 },

    #[error(
        "lockfile format v{found} is newer than this hx supports (up to v{supported}); \
         upgrade hx to read it"
    )]
    UnsupportedVersion { found: u32, supported: u32 },
}

/// Current lockfile format version (the version `hx lock` writes).
///
/// History:
/// - v1: package names and versions, toolchain, platform.
/// - v2: per-package source hashes (`sha256`), Hackage `.cabal` revision
///   pins (`revision` + `cabal_sha256`), flags, git/path sources, BHC
///   toolchain + profile, and the Hackage `index_state` used for resolution.
pub const LOCK_VERSION: u32 = 2;

/// Oldest lockfile format version this crate can still read.
pub const MIN_LOCK_VERSION: u32 = 1;

/// Where a locked package comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A Hackage sdist (`source = "hackage"`).
    Hackage,
    /// A Hackage sdist pinned by a Stackage snapshot (`source = "stackage"`).
    Stackage,
    /// A git checkout pinned to a commit (`source = "git"`).
    Git,
    /// A local directory (`source = "path"`).
    Path,
    /// Shipped with the compiler, never fetched (`source = "ghc"`).
    Installed,
    /// Anything else (written by a newer hx); treated as opaque.
    Other,
}

/// A locked package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPackage {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Source (hackage, stackage, git, path, ghc)
    #[serde(default = "default_source")]
    pub source: String,
    /// SHA-256 of the source tarball, lowercase hex (Hackage/Stackage sources).
    ///
    /// v1 lockfiles called this field `hash` (optionally `sha256:`-prefixed);
    /// both spellings are accepted when reading.
    #[serde(default, alias = "hash", skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Hackage `.cabal` metadata revision (0 = the originally uploaded file).
    ///
    /// Hackage trustees can publish revised `.cabal` files (usually bound
    /// changes) without changing the tarball, so the revision is pinned
    /// separately from `sha256`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u32>,
    /// SHA-256 of the pinned `.cabal` revision, lowercase hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cabal_sha256: Option<String>,
    /// Cabal flag assignments (`true` = `+flag`, `false` = `-flag`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, bool>,
    /// Git repository URL (`source = "git"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Git commit (`source = "git"`), a full 40-character SHA-1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Subdirectory of the git repository holding the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<String>,
    /// Local directory, relative to the project root (`source = "path"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

fn default_source() -> String {
    "hackage".to_string()
}

impl LockedPackage {
    /// A Hackage package with no hash information yet.
    pub fn hackage(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            source: default_source(),
            sha256: None,
            revision: None,
            cabal_sha256: None,
            flags: BTreeMap::new(),
            git: None,
            rev: None,
            subdir: None,
            path: None,
        }
    }

    /// Set the source string (builder style).
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// `name-version`, the Hackage package identifier.
    pub fn package_id(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }

    /// Classify the `source` string.
    pub fn source_kind(&self) -> SourceKind {
        match self.source.as_str() {
            "hackage" => SourceKind::Hackage,
            "stackage" => SourceKind::Stackage,
            "git" => SourceKind::Git,
            "path" | "local" => SourceKind::Path,
            "ghc" | "installed" => SourceKind::Installed,
            _ => SourceKind::Other,
        }
    }

    /// Whether this package is a Hackage sdist that a build may need to
    /// download (and that `hx fetch` therefore caches and verifies).
    ///
    /// Compiler-internal packages such as `base` are never built from source,
    /// so they are excluded even when recorded with a Hackage source.
    pub fn is_fetchable_sdist(&self) -> bool {
        matches!(
            self.source_kind(),
            SourceKind::Hackage | SourceKind::Stackage
        ) && !is_non_reinstallable(&self.name)
    }

    /// Cabal flag assignments in Cabal syntax, e.g. `+ordered-keymap -cffi`.
    pub fn flag_string(&self) -> Option<String> {
        if self.flags.is_empty() {
            return None;
        }
        Some(
            self.flags
                .iter()
                .map(|(f, on)| format!("{}{}", if *on { '+' } else { '-' }, f))
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// Whether `name` is a package that ships with (and is tied to) the compiler,
/// so it can never be rebuilt from a Hackage sdist. Mirrors
/// `hx_solver::resolver::is_ghc_builtin` (plus the legacy `integer-simple`).
pub fn is_non_reinstallable(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "ghc"
            | "ghc-bignum"
            | "ghc-boot"
            | "ghc-boot-th"
            | "ghc-experimental"
            | "ghc-heap"
            | "ghc-internal"
            | "ghc-prim"
            | "ghci"
            | "integer-gmp"
            | "integer-simple"
            | "libiserv"
            | "rts"
            | "system-cxx-std-lib"
            | "template-haskell"
    )
}

/// Toolchain section of the lockfile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedToolchain {
    /// GHC version
    pub ghc: Option<String>,
    /// Cabal version
    pub cabal: Option<String>,
    /// BHC version (only for projects using the BHC backend)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bhc: Option<String>,
    /// BHC optimization profile (only for projects using the BHC backend)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bhc_profile: Option<String>,
}

/// Build plan section of the lockfile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPlan {
    /// Compiler the plan was resolved for (e.g., "ghc-9.8.2"); `.cabal`
    /// conditionals such as `impl(ghc >= 9.6)` were evaluated against it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler_id: Option<String>,
    /// Platform the plan was resolved for (e.g., "aarch64-macos"); `os(...)`
    /// and `arch(...)` conditionals were evaluated against it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Hackage index state (RFC 3339 timestamp) the plan was resolved
    /// against. Passed to Cabal as `--index-state` so builds see the same
    /// `.cabal` revisions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_state: Option<String>,
    /// Stackage snapshot (e.g., "lts-24.64", "nightly-2026-10-19")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// BHC Platform snapshot (e.g., "bhc-platform-2026.1")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bhc_platform: Option<String>,
    /// Overall plan hash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// A workspace package (local package in the workspace).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspacePackageInfo {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Path relative to workspace root
    pub path: String,
}

/// Workspace section of the lockfile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedWorkspace {
    /// Whether this lockfile represents a workspace
    #[serde(default)]
    pub is_workspace: bool,
    /// Workspace packages (local packages)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<WorkspacePackageInfo>,
}

/// The hx.lock lockfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile format version.
    ///
    /// After [`Lockfile::parse`] this is the version *found in the file*
    /// (so callers can tell an old lockfile apart, see
    /// [`Lockfile::needs_upgrade`]); serialization always writes
    /// [`LOCK_VERSION`].
    pub version: u32,
    /// When the lockfile was created/updated
    pub created_at: DateTime<Utc>,
    /// Toolchain versions
    #[serde(default)]
    pub toolchain: LockedToolchain,
    /// Build plan metadata
    #[serde(default)]
    pub plan: LockedPlan,
    /// Workspace information (for multi-package projects)
    #[serde(default, skip_serializing_if = "is_default_workspace")]
    pub workspace: LockedWorkspace,
    /// Locked packages (external dependencies)
    #[serde(default)]
    pub packages: Vec<LockedPackage>,
}

fn is_default_workspace(w: &LockedWorkspace) -> bool {
    !w.is_workspace && w.packages.is_empty()
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

impl Lockfile {
    /// Create a new empty lockfile.
    pub fn new() -> Self {
        Self {
            version: LOCK_VERSION,
            created_at: Utc::now(),
            toolchain: LockedToolchain::default(),
            plan: LockedPlan::default(),
            workspace: LockedWorkspace::default(),
            packages: Vec::new(),
        }
    }

    /// Parse a lockfile from a TOML string.
    ///
    /// Accepts every format from [`MIN_LOCK_VERSION`] to [`LOCK_VERSION`].
    /// Older formats are upgraded in memory (v1 `hash = "sha256:..."` becomes
    /// `sha256 = "..."`); `version` keeps the value found in the file until
    /// the lockfile is written back.
    pub fn parse(s: &str) -> Result<Self, LockError> {
        let mut lock: Lockfile = toml::from_str(s)?;
        if lock.version > LOCK_VERSION {
            return Err(LockError::UnsupportedVersion {
                found: lock.version,
                supported: LOCK_VERSION,
            });
        }
        if lock.version < MIN_LOCK_VERSION {
            return Err(LockError::VersionMismatch {
                expected: LOCK_VERSION,
                found: lock.version,
            });
        }
        for pkg in &mut lock.packages {
            pkg.sha256 = pkg.sha256.take().map(|h| normalize_sha256(&h));
            pkg.cabal_sha256 = pkg.cabal_sha256.take().map(|h| normalize_sha256(&h));
        }
        Ok(lock)
    }

    /// Parse a lockfile from a file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, LockError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| LockError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;
        Self::parse(&content)
    }

    /// Whether this lockfile was read from an older format and should be
    /// regenerated with `hx lock`.
    pub fn needs_upgrade(&self) -> bool {
        self.version < LOCK_VERSION
    }

    /// Serialize the lockfile to a TOML string.
    ///
    /// The output is stable for clean diffs: the current format version is
    /// written, packages are sorted by name (then version), and flags are
    /// sorted by name.
    pub fn to_string(&self) -> Result<String, LockError> {
        Ok(toml::to_string_pretty(&self.normalized())?)
    }

    /// A copy in canonical form: current format version, sorted packages.
    fn normalized(&self) -> Lockfile {
        let mut lock = self.clone();
        lock.version = LOCK_VERSION;
        lock.packages
            .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));
        lock.workspace.packages.sort_by(|a, b| a.name.cmp(&b.name));
        lock
    }

    /// Whether two lockfiles pin exactly the same things (ignoring
    /// `created_at` and the format version they were read from).
    pub fn same_content(&self, other: &Lockfile) -> bool {
        let a = self.normalized();
        let b = other.normalized();
        a.toolchain == b.toolchain
            && a.plan == b.plan
            && a.workspace == b.workspace
            && a.packages == b.packages
    }

    /// Write the lockfile to a file (atomically, to survive interruption).
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), LockError> {
        let path = path.as_ref();
        let content = self.to_string()?;
        hx_core::atomic_write(path, content).map_err(|e| LockError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    /// Calculate a fingerprint for the lockfile.
    ///
    /// Fields that are absent (as in v1 lockfiles) contribute nothing, so a
    /// v1 lockfile keeps the fingerprint it always had.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();

        // Include toolchain
        if let Some(ref ghc) = self.toolchain.ghc {
            hasher.update(format!("ghc:{}", ghc));
        }
        if let Some(ref cabal) = self.toolchain.cabal {
            hasher.update(format!("cabal:{}", cabal));
        }
        if let Some(ref bhc) = self.toolchain.bhc {
            hasher.update(format!("bhc:{}", bhc));
        }
        if let Some(ref profile) = self.toolchain.bhc_profile {
            hasher.update(format!("bhc-profile:{}", profile));
        }

        // Include plan metadata
        if let Some(ref platform) = self.plan.platform {
            hasher.update(format!("platform:{}", platform));
        }
        if let Some(ref index_state) = self.plan.index_state {
            hasher.update(format!("index:{}", index_state));
        }
        if let Some(ref snapshot) = self.plan.snapshot {
            hasher.update(format!("snapshot:{}", snapshot));
        }
        if let Some(ref platform) = self.plan.bhc_platform {
            hasher.update(format!("bhc-platform:{}", platform));
        }

        // Include workspace packages (sorted for determinism)
        if self.workspace.is_workspace {
            hasher.update("workspace:true");
            let mut workspace_pkgs: Vec<_> = self.workspace.packages.iter().collect();
            workspace_pkgs.sort_by(|a, b| a.name.cmp(&b.name));
            for pkg in workspace_pkgs {
                hasher.update(format!("local:{}@{}:{}", pkg.name, pkg.version, pkg.path));
            }
        }

        // Include external packages (sorted for determinism)
        let mut packages: Vec<_> = self.packages.iter().collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        for pkg in packages {
            hasher.update(format!("{}@{}", pkg.name, pkg.version));
            if let Some(ref sha) = pkg.sha256 {
                hasher.update(format!(":sha256={}", sha));
            }
            if let Some(rev) = pkg.revision {
                hasher.update(format!(":rev={}", rev));
            }
            if let Some(ref sha) = pkg.cabal_sha256 {
                hasher.update(format!(":cabal={}", sha));
            }
            if let Some(flags) = pkg.flag_string() {
                hasher.update(format!(":flags={}", flags));
            }
            if let Some(ref git) = pkg.git {
                hasher.update(format!(":git={}@{}", git, pkg.rev.as_deref().unwrap_or("")));
            }
            if let Some(ref path) = pkg.path {
                hasher.update(format!(":path={}", path));
            }
        }

        let result = hasher.finalize();
        format!("sha256:{}", to_hex(result))
    }

    /// Add a package to the lockfile.
    pub fn add_package(&mut self, pkg: LockedPackage) {
        // Remove existing package with same name
        self.packages.retain(|p| p.name != pkg.name);
        self.packages.push(pkg);
    }

    /// Look up a locked package by name.
    pub fn package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Set the toolchain versions.
    pub fn set_toolchain(&mut self, ghc: Option<String>, cabal: Option<String>) {
        self.toolchain.ghc = ghc;
        self.toolchain.cabal = cabal;
    }

    /// Set the Stackage snapshot.
    pub fn set_snapshot(&mut self, snapshot: Option<String>) {
        self.plan.snapshot = snapshot;
    }

    /// Set workspace information.
    pub fn set_workspace(&mut self, packages: Vec<WorkspacePackageInfo>) {
        self.workspace.is_workspace = !packages.is_empty();
        self.workspace.packages = packages;
    }

    /// Check if this is a workspace lockfile.
    pub fn is_workspace(&self) -> bool {
        self.workspace.is_workspace
    }

    /// Get the workspace package names.
    pub fn workspace_package_names(&self) -> Vec<&str> {
        self.workspace
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect()
    }

    /// Packages that are Hackage sdists a build may need (see
    /// [`LockedPackage::is_fetchable_sdist`]), sorted by name.
    pub fn fetchable_sdists(&self) -> Vec<&LockedPackage> {
        let mut pkgs: Vec<_> = self
            .packages
            .iter()
            .filter(|p| p.is_fetchable_sdist())
            .collect();
        pkgs.sort_by(|a, b| a.name.cmp(&b.name));
        pkgs
    }

    /// Cabal command-line arguments that pin a build to this lockfile:
    /// one `--constraint` per locked version and flag assignment, plus
    /// `--index-state` when the resolution index state is known.
    ///
    /// Compiler-internal packages (`base`, `ghc-prim`, ...) are left
    /// unconstrained: they are fixed by the GHC version, which is checked
    /// separately, and a constraint on them only produces an obscure solver
    /// error.
    pub fn cabal_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(ref index_state) = self.plan.index_state {
            args.push(format!("--index-state={}", index_state));
        }
        let mut packages: Vec<_> = self
            .packages
            .iter()
            .filter(|p| !is_non_reinstallable(&p.name))
            .filter(|p| p.source_kind() != SourceKind::Path)
            .collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        for pkg in packages {
            args.push(format!("--constraint=any.{} =={}", pkg.name, pkg.version));
            if let Some(flags) = pkg.flag_string() {
                args.push(format!("--constraint={} {}", pkg.name, flags));
            }
        }
        args
    }
}

/// Normalize a SHA-256 digest: drop an optional `sha256:` prefix and
/// lowercase the hex.
pub fn normalize_sha256(s: &str) -> String {
    s.trim()
        .strip_prefix("sha256:")
        .unwrap_or(s.trim())
        .to_ascii_lowercase()
}

/// Parse a Cabal freeze file and extract constraints.
///
/// Understands both the legacy `pkg ==version` form and the `any.pkg ==version`
/// form modern Cabal writes, and attaches flag lines (`pkg +flag -other`) to
/// their package.
pub fn parse_freeze_file(content: &str) -> Vec<LockedPackage> {
    let mut packages: Vec<LockedPackage> = Vec::new();
    let mut flags: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with("--") {
            continue;
        }

        // Look for constraint lines like "constraints: pkg ==version"
        // or "             pkg ==version,"
        let constraint = line
            .strip_prefix("constraints:")
            .unwrap_or(line)
            .trim()
            .trim_end_matches(',');

        if let Some((name, version)) = parse_constraint(constraint) {
            if !packages.iter().any(|p| p.name == name) {
                packages.push(LockedPackage::hackage(name, version));
            }
        } else if let Some((name, pkg_flags)) = parse_flag_constraint(constraint) {
            flags.entry(name).or_default().extend(pkg_flags);
        }
    }

    for pkg in &mut packages {
        if let Some(f) = flags.remove(&pkg.name) {
            pkg.flags = f;
        }
    }

    packages
}

/// Extract the `index-state:` timestamp from a Cabal freeze/project file.
///
/// Handles both `index-state: 2024-01-15T00:00:00Z` and the per-repository
/// form `index-state: hackage.haskell.org 2024-01-15T00:00:00Z`.
pub fn parse_freeze_index_state(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("index-state:")?;
        rest.split([' ', ','])
            .rfind(|t| !t.is_empty())
            .map(str::to_string)
    })
}

fn parse_constraint(s: &str) -> Option<(String, String)> {
    // Parse "pkg ==version", "any.pkg ==version" or "pkg ==version,"
    let s = s.trim().trim_end_matches(',');
    let parts: Vec<&str> = s.split(" ==").collect();
    if parts.len() == 2 {
        let name = parts[0].trim();
        let name = name.strip_prefix("any.").unwrap_or(name);
        let version = parts[1].trim();
        if !name.is_empty() && !version.is_empty() && !name.contains(char::is_whitespace) {
            return Some((name.to_string(), version.to_string()));
        }
    }
    None
}

fn parse_flag_constraint(s: &str) -> Option<(String, BTreeMap<String, bool>)> {
    // Parse "pkg +flag -other" (optionally "any.pkg ...")
    let mut tokens = s.split_whitespace();
    let name = tokens.next()?;
    let name = name.strip_prefix("any.").unwrap_or(name);
    let mut flags = BTreeMap::new();
    for tok in tokens {
        if let Some(f) = tok.strip_prefix('+') {
            flags.insert(f.to_string(), true);
        } else {
            flags.insert(tok.strip_prefix('-')?.to_string(), false);
        }
    }
    if flags.is_empty() {
        None
    } else {
        Some((name.to_string(), flags))
    }
}

#[cfg(test)]
mod hash_golden_tests {
    //! Lockfile fingerprints decide whether the lockfile is current. Pinned to exact output so a hashing-library upgrade cannot
    //! silently change them.
    use super::*;

    #[test]
    fn fingerprint_is_stable() {
        let mut lock = Lockfile::new();
        lock.set_toolchain(Some("9.8.2".to_string()), Some("3.12.1.0".to_string()));
        lock.plan.platform = Some("aarch64-osx".to_string());
        lock.plan.index_state = Some("2026-01-01T00:00:00Z".to_string());
        lock.plan.snapshot = Some("lts-24.0".to_string());
        lock.set_workspace(vec![WorkspacePackageInfo {
            name: "mylib".to_string(),
            version: "0.1.0".to_string(),
            path: "packages/mylib".to_string(),
        }]);
        for (name, version) in [("text", "2.1.1"), ("aeson", "2.2.3.0")] {
            lock.add_package(LockedPackage::hackage(name, version));
        }
        assert_eq!(
            lock.fingerprint(),
            "sha256:bed5e795c2c9c945b4fc1efd2acd769e4d79db2477da9ad7d83190a2bc37e1c0"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, version: &str) -> LockedPackage {
        LockedPackage::hackage(name, version)
    }

    #[test]
    fn test_lockfile_roundtrip() {
        let mut lock = Lockfile::new();
        lock.set_toolchain(Some("9.8.2".to_string()), Some("3.12.1.0".to_string()));
        lock.add_package(pkg("text", "2.1.1"));

        let toml = lock.to_string().unwrap();
        let parsed = Lockfile::parse(&toml).unwrap();

        assert_eq!(parsed.toolchain.ghc, Some("9.8.2".to_string()));
        assert_eq!(parsed.packages.len(), 1);
        assert_eq!(parsed.packages[0].name, "text");
    }

    /// A fully populated v2 lockfile with a fixed timestamp.
    fn sample_v2() -> Lockfile {
        let mut lock = Lockfile::new();
        lock.created_at = "2026-10-01T12:00:00Z".parse().unwrap();
        lock.toolchain = LockedToolchain {
            ghc: Some("9.8.2".into()),
            cabal: Some("3.12.1.0".into()),
            bhc: Some("2026.2.0".into()),
            bhc_profile: Some("server".into()),
        };
        lock.plan = LockedPlan {
            compiler_id: Some("ghc-9.8.2".into()),
            platform: Some("aarch64-macos".into()),
            index_state: Some("2026-09-30T08:15:00Z".into()),
            snapshot: Some("lts-22.28".into()),
            bhc_platform: None,
            hash: None,
        };
        // Deliberately out of order: serialization must sort.
        let mut aeson = pkg("aeson", "2.2.3.0").with_source("stackage");
        aeson.sha256 = Some("a".repeat(64));
        aeson.revision = Some(2);
        aeson.cabal_sha256 = Some("b".repeat(64));
        aeson.flags.insert("ordered-keymap".into(), true);
        aeson.flags.insert("cffi".into(), false);
        let mut text = pkg("text-short", "0.1.6");
        text.sha256 = Some("c".repeat(64));
        text.revision = Some(0);
        text.cabal_sha256 = Some("d".repeat(64));
        let mut git = pkg("my-fork", "0.3.0").with_source("git");
        git.git = Some("https://github.com/example/my-fork.git".into());
        git.rev = Some("0123456789abcdef0123456789abcdef01234567".into());
        git.subdir = Some("core".into());
        let mut local = pkg("vendored", "1.0").with_source("path");
        local.path = Some("vendor/vendored".into());
        lock.packages = vec![
            text,
            local,
            git,
            aeson,
            pkg("base", "4.19.1.0").with_source("ghc"),
        ];
        lock.plan.hash = Some(lock.fingerprint());
        lock
    }

    #[test]
    fn test_v2_golden_snapshot() {
        let toml = sample_v2().to_string().unwrap();
        insta::assert_snapshot!("lockfile_v2", toml);
    }

    #[test]
    fn test_v2_roundtrip_is_lossless_and_stable() {
        let lock = sample_v2();
        let first = lock.to_string().unwrap();
        let parsed = Lockfile::parse(&first).unwrap();
        assert!(parsed.same_content(&lock));
        assert!(!parsed.needs_upgrade());
        // Re-serializing must be byte-identical (clean diffs).
        assert_eq!(parsed.to_string().unwrap(), first);
        let aeson = parsed.package("aeson").unwrap();
        assert_eq!(aeson.revision, Some(2));
        assert_eq!(
            aeson.flag_string().as_deref(),
            Some("-cffi +ordered-keymap")
        );
        assert_eq!(
            parsed.package("my-fork").unwrap().source_kind(),
            SourceKind::Git
        );
    }

    #[test]
    fn test_v1_lockfile_reads_and_upgrades() {
        let v1 = r#"
version = 1
created_at = "2024-01-15T10:30:00Z"

[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"

[plan]
platform = "aarch64-macos"
hash = "sha256:old"

[[packages]]
name = "text"
version = "2.1.1"
source = "hackage"
hash = "sha256:ABCDEF0123"

[[packages]]
name = "aeson"
version = "2.2.1.0"
"#;
        let lock = Lockfile::parse(v1).unwrap();
        assert_eq!(lock.version, 1);
        assert!(lock.needs_upgrade());
        let text = lock.package("text").unwrap();
        assert_eq!(text.sha256.as_deref(), Some("abcdef0123"));
        assert_eq!(text.revision, None);
        assert_eq!(lock.package("aeson").unwrap().source, "hackage");

        // Writing upgrades the format in place.
        let out = lock.to_string().unwrap();
        assert!(out.starts_with("version = 2\n"), "{out}");
        assert!(out.contains("sha256 = \"abcdef0123\""));
        assert!(!out.contains("hash = \"sha256:ABCDEF0123\""));
        let again = Lockfile::parse(&out).unwrap();
        assert!(!again.needs_upgrade());
        assert!(again.same_content(&lock));
    }

    #[test]
    fn test_v1_fingerprint_unchanged_by_upgrade() {
        // Absent v2 fields must not perturb the fingerprint of v1 data, so
        // build caches keyed on it survive the format upgrade.
        let mut lock = Lockfile::new();
        lock.set_toolchain(Some("9.8.2".into()), None);
        lock.add_package(pkg("text", "2.1.1"));
        let mut hasher = Sha256::new();
        hasher.update("ghc:9.8.2");
        hasher.update("text@2.1.1");
        let expected = format!("sha256:{}", to_hex(hasher.finalize()));
        assert_eq!(lock.fingerprint(), expected);
    }

    #[test]
    fn test_fingerprint_covers_hashes_and_revisions() {
        let base = sample_v2();
        let mut changed = base.clone();
        changed.packages[0].sha256 = Some("e".repeat(64));
        assert_ne!(base.fingerprint(), changed.fingerprint());
        let mut changed = base.clone();
        changed.packages[3].revision = Some(3);
        assert_ne!(base.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn test_newer_version_is_rejected() {
        let err =
            Lockfile::parse("version = 99\ncreated_at = \"2024-01-15T10:30:00Z\"\n").unwrap_err();
        assert!(matches!(
            err,
            LockError::UnsupportedVersion {
                found: 99,
                supported: LOCK_VERSION
            }
        ));
        assert!(err.to_string().contains("upgrade hx"));
    }

    #[test]
    fn test_same_content_ignores_timestamp_and_order() {
        let a = sample_v2();
        let mut b = a.clone();
        b.created_at = Utc::now();
        b.packages.reverse();
        assert!(a.same_content(&b));
        b.packages[0].version = "9.9".into();
        assert!(!a.same_content(&b));
    }

    #[test]
    fn test_cabal_args() {
        let args = sample_v2().cabal_args();
        assert_eq!(
            args,
            vec![
                "--index-state=2026-09-30T08:15:00Z",
                "--constraint=any.aeson ==2.2.3.0",
                "--constraint=aeson -cffi +ordered-keymap",
                "--constraint=any.my-fork ==0.3.0",
                "--constraint=any.text-short ==0.1.6",
            ]
        );
    }

    #[test]
    fn test_fetchable_sdists() {
        let lock = sample_v2();
        let names: Vec<_> = lock.fetchable_sdists().iter().map(|p| &p.name).collect();
        assert_eq!(names, vec!["aeson", "text-short"]);
        assert!(!pkg("base", "4.19").is_fetchable_sdist());
        assert!(!pkg("ghc-prim", "0.11").is_fetchable_sdist());
        assert!(pkg("containers", "0.7").is_fetchable_sdist());
    }

    #[test]
    fn test_workspace_lockfile_roundtrip() {
        let mut lock = Lockfile::new();
        lock.set_toolchain(Some("9.8.2".to_string()), Some("3.12.1.0".to_string()));

        // Set workspace packages
        lock.set_workspace(vec![
            WorkspacePackageInfo {
                name: "mylib".to_string(),
                version: "0.1.0".to_string(),
                path: "packages/mylib".to_string(),
            },
            WorkspacePackageInfo {
                name: "myapp".to_string(),
                version: "0.1.0".to_string(),
                path: "packages/myapp".to_string(),
            },
        ]);

        // Add external dependencies
        lock.add_package(pkg("text", "2.1.1"));

        let toml = lock.to_string().unwrap();
        let parsed = Lockfile::parse(&toml).unwrap();

        assert!(parsed.is_workspace());
        assert_eq!(parsed.workspace.packages.len(), 2);
        assert_eq!(parsed.packages.len(), 1);

        let names = parsed.workspace_package_names();
        assert!(names.contains(&"mylib"));
        assert!(names.contains(&"myapp"));
    }

    #[test]
    fn test_workspace_fingerprint_includes_packages() {
        let mut lock1 = Lockfile::new();
        lock1.set_workspace(vec![WorkspacePackageInfo {
            name: "pkg1".to_string(),
            version: "0.1.0".to_string(),
            path: "packages/pkg1".to_string(),
        }]);

        let mut lock2 = Lockfile::new();
        lock2.set_workspace(vec![WorkspacePackageInfo {
            name: "pkg2".to_string(),
            version: "0.1.0".to_string(),
            path: "packages/pkg2".to_string(),
        }]);

        // Different workspace packages should produce different fingerprints
        assert_ne!(lock1.fingerprint(), lock2.fingerprint());
    }

    #[test]
    fn test_parse_constraint() {
        assert_eq!(
            parse_constraint("text ==2.1.1"),
            Some(("text".to_string(), "2.1.1".to_string()))
        );
        assert_eq!(
            parse_constraint("  aeson ==2.2.0.0,"),
            Some(("aeson".to_string(), "2.2.0.0".to_string()))
        );
        // Modern cabal qualifies every constraint with `any.`
        assert_eq!(
            parse_constraint("any.base ==4.19"),
            Some(("base".to_string(), "4.19".to_string()))
        );
        assert_eq!(parse_constraint("index-state: hackage.haskell.org"), None);
    }

    #[test]
    fn test_parse_modern_freeze_file() {
        let freeze = "\
active-repositories: hackage.haskell.org:merge
constraints: any.aeson ==2.2.1.0,
             aeson -cffi +ordered-keymap,
             any.base ==4.19.1.0,
             any.text ==2.1.1
index-state: hackage.haskell.org 2024-01-15T00:00:00Z
";
        let pkgs = parse_freeze_file(freeze);
        let names: Vec<_> = pkgs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["aeson", "base", "text"]);
        let aeson = &pkgs[0];
        assert_eq!(aeson.flags.get("ordered-keymap"), Some(&true));
        assert_eq!(aeson.flags.get("cffi"), Some(&false));
        assert_eq!(
            parse_freeze_index_state(freeze).as_deref(),
            Some("2024-01-15T00:00:00Z")
        );
        assert_eq!(
            parse_freeze_index_state("index-state: 2023-05-01T00:00:00Z").as_deref(),
            Some("2023-05-01T00:00:00Z")
        );
        assert_eq!(parse_freeze_index_state("constraints: any.a ==1"), None);
    }
}
