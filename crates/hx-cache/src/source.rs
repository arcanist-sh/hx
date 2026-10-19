//! Source fingerprinting for incremental builds.
//!
//! This module computes fingerprints of source files to detect when
//! a project's code has changed, enabling skip-build optimizations.

use hx_core::error::{Error, Result};
use hx_core::hash::{Digest, Sha256, to_hex};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tracing::debug;

/// A fingerprint of project source files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFingerprint {
    /// The hex-encoded SHA256 hash
    pub hash: String,
    /// Number of files included in the fingerprint
    pub file_count: usize,
}

/// Compute a fingerprint of all source files in a directory.
///
/// This includes:
/// - All `.hs` files (Haskell source)
/// - All `.cabal` files (package definitions)
/// - The `hx.toml` manifest
/// - Any `cabal.project` file (workspace config)
///
/// Files are sorted before hashing to ensure deterministic results.
pub fn compute_source_fingerprint(project_root: &Path) -> Result<SourceFingerprint> {
    let mut hasher = Sha256::new();
    let mut file_count = 0;

    // Collect all source files, sorted for determinism
    let files = collect_source_files(project_root)?;

    for path in &files {
        // Hash the relative path (for cross-machine consistency)
        if let Ok(relative) = path.strip_prefix(project_root) {
            hasher.update(relative.to_string_lossy().as_bytes());
        } else {
            hasher.update(path.to_string_lossy().as_bytes());
        }

        // Hash the file contents
        let content = std::fs::read(path).map_err(|e| Error::Io {
            message: format!("failed to read source file: {}", path.display()),
            path: Some(path.clone()),
            source: e,
        })?;
        hasher.update(&content);
        file_count += 1;
    }

    let hash = format!("sha256:{}", to_hex(hasher.finalize()));

    debug!(
        "Computed source fingerprint: {} ({} files)",
        hash, file_count
    );

    Ok(SourceFingerprint { hash, file_count })
}

/// Collect all source files for fingerprinting.
fn collect_source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();

    // Add manifest files at the root
    let hx_toml = root.join("hx.toml");
    if hx_toml.exists() {
        files.insert(hx_toml);
    }

    let cabal_project = root.join("cabal.project");
    if cabal_project.exists() {
        files.insert(cabal_project);
    }

    // Walk directory tree for .hs and .cabal files
    collect_source_files_recursive(root, &mut files)?;

    Ok(files.into_iter().collect())
}

fn collect_source_files_recursive(dir: &Path, files: &mut BTreeSet<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| Error::Io {
        message: format!("failed to read directory: {}", dir.display()),
        path: Some(dir.to_path_buf()),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| Error::Io {
            message: "failed to read directory entry".to_string(),
            path: Some(dir.to_path_buf()),
            source: e,
        })?;

        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        // Skip hidden directories and common non-source directories
        if name.starts_with('.') || name == "dist-newstyle" || name == "node_modules" {
            continue;
        }

        if path.is_dir() {
            collect_source_files_recursive(&path, files)?;
        } else if path.is_file()
            && let Some(ext) = path.extension()
        {
            let ext = ext.to_string_lossy();
            // Include Haskell source and cabal files
            if ext == "hs" || ext == "lhs" || ext == "cabal" || ext == "hsc" {
                files.insert(path);
            }
        }
    }

    Ok(())
}

/// Path to the cached source fingerprint file.
pub fn source_fingerprint_path(project_root: &Path) -> PathBuf {
    project_root.join(".hx").join("source-fingerprint")
}

/// Load the last computed source fingerprint.
pub fn load_source_fingerprint(project_root: &Path) -> Option<SourceFingerprint> {
    let path = source_fingerprint_path(project_root);
    let content = std::fs::read_to_string(&path).ok()?;
    let mut lines = content.lines();

    let hash = lines.next()?.to_string();
    let file_count = lines.next()?.parse().ok()?;

    Some(SourceFingerprint { hash, file_count })
}

/// Save the source fingerprint for later comparison.
pub fn save_source_fingerprint(project_root: &Path, fingerprint: &SourceFingerprint) -> Result<()> {
    let path = source_fingerprint_path(project_root);

    // Ensure .hx directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io {
            message: "failed to create .hx directory".to_string(),
            path: Some(parent.to_path_buf()),
            source: e,
        })?;
    }

    let content = format!("{}\n{}\n", fingerprint.hash, fingerprint.file_count);
    std::fs::write(&path, content).map_err(|e| Error::Io {
        message: "failed to write source fingerprint".to_string(),
        path: Some(path),
        source: e,
    })?;

    Ok(())
}

#[cfg(test)]
mod hash_golden_tests {
    //! Source fingerprints decide whether a build is up to date. Pinned to exact output so a hashing-library upgrade cannot
    //! silently change them.
    use super::*;

    #[test]
    fn source_fingerprint_is_stable() {
        // Root-level files only, so relative paths have no separator and the
        // fingerprint is the same on every OS.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hx.toml"), b"[project]\nname = \"demo\"\n").unwrap();
        std::fs::write(dir.path().join("demo.cabal"), b"name: demo\n").unwrap();
        std::fs::write(dir.path().join("Main.hs"), b"main = pure ()\n").unwrap();
        let fp = compute_source_fingerprint(dir.path()).unwrap();
        assert_eq!(fp.file_count, 3);
        assert_eq!(
            fp.hash,
            "sha256:ebc8cdfa4f3263cf60e75f53f86f93a94f7b7fc639d3113e8dc8223246a20ad5"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_compute_source_fingerprint() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Create some source files
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/Main.hs"),
            "module Main where\nmain = pure ()",
        )
        .unwrap();
        fs::write(root.join("src/Lib.hs"), "module Lib where").unwrap();
        fs::write(root.join("test.cabal"), "name: test\nversion: 0.1.0").unwrap();
        fs::write(root.join("hx.toml"), "[toolchain]\nghc = \"9.8.2\"").unwrap();

        let fp = compute_source_fingerprint(root).unwrap();

        assert!(fp.hash.starts_with("sha256:"));
        assert_eq!(fp.file_count, 4);
    }

    #[test]
    fn test_fingerprint_changes_with_content() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/Main.hs"), "module Main where").unwrap();

        let fp1 = compute_source_fingerprint(root).unwrap();

        // Modify the file
        fs::write(
            root.join("src/Main.hs"),
            "module Main where\nmain = pure ()",
        )
        .unwrap();

        let fp2 = compute_source_fingerprint(root).unwrap();

        assert_ne!(fp1.hash, fp2.hash);
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/Main.hs"), "module Main").unwrap();
        fs::write(root.join("src/Lib.hs"), "module Lib").unwrap();

        let fp1 = compute_source_fingerprint(root).unwrap();
        let fp2 = compute_source_fingerprint(root).unwrap();

        assert_eq!(fp1.hash, fp2.hash);
    }

    #[test]
    fn test_save_load_fingerprint() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        let fp = SourceFingerprint {
            hash: "sha256:abc123".to_string(),
            file_count: 10,
        };

        save_source_fingerprint(root, &fp).unwrap();
        let loaded = load_source_fingerprint(root).unwrap();

        assert_eq!(fp, loaded);
    }
}
