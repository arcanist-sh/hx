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

/// Get the default Hackage index path.
pub fn default_index_path() -> Option<std::path::PathBuf> {
    let home = dirs_next::home_dir()?;
    let path = home
        .join(".cabal")
        .join("packages")
        .join("hackage.haskell.org")
        .join("01-index.tar.gz");

    if path.exists() {
        Some(path)
    } else {
        // Try uncompressed version
        let uncompressed = home
            .join(".cabal")
            .join("packages")
            .join("hackage.haskell.org")
            .join("01-index.tar");
        if uncompressed.exists() {
            Some(uncompressed)
        } else {
            None
        }
    }
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
