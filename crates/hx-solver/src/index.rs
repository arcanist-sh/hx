//! Hackage package index parsing.
//!
//! This module reads the Hackage 01-index.tar.gz file and builds
//! a PackageIndex for dependency resolution.

use crate::cabal::parse_cabal_ctx;
use crate::package::{PackageIndex, PackageVersion};
use crate::version::Version;
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Duration;
use tar::Archive;
use tracing::{debug, info, trace, warn};

/// Error loading the package index.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("failed to open index file: {0}")]
    OpenFile(#[from] std::io::Error),

    #[error("failed to read tar entry: {0}")]
    TarRead(String),

    #[error("index file not found: {0}")]
    NotFound(String),
}

/// Options for loading the index.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Maximum number of packages to load (0 = unlimited)
    pub max_packages: usize,
    /// Only load specific packages (empty = all)
    pub filter_packages: Vec<String>,
    /// Skip packages with parse errors
    pub skip_errors: bool,
    /// Show progress bar during loading
    pub show_progress: bool,
    /// Build context for evaluating `.cabal` conditionals. `None` uses the host
    /// platform with a permissive GHC version (suitable for metadata browsing).
    pub context: Option<crate::condition::CabalContext>,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_packages: 0,
            filter_packages: Vec::new(),
            skip_errors: true,
            show_progress: false,
            context: None,
        }
    }
}

impl IndexOptions {
    /// The effective build context (host platform + a permissive GHC when none
    /// is configured).
    pub fn effective_context(&self) -> crate::condition::CabalContext {
        self.context.clone().unwrap_or_else(|| {
            crate::condition::CabalContext::host(
                "9.99.0"
                    .parse()
                    .unwrap_or_else(|_| crate::version::Version::new(vec![9, 99])),
            )
        })
    }

    /// A stable key identifying the build context, for cache validation.
    pub fn context_key(&self) -> String {
        self.effective_context().cache_key()
    }
}

/// Load a package index from a Hackage 01-index.tar.gz file.
///
/// The index file is typically located at:
/// - Linux: `~/.cabal/packages/hackage.haskell.org/01-index.tar.gz`
/// - macOS: `~/.cabal/packages/hackage.haskell.org/01-index.tar.gz`
pub fn load_index(path: &Path, options: &IndexOptions) -> Result<PackageIndex, IndexError> {
    info!("Loading package index from {}", path.display());

    let file = File::open(path)?;
    let reader = BufReader::new(file);

    // Check if gzipped
    let is_gzipped = path.extension().map(|e| e == "gz").unwrap_or(false);

    if is_gzipped {
        let decoder = GzDecoder::new(reader);
        load_from_tar(decoder, options)
    } else {
        load_from_tar(reader, options)
    }
}

/// Load index from a tar archive reader.
fn load_from_tar<R: Read>(reader: R, options: &IndexOptions) -> Result<PackageIndex, IndexError> {
    let mut archive = Archive::new(reader);
    let mut index = PackageIndex::new();
    // Evaluate each package's `.cabal` conditionals against this context, so
    // platform/compiler-specific dependencies don't leak into the graph.
    let ctx = options.effective_context();

    // Track the latest revision for each package-version
    let mut revisions: HashMap<(String, String), (u32, String)> = HashMap::new();

    // Phase 1: Read entries from tar archive (use spinner since count is unknown)
    let read_pb = if options.show_progress {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message("Reading package index...");
        pb.enable_steady_tick(Duration::from_millis(100));
        Some(pb)
    } else {
        None
    };

    let entries = archive
        .entries()
        .map_err(|e| IndexError::TarRead(e.to_string()))?;

    let mut entry_count = 0;
    for entry in entries {
        let mut entry = match entry {
            Ok(e) => e,
            Err(e) => {
                if options.skip_errors {
                    warn!("Skipping tar entry: {}", e);
                    continue;
                }
                return Err(IndexError::TarRead(e.to_string()));
            }
        };

        let path = match entry.path() {
            Ok(p) => p.to_path_buf(),
            Err(_) => continue,
        };

        // Parse path: package-name/version/package-name.cabal
        let path_str = path.to_string_lossy();
        if !path_str.ends_with(".cabal") {
            continue;
        }

        let parts: Vec<&str> = path_str.split('/').collect();
        if parts.len() < 3 {
            continue;
        }

        let package_name = parts[0];
        let version_str = parts[1];

        // Apply package filter
        if !options.filter_packages.is_empty()
            && !options.filter_packages.iter().any(|p| p == package_name)
        {
            continue;
        }

        // Read the .cabal file content
        let mut content = String::new();
        if let Err(e) = entry.read_to_string(&mut content) {
            if options.skip_errors {
                trace!("Skipping {}: {}", path_str, e);
                continue;
            }
            return Err(IndexError::TarRead(format!(
                "failed to read {}: {}",
                path_str, e
            )));
        }

        // Track revisions - later entries in tar are newer revisions
        let key = (package_name.to_string(), version_str.to_string());
        let revision = revisions.get(&key).map(|(r, _)| r + 1).unwrap_or(0);
        revisions.insert(key, (revision, content));

        entry_count += 1;
        if let Some(ref pb) = read_pb {
            pb.set_message(format!("Reading package index... {} entries", entry_count));
        }

        // Check package limit
        if options.max_packages > 0 && revisions.len() >= options.max_packages {
            debug!("Reached package limit of {}", options.max_packages);
            break;
        }
    }

    if let Some(pb) = read_pb {
        pb.finish_with_message(format!("Read {} cabal entries", entry_count));
    }

    // Phase 2: Parse all the collected .cabal files (known count, use progress bar)
    let total = revisions.len();
    info!("Parsing {} package versions", total);

    let parse_pb = if options.show_progress {
        let pb = ProgressBar::new(total as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} packages parsed")
                .unwrap()
                .progress_chars("#>-"),
        );
        Some(pb)
    } else {
        None
    };

    for ((package_name, version_str), (revision, content)) in revisions {
        let version = match version_str.parse::<Version>() {
            Ok(v) => v,
            Err(e) => {
                trace!(
                    "Skipping {}-{}: invalid version: {}",
                    package_name, version_str, e
                );
                if let Some(ref pb) = parse_pb {
                    pb.inc(1);
                }
                continue;
            }
        };

        let cabal = parse_cabal_ctx(&content, &ctx);

        let mut pv = PackageVersion::new(&package_name, version);
        pv.revision = revision;

        // Add all dependencies
        for dep in cabal.all_dependencies() {
            pv.add_dependency(dep);
        }

        index.add_package_version(pv);

        if let Some(ref pb) = parse_pb {
            pb.inc(1);
        }
    }

    if let Some(pb) = parse_pb {
        pb.finish_with_message(format!(
            "Parsed {} packages ({} versions)",
            index.package_count(),
            index.version_count()
        ));
    }

    info!(
        "Loaded {} packages with {} total versions",
        index.package_count(),
        index.version_count()
    );

    Ok(index)
}

/// Cabal's per-repository download cache for Hackage
/// (`<cabal packages dir>/hackage.haskell.org`).
///
/// This is where `cabal` keeps `01-index.tar` and where it looks for
/// downloaded sdists (`<pkg>/<ver>/<pkg>-<ver>.tar.gz`), including when run
/// with `--offline`. Mirrors cabal-install's own lookup: `$CABAL_DIR/packages`
/// if set, else `~/.cabal/packages` if `~/.cabal` exists, else the XDG cache
/// (`$XDG_CACHE_HOME/cabal/packages`, default `~/.cache/cabal/packages`).
/// A custom `remote-repo-cache` in the cabal config is not consulted.
pub fn cabal_repo_cache_dir() -> Option<std::path::PathBuf> {
    let packages = if let Some(dir) = std::env::var_os("CABAL_DIR").filter(|d| !d.is_empty()) {
        std::path::PathBuf::from(dir).join("packages")
    } else {
        let home = hx_cache::home_dir().ok()?;
        let dot_cabal = home.join(".cabal");
        if dot_cabal.exists() {
            dot_cabal.join("packages")
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .filter(|d| !d.is_empty())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".cache"))
                .join("cabal")
                .join("packages")
        }
    };
    Some(packages.join("hackage.haskell.org"))
}

/// Get the default Hackage index path (cabal's copy of the index).
pub fn default_index_path() -> Option<std::path::PathBuf> {
    let repo = cabal_repo_cache_dir()?;
    ["01-index.tar.gz", "01-index.tar"]
        .iter()
        .map(|name| repo.join(name))
        .find(|path| path.exists())
}

/// Source and metadata hashes the Hackage index records for one
/// package version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexPackageMeta {
    /// SHA-256 of the sdist tarball, from the index's signed
    /// `package.json` (TUF targets) entry.
    pub sdist_sha256: Option<String>,
    /// Size of the sdist tarball in bytes, from the same entry.
    pub sdist_length: Option<u64>,
    /// SHA-256 of every `.cabal` revision, oldest (revision 0) first.
    pub cabal_revisions: Vec<String>,
}

impl IndexPackageMeta {
    /// The latest revision number and its `.cabal` hash.
    pub fn latest_revision(&self) -> Option<(u32, &str)> {
        let last = self.cabal_revisions.last()?;
        Some(((self.cabal_revisions.len() - 1) as u32, last.as_str()))
    }
}

/// What a pass over the Hackage index learned about a set of packages.
#[derive(Debug, Clone, Default)]
pub struct IndexMetadata {
    /// Timestamp of the newest index entry considered: the `index-state`
    /// that reproduces this view of the index.
    pub index_state: Option<chrono::DateTime<chrono::Utc>>,
    /// Metadata keyed by `(name, version)`.
    pub packages: HashMap<(String, String), IndexPackageMeta>,
}

impl IndexMetadata {
    /// Metadata for `name-version`, if the index has it.
    pub fn get(&self, name: &str, version: &str) -> Option<&IndexPackageMeta> {
        self.packages.get(&(name.to_string(), version.to_string()))
    }

    /// `index_state` formatted the way Cabal's `--index-state` expects.
    pub fn index_state_string(&self) -> Option<String> {
        self.index_state
            .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
    }
}

/// Scan a Hackage index for the sdist hashes and `.cabal` revisions of
/// `wanted` packages (`(name, version)` pairs).
///
/// With `cutoff` (a Unix timestamp), entries newer than it are ignored,
/// which is exactly how Cabal applies an `index-state`: the result then
/// describes the revisions a build pinned to that index state will see.
pub fn scan_index_metadata(
    path: &Path,
    wanted: &std::collections::HashSet<(String, String)>,
    cutoff: Option<i64>,
) -> Result<IndexMetadata, IndexError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    if path.extension().map(|e| e == "gz").unwrap_or(false) {
        scan_metadata_from_tar(GzDecoder::new(reader), wanted, cutoff)
    } else {
        scan_metadata_from_tar(reader, wanted, cutoff)
    }
}

fn scan_metadata_from_tar<R: Read>(
    reader: R,
    wanted: &std::collections::HashSet<(String, String)>,
    cutoff: Option<i64>,
) -> Result<IndexMetadata, IndexError> {
    let mut archive = Archive::new(reader);
    let mut meta = IndexMetadata::default();
    let mut newest: Option<i64> = None;

    let entries = archive
        .entries()
        .map_err(|e| IndexError::TarRead(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| IndexError::TarRead(e.to_string()))?;
        let mtime = entry.header().mtime().unwrap_or(0) as i64;
        if cutoff.is_some_and(|c| mtime > c) {
            continue;
        }
        newest = Some(newest.map_or(mtime, |n| n.max(mtime)));

        let path = match entry.path() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => continue,
        };
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 3 {
            continue;
        }
        let key = (parts[0].to_string(), parts[1].to_string());
        if !wanted.contains(&key) {
            continue;
        }

        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|e| IndexError::TarRead(format!("failed to read {}: {}", path, e)))?;

        if parts[2].ends_with(".cabal") {
            meta.packages
                .entry(key)
                .or_default()
                .cabal_revisions
                .push(crate::fetch::sha256_hex(&content));
        } else if parts[2] == "package.json" {
            let tarball = format!("{}-{}.tar.gz", key.0, key.1);
            if let Some((sha, len)) = parse_package_json_target(&content, &tarball) {
                let pkg = meta.packages.entry(key).or_default();
                pkg.sdist_sha256 = Some(sha);
                pkg.sdist_length = len;
            }
        }
    }

    meta.index_state = newest.and_then(|t| chrono::DateTime::from_timestamp(t, 0));
    Ok(meta)
}

/// Extract the sdist hash from an index `package.json` (TUF targets file).
fn parse_package_json_target(content: &[u8], tarball: &str) -> Option<(String, Option<u64>)> {
    let json: serde_json::Value = serde_json::from_slice(content).ok()?;
    let targets = json.get("signed")?.get("targets")?.as_object()?;
    let suffix = format!("/{}", tarball);
    let target = targets
        .iter()
        .find(|(k, _)| k.ends_with(&suffix) || k.as_str() == tarball)
        .map(|(_, v)| v)?;
    let sha = target
        .get("hashes")?
        .get("sha256")?
        .as_str()?
        .to_ascii_lowercase();
    let len = target.get("length").and_then(|l| l.as_u64());
    Some((sha, len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tar::Builder;

    fn create_test_index() -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());

        // Add a simple package
        let cabal_content = br#"name: test-pkg
version: 1.0.0

library
  build-depends: base >= 4.7 && < 5
"#;

        let mut header = tar::Header::new_gnu();
        header.set_path("test-pkg/1.0.0/test-pkg.cabal").unwrap();
        header.set_size(cabal_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        builder.append(&header, &cabal_content[..]).unwrap();

        // Add another version
        let cabal_content2 = br#"name: test-pkg
version: 2.0.0

library
  build-depends: base >= 4.9 && < 5, text >= 1.0
"#;

        let mut header2 = tar::Header::new_gnu();
        header2.set_path("test-pkg/2.0.0/test-pkg.cabal").unwrap();
        header2.set_size(cabal_content2.len() as u64);
        header2.set_mode(0o644);
        header2.set_cksum();

        builder.append(&header2, &cabal_content2[..]).unwrap();

        builder.into_inner().unwrap()
    }

    fn append(builder: &mut Builder<Vec<u8>>, path: &str, content: &[u8], mtime: u64) {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(mtime);
        header.set_cksum();
        builder.append(&header, content).unwrap();
    }

    fn package_json(tarball: &str, sha: &str, len: u64) -> Vec<u8> {
        format!(
            r#"{{"signatures":[],"signed":{{"_type":"Targets","expires":null,"targets":{{"<repo>/package/{tarball}":{{"hashes":{{"md5":"00","sha256":"{sha}"}},"length":{len}}}}},"version":0}}}}"#
        )
        .into_bytes()
    }

    /// An index where `foo-1.0` has two `.cabal` revisions and a signed
    /// sdist hash, plus an unrelated package.
    fn create_revisioned_index() -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        append(
            &mut builder,
            "foo/1.0/foo.cabal",
            b"name: foo\nversion: 1.0\n",
            1_000,
        );
        append(
            &mut builder,
            "foo/1.0/package.json",
            &package_json("foo-1.0.tar.gz", "ABCD", 42),
            1_000,
        );
        append(
            &mut builder,
            "bar/2.0/bar.cabal",
            b"name: bar\nversion: 2.0\n",
            2_000,
        );
        append(
            &mut builder,
            "foo/1.0/foo.cabal",
            b"name: foo\nversion: 1.0\nx-revision: 1\n",
            3_000,
        );
        builder.into_inner().unwrap()
    }

    fn wanted(pairs: &[(&str, &str)]) -> std::collections::HashSet<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_scan_metadata_revisions_and_sdist_hash() {
        let data = create_revisioned_index();
        let meta =
            scan_metadata_from_tar(std::io::Cursor::new(data), &wanted(&[("foo", "1.0")]), None)
                .unwrap();
        let foo = meta.get("foo", "1.0").unwrap();
        assert_eq!(foo.sdist_sha256.as_deref(), Some("abcd"));
        assert_eq!(foo.sdist_length, Some(42));
        assert_eq!(foo.cabal_revisions.len(), 2);
        assert_eq!(
            foo.cabal_revisions[0],
            crate::fetch::sha256_hex(b"name: foo\nversion: 1.0\n")
        );
        assert_eq!(foo.latest_revision().unwrap().0, 1);
        // Unwanted packages are not collected, but still count for the state.
        assert!(meta.get("bar", "2.0").is_none());
        assert_eq!(
            meta.index_state_string().as_deref(),
            Some("1970-01-01T00:50:00Z")
        );
    }

    #[test]
    fn test_scan_metadata_respects_index_state_cutoff() {
        let data = create_revisioned_index();
        let meta = scan_metadata_from_tar(
            std::io::Cursor::new(data),
            &wanted(&[("foo", "1.0")]),
            Some(2_500),
        )
        .unwrap();
        // The revision published at t=3000 is invisible at index-state 2500.
        let foo = meta.get("foo", "1.0").unwrap();
        assert_eq!(foo.cabal_revisions.len(), 1);
        assert_eq!(meta.index_state.unwrap().timestamp(), 2_000);
    }

    #[test]
    fn test_load_from_tar() {
        let tar_data = create_test_index();
        let cursor = std::io::Cursor::new(tar_data);

        let index = load_from_tar(cursor, &IndexOptions::default()).unwrap();

        assert_eq!(index.package_count(), 1);
        assert_eq!(index.version_count(), 2);

        let pkg = index.get_package("test-pkg").unwrap();
        assert_eq!(pkg.versions.len(), 2);

        let v1 = pkg.get_version(&"1.0.0".parse().unwrap()).unwrap();
        assert_eq!(v1.dependencies.len(), 1);
        assert_eq!(v1.dependencies[0].name, "base");

        let v2 = pkg.get_version(&"2.0.0".parse().unwrap()).unwrap();
        assert_eq!(v2.dependencies.len(), 2);
    }

    #[test]
    fn test_load_with_filter() {
        let tar_data = create_test_index();
        let cursor = std::io::Cursor::new(tar_data);

        let options = IndexOptions {
            filter_packages: vec!["nonexistent".to_string()],
            ..Default::default()
        };

        let index = load_from_tar(cursor, &options).unwrap();
        assert_eq!(index.package_count(), 0);
    }

    #[test]
    fn test_load_with_limit() {
        let tar_data = create_test_index();
        let cursor = std::io::Cursor::new(tar_data);

        let options = IndexOptions {
            max_packages: 1,
            ..Default::default()
        };

        let index = load_from_tar(cursor, &options).unwrap();
        // Should only have loaded 1 package-version entry
        assert!(index.version_count() <= 1);
    }
}
