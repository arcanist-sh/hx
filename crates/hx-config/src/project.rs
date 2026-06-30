//! Project detection and context.

use crate::{CACHE_DIR_NAME, LOCKFILE_FILENAME, MANIFEST_FILENAME, Manifest, load_global_config};
use hx_core::error::{Error, Fix};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// A detected hx project.
#[derive(Debug, Clone)]
pub struct Project {
    /// Path to the project root (containing hx.toml)
    pub root: PathBuf,
    /// Parsed manifest
    pub manifest: Manifest,
    /// Path to the .cabal file, if found
    pub cabal_file: Option<PathBuf>,
    /// Whether cabal.project exists
    pub has_cabal_project: bool,
    /// Workspace packages (if this is a workspace)
    pub workspace_packages: Vec<WorkspacePackage>,
}

/// A package within a workspace.
#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    /// Package name
    pub name: String,
    /// Path to the package directory (relative to workspace root)
    pub path: PathBuf,
    /// Path to the .cabal file
    pub cabal_file: PathBuf,
}

impl Project {
    /// Load a project from a directory.
    ///
    /// This automatically loads and applies global config defaults from
    /// `~/.config/hx/config.toml` (or platform equivalent). Project-local
    /// settings in `hx.toml` take precedence over global config.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = root.as_ref().to_path_buf();
        let manifest_path = root.join(MANIFEST_FILENAME);

        let mut manifest = Manifest::from_file(&manifest_path).map_err(|e| Error::Config {
            message: format!("failed to load manifest: {}", e),
            path: Some(manifest_path.clone()),
            source: Some(Box::new(e)),
            fixes: vec![Fix::with_command("Create a new project", "hx init")],
        })?;

        // Apply global config defaults
        match load_global_config() {
            Ok(Some(global)) => {
                debug!("Applying global config defaults");
                manifest = manifest.with_global_defaults(&global);
            }
            Ok(None) => {
                debug!("No global config found");
            }
            Err(e) => {
                // Warn but continue - don't fail if global config is malformed
                warn!("Failed to load global config: {}", e);
            }
        }

        let cabal_file = find_cabal_file(&root);
        let has_cabal_project = root.join("cabal.project").exists();

        // Parse workspace packages if this is a multi-package project
        let workspace_packages = if has_cabal_project {
            parse_workspace_packages(&root)?
        } else {
            Vec::new()
        };

        Ok(Self {
            root,
            manifest,
            cabal_file,
            has_cabal_project,
            workspace_packages,
        })
    }

    /// Get the path to the hx.toml manifest.
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILENAME)
    }

    /// Get the path to the hx.lock file.
    pub fn lockfile_path(&self) -> PathBuf {
        self.root.join(LOCKFILE_FILENAME)
    }

    /// Get the path to the .hx cache directory.
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(CACHE_DIR_NAME)
    }

    /// Get the cabal build directory.
    pub fn cabal_build_dir(&self) -> PathBuf {
        self.cache_dir().join("cabal").join("dist-newstyle")
    }

    /// Check if the lockfile exists.
    pub fn has_lockfile(&self) -> bool {
        self.lockfile_path().exists()
    }

    /// Get the project name.
    pub fn name(&self) -> &str {
        &self.manifest.project.name
    }

    /// Check if this is a workspace (multi-package project).
    pub fn is_workspace(&self) -> bool {
        !self.workspace_packages.is_empty()
    }

    /// Get the number of packages in the workspace.
    pub fn package_count(&self) -> usize {
        if self.workspace_packages.is_empty() {
            1 // Single package project
        } else {
            self.workspace_packages.len()
        }
    }

    /// Get a workspace package by name.
    pub fn get_package(&self, name: &str) -> Option<&WorkspacePackage> {
        self.workspace_packages.iter().find(|p| p.name == name)
    }

    /// List all package names in the workspace.
    pub fn package_names(&self) -> Vec<&str> {
        if self.workspace_packages.is_empty() {
            vec![self.name()]
        } else {
            self.workspace_packages
                .iter()
                .map(|p| p.name.as_str())
                .collect()
        }
    }

    /// Paths to every `.cabal` file that contributes dependencies to this
    /// project: each workspace member's file, or — for a single-package
    /// project (no `cabal.project`) — the project's own `.cabal` file.
    ///
    /// Callers that only scanned `workspace_packages` silently collected zero
    /// dependencies for single-package projects (the common case), producing an
    /// empty lockfile. Use this so both layouts are covered.
    pub fn cabal_files(&self) -> Vec<PathBuf> {
        if self.workspace_packages.is_empty() {
            self.cabal_file.clone().into_iter().collect()
        } else {
            self.workspace_packages
                .iter()
                .map(|p| p.cabal_file.clone())
                .collect()
        }
    }
}

/// Find the project root by searching upward for hx.toml.
pub fn find_project_root(start: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let start = start.as_ref();
    let start = if start.is_relative() {
        std::env::current_dir()
            .map_err(|e| Error::Io {
                message: "failed to get current directory".to_string(),
                path: None,
                source: e,
            })?
            .join(start)
    } else {
        start.to_path_buf()
    };

    let mut current = start.as_path();
    let mut searched = Vec::new();
    // Remember an existing Cabal/Stack project found during the upward search,
    // so a "not found" error can point the user at adoption instead of dead-ending.
    let mut adoptable: Option<&'static str> = None;

    loop {
        debug!("Checking for project root at: {}", current.display());
        searched.push(current.to_path_buf());

        let manifest_path = current.join(MANIFEST_FILENAME);
        if manifest_path.exists() {
            debug!("Found project root: {}", current.display());
            return Ok(current.to_path_buf());
        }

        // No hx.toml here — but note an existing Cabal/Stack project so we can
        // suggest adopting it. The first (deepest) one wins.
        if adoptable.is_none() {
            if current.join("stack.yaml").exists() {
                adoptable = Some("stack");
            } else if current.join("cabal.project").exists() || find_cabal_file(current).is_some() {
                adoptable = Some("cabal");
            }
        }

        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    // Lead with an adoption command when there is a project to adopt.
    let fixes = match adoptable {
        Some("stack") => vec![
            Fix::with_command("Adopt this Stack project", "hx import --from stack"),
            Fix::with_command("Or start a fresh project", "hx init"),
        ],
        Some(_) => vec![
            Fix::with_command("Adopt this Cabal project", "hx import --from cabal"),
            Fix::with_command("Or start a fresh project", "hx init"),
        ],
        None => vec![
            Fix::with_command("Create a new project", "hx init"),
            Fix::new("Run from within a project directory containing hx.toml"),
        ],
    };

    Err(Error::ProjectNotFound { searched, fixes })
}

/// Find a .cabal file in the given directory.
fn find_cabal_file(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "cabal") {
            return Some(path);
        }
    }
    None
}

/// Parse workspace packages from cabal.project file.
fn parse_workspace_packages(root: &Path) -> Result<Vec<WorkspacePackage>, Error> {
    let cabal_project_path = root.join("cabal.project");

    let content = match std::fs::read_to_string(&cabal_project_path) {
        Ok(c) => c,
        Err(e) => {
            debug!("Failed to read cabal.project: {}", e);
            return Ok(Vec::new());
        }
    };

    let mut packages = Vec::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with("--") {
            continue;
        }

        // Parse "packages:" directive
        // Can be single line: packages: ./pkg1 ./pkg2
        // Or multi-line with continuation
        if line.starts_with("packages:") {
            let paths_str = line.strip_prefix("packages:").unwrap_or("").trim();
            for path_part in paths_str.split_whitespace() {
                if let Some(pkg) = parse_package_path(root, path_part) {
                    packages.push(pkg);
                }
            }
        } else if !line.contains(':') && !packages.is_empty() {
            // Continuation line for packages (part of previous packages: block)
            // Only if we've already seen packages: and line doesn't start a new directive
            for path_part in line.split_whitespace() {
                if let Some(pkg) = parse_package_path(root, path_part) {
                    packages.push(pkg);
                }
            }
        }
    }

    debug!("Found {} workspace packages", packages.len());
    Ok(packages)
}

/// Parse a single package path from cabal.project.
fn parse_package_path(root: &Path, path_str: &str) -> Option<WorkspacePackage> {
    // Handle glob patterns like ./packages/*
    // For now, just handle direct paths
    let path_str = path_str.trim();

    // Skip glob patterns for now (would need glob crate)
    if path_str.contains('*') {
        debug!("Skipping glob pattern in cabal.project: {}", path_str);
        return None;
    }

    let package_path = if let Some(stripped) = path_str.strip_prefix("./") {
        root.join(stripped)
    } else if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        root.join(path_str)
    };

    // Check if the path exists and contains a .cabal file
    if !package_path.exists() || !package_path.is_dir() {
        debug!("Package path does not exist: {}", package_path.display());
        return None;
    }

    let cabal_file = find_cabal_file(&package_path)?;
    let name = cabal_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let relative_path = package_path
        .strip_prefix(root)
        .unwrap_or(&package_path)
        .to_path_buf();

    Some(WorkspacePackage {
        name,
        path: relative_path,
        cabal_file,
    })
}

/// Check if a directory looks like a Haskell project.
pub fn is_haskell_project(dir: &Path) -> bool {
    // Has hx.toml
    if dir.join(MANIFEST_FILENAME).exists() {
        return true;
    }

    // Has .cabal file
    if find_cabal_file(dir).is_some() {
        return true;
    }

    // Has cabal.project
    if dir.join("cabal.project").exists() {
        return true;
    }

    // Has stack.yaml (we don't support stack yet, but it's a Haskell project)
    if dir.join("stack.yaml").exists() {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_find_project_root() {
        let dir = tempdir().unwrap();
        let manifest = dir.path().join(MANIFEST_FILENAME);
        fs::write(
            &manifest,
            r#"
[project]
name = "test"
"#,
        )
        .unwrap();

        let root = find_project_root(dir.path()).unwrap();
        assert_eq!(root, dir.path());
    }

    #[test]
    fn test_find_project_root_nested() {
        let dir = tempdir().unwrap();
        let manifest = dir.path().join(MANIFEST_FILENAME);
        fs::write(
            &manifest,
            r#"
[project]
name = "test"
"#,
        )
        .unwrap();

        let subdir = dir.path().join("src").join("deep");
        fs::create_dir_all(&subdir).unwrap();

        let root = find_project_root(&subdir).unwrap();
        assert_eq!(root, dir.path());
    }

    #[test]
    fn test_project_not_found() {
        let dir = tempdir().unwrap();
        let result = find_project_root(dir.path());
        assert!(result.is_err());
    }

    /// Helper: collect the fix commands from a ProjectNotFound error.
    fn fix_commands(err: &Error) -> Vec<String> {
        err.fixes()
            .iter()
            .filter_map(|f| f.command.clone())
            .collect()
    }

    #[test]
    fn test_not_found_suggests_cabal_adoption() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("mylib.cabal"), "name: mylib\n").unwrap();
        let err = find_project_root(dir.path()).unwrap_err();
        let cmds = fix_commands(&err);
        assert!(cmds.iter().any(|c| c == "hx import --from cabal"));
        assert!(cmds.iter().any(|c| c == "hx init"));
    }

    #[test]
    fn test_not_found_suggests_stack_adoption() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("stack.yaml"), "resolver: lts-22.0\n").unwrap();
        let err = find_project_root(dir.path()).unwrap_err();
        assert!(
            fix_commands(&err)
                .iter()
                .any(|c| c == "hx import --from stack")
        );
    }

    #[test]
    fn test_not_found_no_project_suggests_init_only() {
        let dir = tempdir().unwrap();
        let err = find_project_root(dir.path()).unwrap_err();
        let cmds = fix_commands(&err);
        assert!(cmds.iter().any(|c| c == "hx init"));
        assert!(!cmds.iter().any(|c| c.contains("import")));
    }
}
