//! Project detection and context.

use crate::workspace;
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
    /// Path to the package directory (relative to workspace root; empty when
    /// the root directory is itself a member)
    pub path: PathBuf,
    /// Path to the .cabal file
    pub cabal_file: PathBuf,
    /// Package version from the .cabal file, if declared
    pub version: Option<String>,
    /// Every package named in the member's `build-depends` fields (all
    /// components and conditional branches)
    pub dependencies: Vec<String>,
    /// Names of the member's `executable` stanzas
    pub executables: Vec<String>,
    /// Names of the member's `test-suite` stanzas
    pub test_suites: Vec<String>,
}

impl WorkspacePackage {
    /// Whether this member is the workspace root directory itself (`packages: .`).
    pub fn is_root(&self) -> bool {
        self.path.as_os_str().is_empty()
    }
}

/// What a set of changed files means for a workspace build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AffectedMembers {
    /// A workspace-wide file changed (`cabal.project`, `hx.toml`, `hx.lock`, …):
    /// every member is affected.
    All,
    /// These members (changed, plus their reverse dependencies inside the
    /// workspace), in workspace order. May be empty.
    Some(Vec<String>),
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

    /// Path to the workspace's `cabal.project`.
    pub fn cabal_project_path(&self) -> PathBuf {
        self.root.join(workspace::CABAL_PROJECT)
    }

    /// The workspace members `name` depends on directly, in workspace order.
    pub fn local_deps(&self, name: &str) -> Vec<&str> {
        let Some(pkg) = self.get_package(name) else {
            return Vec::new();
        };
        self.workspace_packages
            .iter()
            .filter(|m| m.name != name && pkg.dependencies.contains(&m.name))
            .map(|m| m.name.as_str())
            .collect()
    }

    /// The workspace members that depend on `name` directly, in workspace order.
    pub fn local_reverse_deps(&self, name: &str) -> Vec<&str> {
        self.workspace_packages
            .iter()
            .filter(|m| m.name != name && m.dependencies.iter().any(|d| d == name))
            .map(|m| m.name.as_str())
            .collect()
    }

    /// `names` plus every member that depends on one of them, directly or
    /// transitively — everything a change to `names` can break. Returned in
    /// workspace order; unknown names are ignored.
    pub fn with_reverse_deps(&self, names: &[String]) -> Vec<String> {
        let mut affected: std::collections::HashSet<&str> = names
            .iter()
            .filter(|n| self.get_package(n).is_some())
            .map(String::as_str)
            .collect();
        let mut queue: Vec<&str> = affected.iter().copied().collect();
        while let Some(name) = queue.pop() {
            for dependent in self.local_reverse_deps(name) {
                if affected.insert(dependent) {
                    queue.push(dependent);
                }
            }
        }
        self.workspace_packages
            .iter()
            .filter(|m| affected.contains(m.name.as_str()))
            .map(|m| m.name.clone())
            .collect()
    }

    /// The member owning `path` (relative to the workspace root): the member
    /// with the longest directory prefix. A root member (`packages: .`) owns
    /// whatever no other member does.
    pub fn member_owning(&self, path: &Path) -> Option<&WorkspacePackage> {
        let path = workspace::normalize_entry(&path.to_string_lossy());
        self.workspace_packages
            .iter()
            .filter(|m| path.starts_with(&m.path))
            .max_by_key(|m| m.path.components().count())
    }

    /// The member whose directory contains `dir` (an absolute path, normally
    /// the current directory), ignoring a root member: standing at the root,
    /// or anywhere outside a member subdirectory, means the whole workspace.
    pub fn member_at(&self, dir: &Path) -> Option<&WorkspacePackage> {
        let relative = dir.strip_prefix(&self.root).ok()?;
        self.member_owning(relative).filter(|m| !m.is_root())
    }

    /// Map changed files (relative to the workspace root) to the members they
    /// affect, including reverse dependencies inside the workspace.
    ///
    /// A change to a workspace-wide file (see
    /// [`workspace::WORKSPACE_GLOBAL_FILES`]) affects everything; a file that
    /// belongs to no member (README, CI config) affects nothing.
    pub fn affected_by(&self, changed: &[PathBuf]) -> AffectedMembers {
        let mut touched: Vec<String> = Vec::new();
        for file in changed {
            let file = workspace::normalize_entry(&file.to_string_lossy());
            if file.components().count() == 1
                && workspace::WORKSPACE_GLOBAL_FILES
                    .iter()
                    .any(|g| file.as_os_str() == *g)
            {
                return AffectedMembers::All;
            }
            if let Some(member) = self.member_owning(&file)
                && !touched.contains(&member.name)
            {
                touched.push(member.name.clone());
            }
        }
        AffectedMembers::Some(self.with_reverse_deps(&touched))
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
            // A member package may carry its own hx.toml (e.g. one created by
            // `hx init` before `hx workspace add`). Like Cargo, prefer the
            // enclosing workspace root that lists it, so builds, the lockfile
            // and the toolchain pin are shared across the workspace.
            if let Some(workspace_root) = current
                .ancestors()
                .skip(1)
                .find(|a| is_member_of_workspace(a, current))
            {
                debug!(
                    "{} is a member of workspace {}",
                    current.display(),
                    workspace_root.display()
                );
                return Ok(workspace_root.to_path_buf());
            }
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
    workspace::find_cabal_file(dir)
}

/// Parse workspace packages from cabal.project file.
fn parse_workspace_packages(root: &Path) -> Result<Vec<WorkspacePackage>, Error> {
    let cabal_project_path = root.join(workspace::CABAL_PROJECT);

    let content = match std::fs::read_to_string(&cabal_project_path) {
        Ok(c) => c,
        Err(e) => {
            debug!("Failed to read cabal.project: {}", e);
            return Ok(Vec::new());
        }
    };

    let packages: Vec<WorkspacePackage> = workspace::resolve_members(root, &content)
        .into_iter()
        .map(|member| {
            let summary = std::fs::read_to_string(&member.cabal_file)
                .map(|c| workspace::scan_cabal(&c))
                .unwrap_or_default();
            // Prefer the declared package name; fall back to the file stem.
            let name = if summary.name.is_empty() {
                member
                    .cabal_file
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            } else {
                summary.name
            };
            WorkspacePackage {
                name,
                path: member.path,
                cabal_file: member.cabal_file,
                version: summary.version,
                dependencies: summary.dependencies.into_iter().collect(),
                executables: summary.executables,
                test_suites: summary.test_suites,
            }
        })
        .collect();

    debug!("Found {} workspace packages", packages.len());
    Ok(packages)
}

/// Whether `dir` is a member of the workspace rooted at `root` (a directory
/// holding both `hx.toml` and `cabal.project`).
fn is_member_of_workspace(root: &Path, dir: &Path) -> bool {
    if !root.join(MANIFEST_FILENAME).exists() {
        return false;
    }
    let Ok(content) = std::fs::read_to_string(root.join(workspace::CABAL_PROJECT)) else {
        return false;
    };
    let Ok(relative) = dir.strip_prefix(root) else {
        return false;
    };
    let relative = workspace::normalize_entry(&relative.to_string_lossy());
    workspace::resolve_members(root, &content)
        .iter()
        .any(|m| m.path == relative)
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

    /// Write a 3-member workspace: lib-a, lib-b (depends on lib-a), app
    /// (depends on lib-b).
    fn three_member_workspace() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(MANIFEST_FILENAME), "[project]\nname = \"ws\"\n").unwrap();
        fs::write(
            root.join("cabal.project"),
            "packages:\n  ./lib-a\n  ./lib-b\n  ./app\n",
        )
        .unwrap();
        let pkgs = [
            ("lib-a", "library\n  build-depends: base\n"),
            (
                "lib-b",
                "library\n  build-depends: base, lib-a\n\ntest-suite lib-b-test\n  build-depends: base, lib-b\n",
            ),
            ("app", "executable app\n  build-depends: base, lib-b\n"),
        ];
        for (name, body) in pkgs {
            fs::create_dir_all(root.join(name).join("src")).unwrap();
            fs::write(
                root.join(name).join(format!("{name}.cabal")),
                format!("cabal-version: 3.0\nname: {name}\nversion: 0.1.0\n\n{body}"),
            )
            .unwrap();
        }
        dir
    }

    #[test]
    fn workspace_members_and_graph() {
        let dir = three_member_workspace();
        let project = Project::load(dir.path()).unwrap();
        assert!(project.is_workspace());
        assert_eq!(project.package_names(), vec!["lib-a", "lib-b", "app"]);
        let lib_b = project.get_package("lib-b").unwrap();
        assert_eq!(lib_b.version.as_deref(), Some("0.1.0"));
        assert_eq!(lib_b.test_suites, vec!["lib-b-test"]);
        assert_eq!(project.local_deps("lib-b"), vec!["lib-a"]);
        assert_eq!(project.local_deps("app"), vec!["lib-b"]);
        assert!(project.local_deps("lib-a").is_empty());
        assert_eq!(project.local_reverse_deps("lib-a"), vec!["lib-b"]);
        assert_eq!(
            project.with_reverse_deps(&["lib-a".to_string()]),
            vec!["lib-a", "lib-b", "app"]
        );
        assert_eq!(project.with_reverse_deps(&["app".to_string()]), vec!["app"]);
    }

    #[test]
    fn workspace_affected_set() {
        let dir = three_member_workspace();
        let project = Project::load(dir.path()).unwrap();
        let affected = |files: &[&str]| {
            project.affected_by(&files.iter().map(PathBuf::from).collect::<Vec<_>>())
        };
        assert_eq!(
            affected(&["lib-b/src/LibB.hs"]),
            AffectedMembers::Some(vec!["lib-b".into(), "app".into()])
        );
        assert_eq!(
            affected(&["./app/src/Main.hs", "app/app.cabal"]),
            AffectedMembers::Some(vec!["app".into()])
        );
        assert_eq!(
            affected(&["README.md", "docs/x.md"]),
            AffectedMembers::Some(vec![])
        );
        assert_eq!(affected(&["cabal.project"]), AffectedMembers::All);
        assert_eq!(affected(&["hx.lock"]), AffectedMembers::All);
        // A nested file that merely shares a global file's name is not global.
        assert_eq!(
            affected(&["lib-a/hx.toml"]),
            AffectedMembers::Some(vec!["lib-a".into(), "lib-b".into(), "app".into()])
        );
    }

    #[test]
    fn workspace_member_at_directory() {
        let dir = three_member_workspace();
        let project = Project::load(dir.path()).unwrap();
        let at = |rel: &str| {
            project
                .member_at(&dir.path().join(rel))
                .map(|m| m.name.clone())
        };
        assert_eq!(at("lib-b/src"), Some("lib-b".to_string()));
        assert_eq!(at("app"), Some("app".to_string()));
        assert_eq!(at(""), None);
        assert_eq!(at("docs"), None);
    }

    #[test]
    fn member_with_own_manifest_resolves_to_workspace_root() {
        let dir = three_member_workspace();
        let member = dir.path().join("lib-b");
        fs::write(
            member.join(MANIFEST_FILENAME),
            "[project]\nname = \"lib-b\"\n",
        )
        .unwrap();
        assert_eq!(find_project_root(member.join("src")).unwrap(), dir.path());

        // A nested project the workspace does not list stays standalone.
        let other = dir.path().join("scratch");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join(MANIFEST_FILENAME), "[project]\nname = \"x\"\n").unwrap();
        assert_eq!(find_project_root(&other).unwrap(), other);
    }
}
