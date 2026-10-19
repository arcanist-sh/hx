//! Workspace commands and member selection.
//!
//! Membership lives in `cabal.project` (see [`hx_config::workspace`]); these
//! commands edit it and report on it. The selection helpers decide which
//! members `hx build`/`test`/`check`/`run` act on: an explicit `--package`,
//! the members affected by changed files (`--changed`), the member containing
//! the current directory, or the whole workspace.

use anyhow::Result;
use hx_config::workspace::{self as ws, CABAL_PROJECT};
use hx_config::{AffectedMembers, MANIFEST_FILENAME, Project, find_project_root};
use hx_core::CommandRunner;
use hx_core::error::{Error, Fix};
use hx_ui::Output;
use std::path::{Path, PathBuf};

/// Which packages a build-like command operates on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selection {
    /// Not a workspace: the package in the project root.
    Single,
    /// Every workspace member (the `all` target).
    All,
    /// These workspace members, in workspace order. Never empty.
    Members(Vec<String>),
    /// `--changed` found nothing to do.
    Nothing,
}

impl Selection {
    /// The Cabal targets for this selection.
    pub(crate) fn targets(&self) -> Vec<String> {
        match self {
            Selection::Single | Selection::Nothing => Vec::new(),
            Selection::All => vec!["all".to_string()],
            Selection::Members(names) => names.clone(),
        }
    }

    /// Whether this selection covers the whole project, so whole-project
    /// state (the build freshness fingerprint) may be consulted and updated.
    pub(crate) fn is_everything(&self) -> bool {
        matches!(self, Selection::Single | Selection::All)
    }

    /// Human-readable description for status lines.
    pub(crate) fn label(&self, project: &Project) -> String {
        match self {
            Selection::Single | Selection::Nothing => project.name().to_string(),
            Selection::All => format!(
                "{} ({}/{} packages)",
                project.name(),
                project.package_count(),
                project.package_count()
            ),
            Selection::Members(names) => format!(
                "{} ({}/{} packages)",
                names.join(", "),
                names.len(),
                project.package_count()
            ),
        }
    }
}

/// Resolve which members a build-like command should act on.
///
/// Precedence: `--package` (validated), then `--changed` (intersected with
/// `--package` when both are given), then the member containing the current
/// directory, then the whole workspace. Returns `Err(exit_code)` after
/// reporting a problem.
pub(crate) async fn select(
    project: &Project,
    packages: &[String],
    changed: Option<&str>,
    output: &Output,
) -> std::result::Result<Selection, i32> {
    if !project.is_workspace() {
        if !packages.is_empty() {
            output.warn("--package flag ignored (not a workspace project)");
        }
        if changed.is_some() {
            output.warn("--changed flag ignored (not a workspace project)");
        }
        return Ok(Selection::Single);
    }

    for name in packages {
        if project.get_package(name).is_none() {
            output.error(&format!("Package '{}' not found in workspace", name));
            output.info(&format!(
                "Available packages: {}",
                project.package_names().join(", ")
            ));
            return Err(1);
        }
    }

    if let Some(base) = changed {
        let files = match changed_files(&project.root, base).await {
            Ok(files) => files,
            Err(e) => {
                output.print_error(&e);
                return Err(1);
            }
        };
        output.verbose(&format!("{} changed file(s) since {}", files.len(), base));
        let affected = match project.affected_by(&files) {
            AffectedMembers::All => project
                .package_names()
                .into_iter()
                .map(str::to_string)
                .collect(),
            AffectedMembers::Some(names) => names,
        };
        let selected: Vec<String> = if packages.is_empty() {
            affected
        } else {
            affected
                .into_iter()
                .filter(|n| packages.contains(n))
                .collect()
        };
        if selected.is_empty() {
            output.info(&format!(
                "No workspace members affected by changes since {}",
                base
            ));
            return Ok(Selection::Nothing);
        }
        if packages.is_empty() && selected.len() == project.package_count() {
            return Ok(Selection::All);
        }
        output.info(&format!("Affected members: {}", selected.join(", ")));
        return Ok(Selection::Members(selected));
    }

    if !packages.is_empty() {
        let names = project
            .package_names()
            .into_iter()
            .filter(|n| packages.iter().any(|p| p == n))
            .map(str::to_string)
            .collect();
        return Ok(Selection::Members(names));
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Some(member) = project.member_at(&cwd)
    {
        output.verbose(&format!(
            "Selecting member {} (current directory)",
            member.name
        ));
        return Ok(Selection::Members(vec![member.name.clone()]));
    }

    Ok(Selection::All)
}

/// Keep only members that have a test suite, so `cabal test <pkg>` does not
/// fail on a library without tests. Reports skipped members verbosely.
pub(crate) fn members_with_tests(
    project: &Project,
    selection: Selection,
    output: &Output,
) -> Selection {
    let names: Vec<String> = match &selection {
        Selection::All => project
            .package_names()
            .into_iter()
            .map(str::to_string)
            .collect(),
        Selection::Members(names) => names.clone(),
        Selection::Single | Selection::Nothing => return selection,
    };
    let (with, without): (Vec<String>, Vec<String>) = names.into_iter().partition(|n| {
        project
            .get_package(n)
            .is_some_and(|p| !p.test_suites.is_empty())
    });
    if !without.is_empty() {
        output.verbose(&format!("No test suites in: {}", without.join(", ")));
    }
    match selection {
        // Keep `all` when every member has tests — the shortest invocation.
        Selection::All if without.is_empty() => Selection::All,
        _ if with.is_empty() => Selection::Nothing,
        _ => Selection::Members(with),
    }
}

/// Files changed relative to `base` (default `HEAD`), as paths relative to
/// the workspace root: tracked changes (staged or not) plus untracked files.
///
/// For a base other than `HEAD` the comparison starts from the merge base of
/// `base` and `HEAD`, so a branch is compared against where it forked rather
/// than against whatever has landed on `base` since.
pub(crate) async fn changed_files(root: &Path, base: &str) -> Result<Vec<PathBuf>, Error> {
    let runner = CommandRunner::new().with_working_dir(root);
    let git_error = |message: String| Error::Config {
        message,
        path: None,
        source: None,
        fixes: vec![
            Fix::new("--changed needs a git repository with at least one commit"),
            Fix::with_command(
                "Compare against a branch instead",
                "hx build --changed origin/main",
            ),
        ],
    };

    let mut from = base.to_string();
    if base != "HEAD" {
        let merge_base = runner
            .run("git", ["merge-base", base, "HEAD"])
            .await
            .map_err(|e| git_error(format!("could not run git: {e}")))?;
        if merge_base.success() {
            from = merge_base.stdout.trim().to_string();
        }
    }

    let diff = runner
        .run("git", ["diff", "--name-only", "--relative", from.as_str()])
        .await
        .map_err(|e| git_error(format!("could not run git: {e}")))?;
    if !diff.success() {
        return Err(git_error(format!(
            "git diff against `{}` failed: {}",
            base,
            diff.stderr.trim()
        )));
    }
    let untracked = runner
        .run("git", ["ls-files", "--others", "--exclude-standard"])
        .await
        .map_err(|e| git_error(format!("could not run git: {e}")))?;

    let mut files: Vec<PathBuf> = diff
        .stdout
        .lines()
        .chain(untracked.stdout.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Load the workspace for a membership-editing command: the project root must
/// hold a `cabal.project`.
fn load_workspace_root() -> Result<(PathBuf, String), Error> {
    let root = find_project_root(".")?;
    let path = root.join(CABAL_PROJECT);
    let content = std::fs::read_to_string(&path).map_err(|_| Error::Config {
        message: format!("{} is not a workspace (no cabal.project)", root.display()),
        path: Some(path.clone()),
        source: None,
        fixes: vec![Fix::with_command(
            "Turn this project into a workspace",
            "hx workspace init",
        )],
    })?;
    Ok((root, content))
}

/// Write `cabal.project` atomically.
fn write_cabal_project(root: &Path, content: &str) -> Result<(), Error> {
    let path = root.join(CABAL_PROJECT);
    hx_core::atomic_write(&path, content).map_err(|e| Error::Io {
        message: "failed to write cabal.project".to_string(),
        path: Some(path),
        source: e,
    })
}

/// `hx workspace init [PATH]`: make a directory a workspace root.
pub async fn init(path: Option<String>, name: Option<String>, output: &Output) -> Result<i32> {
    let dir = match path {
        Some(p) => std::env::current_dir()?.join(p),
        None => std::env::current_dir()?,
    };
    std::fs::create_dir_all(&dir)?;

    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string()
    });

    let manifest_path = dir.join(MANIFEST_FILENAME);
    if manifest_path.exists() {
        output.verbose("Keeping existing hx.toml");
    } else {
        let manifest = format!(
            "# hx workspace root.\n\
             # Members are listed in cabal.project; manage them with\n\
             # `hx workspace add <path>` and `hx workspace rm <path>`.\n\
             # Settings here (toolchain, build, plugins) apply to every member.\n\
             \n\
             [project]\n\
             name = \"{name}\"\n"
        );
        hx_core::atomic_write(&manifest_path, manifest)?;
        output.status("Created", "hx.toml");
    }

    let cabal_project = dir.join(CABAL_PROJECT);
    if cabal_project.exists() {
        output.verbose("Keeping existing cabal.project");
    } else {
        let entries: Vec<String> = ws::discover_package_dirs(&dir)
            .iter()
            .map(|d| ws::entry_for_dir(d))
            .collect();
        let mut content = String::from(
            "-- Workspace members. Cabal and hx both read this list; manage it\n\
             -- with `hx workspace add <path>` / `hx workspace rm <path>`.\n",
        );
        for entry in &entries {
            content = ws::add_package_entry(&content, entry).unwrap_or(content);
        }
        if entries.is_empty() {
            content.push_str("packages:\n");
        }
        write_cabal_project(&dir, &content)?;
        output.status("Created", "cabal.project");
    }

    let project = Project::load(&dir)?;
    if project.workspace_packages.is_empty() {
        output.status("Initialized", &format!("workspace {name} (no members yet)"));
        output.info("Add a member with: hx workspace add <path>");
        output.info("Create a new one with: hx init --lib <path> && hx workspace add <path>");
    } else {
        output.status(
            "Initialized",
            &format!(
                "workspace {} with {} member(s): {}",
                name,
                project.workspace_packages.len(),
                project.package_names().join(", ")
            ),
        );
        output.info("Lock dependencies for every member with: hx lock");
    }
    Ok(0)
}

/// `hx workspace add <PATH>...`: add member directories to `cabal.project`.
pub async fn add(paths: Vec<String>, output: &Output) -> Result<i32> {
    let (root, mut content) = match load_workspace_root() {
        Ok(v) => v,
        Err(e) => {
            output.print_error(&e);
            return Ok(e.code().exit_code());
        }
    };
    let canonical_root = root.canonicalize()?;
    let cwd = std::env::current_dir()?;
    let mut added = Vec::new();

    for raw in &paths {
        let mut dir = cwd.join(raw);
        if dir.extension().is_some_and(|e| e == "cabal") {
            dir.pop();
        }
        let Ok(canonical) = dir.canonicalize() else {
            output.print_error(&Error::Config {
                message: format!("{raw} does not exist"),
                path: Some(dir.clone()),
                source: None,
                fixes: vec![Fix::with_command(
                    "Create a library package there first",
                    format!("hx init --lib {raw}"),
                )],
            });
            return Ok(3);
        };
        let Ok(relative) = canonical.strip_prefix(&canonical_root) else {
            output.print_error(&Error::Config {
                message: format!("{raw} is outside the workspace root {}", root.display()),
                path: Some(canonical.clone()),
                source: None,
                fixes: vec![Fix::new(
                    "Move the package under the workspace root, or reference it from cabal.project by hand",
                )],
            });
            return Ok(3);
        };
        let relative = relative.to_path_buf();
        if ws::find_cabal_file(&canonical).is_none() {
            output.print_error(&Error::Config {
                message: format!("{raw} has no .cabal file"),
                path: Some(canonical.clone()),
                source: None,
                fixes: vec![Fix::with_command(
                    "Create a library package there",
                    format!("hx init --lib {raw}"),
                )],
            });
            return Ok(3);
        }

        if ws::resolve_members(&root, &content)
            .iter()
            .any(|m| m.path == relative)
        {
            output.info(&format!("{} is already a workspace member", raw));
            continue;
        }
        let entry = ws::entry_for_dir(&relative);
        if let Some(updated) = ws::add_package_entry(&content, &entry) {
            content = updated;
            added.push(entry);
        }
        if !relative.as_os_str().is_empty() && canonical.join(MANIFEST_FILENAME).exists() {
            output.info(&format!(
                "{}/hx.toml is not used inside the workspace; the root hx.toml applies",
                relative.display()
            ));
        }
    }

    if added.is_empty() {
        return Ok(0);
    }
    write_cabal_project(&root, &content)?;
    for entry in &added {
        output.status("Added", entry);
    }
    output.info("Update the workspace lockfile with: hx lock");
    Ok(0)
}

/// `hx workspace rm <NAME|PATH>...`: remove members from `cabal.project`.
/// Package files are left in place.
pub async fn remove(members: Vec<String>, output: &Output) -> Result<i32> {
    let (root, mut content) = match load_workspace_root() {
        Ok(v) => v,
        Err(e) => {
            output.print_error(&e);
            return Ok(e.code().exit_code());
        }
    };
    let project = Project::load(&root)?;
    let cwd = std::env::current_dir()?;
    let mut removed = Vec::new();

    for raw in &members {
        // A member name, or a path (relative to the current directory).
        let member = project.get_package(raw).or_else(|| {
            let abs = cwd.join(raw).canonicalize().ok()?;
            let rel = abs
                .strip_prefix(root.canonicalize().ok()?)
                .ok()?
                .to_path_buf();
            project.workspace_packages.iter().find(|m| m.path == rel)
        });
        let Some(member) = member else {
            output.error(&format!("'{}' is not a workspace member", raw));
            output.info(&format!("Members: {}", project.package_names().join(", ")));
            return Ok(1);
        };

        match ws::remove_package_entry(&content, &member.path) {
            Some(updated) => content = updated,
            None => {
                output.print_error(&Error::Config {
                    message: format!(
                        "{} is matched by a glob in cabal.project, not listed by name",
                        member.name
                    ),
                    path: Some(project.cabal_project_path()),
                    source: None,
                    fixes: vec![Fix::new(
                        "Narrow the glob in the `packages:` field of cabal.project by hand",
                    )],
                });
                return Ok(3);
            }
        }

        let dependents = project.local_reverse_deps(&member.name);
        if !dependents.is_empty() {
            output.warn(&format!(
                "{} depend(s) on {}; it will now be resolved from Hackage",
                dependents.join(", "),
                member.name
            ));
        }
        removed.push(member.name.clone());
    }

    write_cabal_project(&root, &content)?;
    for name in &removed {
        output.status("Removed", name);
    }
    output.info("Update the workspace lockfile with: hx lock");
    Ok(0)
}

/// `hx workspace list`: members, versions, paths and dependencies between them.
pub async fn list(json: bool, output: &Output) -> Result<i32> {
    let root = find_project_root(".")?;
    let project = Project::load(&root)?;

    if json {
        let members: Vec<serde_json::Value> = project
            .workspace_packages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "version": m.version,
                    "path": display_path(&m.path),
                    "local_deps": project.local_deps(&m.name),
                    "executables": m.executables,
                    "test_suites": m.test_suites,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "root": project.root.display().to_string(),
            "members": members,
        });
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(0);
    }

    if !project.is_workspace() {
        output.info(&format!("{} is not a workspace", project.name()));
        output.info("Turn it into one with: hx workspace init");
        return Ok(0);
    }

    let rows: Vec<[String; 4]> = project
        .workspace_packages
        .iter()
        .map(|m| {
            let deps = project.local_deps(&m.name);
            [
                m.name.clone(),
                m.version.clone().unwrap_or_else(|| "-".to_string()),
                display_path(&m.path),
                if deps.is_empty() {
                    "-".to_string()
                } else {
                    deps.join(", ")
                },
            ]
        })
        .collect();
    let header = [
        "NAME".to_string(),
        "VERSION".to_string(),
        "PATH".to_string(),
        "DEPENDS ON".to_string(),
    ];
    let mut widths = header.clone().map(|h| h.len());
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.len());
        }
    }
    for row in std::iter::once(&header).chain(&rows) {
        let line = row
            .iter()
            .zip(widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect::<Vec<_>>()
            .join("  ");
        println!("{}", line.trim_end());
    }
    Ok(0)
}

/// `hx workspace affected [BASE]`: print the members affected by changes since
/// `BASE` (default `HEAD`), one per line — the set `--changed` would select.
pub async fn affected(base: String, output: &Output) -> Result<i32> {
    let root = find_project_root(".")?;
    let project = Project::load(&root)?;
    if !project.is_workspace() {
        output.info(&format!("{} is not a workspace", project.name()));
        return Ok(0);
    }
    let files = match changed_files(&project.root, &base).await {
        Ok(files) => files,
        Err(e) => {
            output.print_error(&e);
            return Ok(1);
        }
    };
    let names = match project.affected_by(&files) {
        AffectedMembers::All => project
            .package_names()
            .into_iter()
            .map(str::to_string)
            .collect(),
        AffectedMembers::Some(names) => names,
    };
    for name in names {
        println!("{name}");
    }
    Ok(0)
}

/// A member path for display (`.` for the root).
fn display_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("hx.toml"), "[project]\nname = \"ws\"\n").unwrap();
        fs::write(
            root.join("cabal.project"),
            "packages: ./lib-a ./lib-b ./app\n",
        )
        .unwrap();
        for (name, body) in [
            ("lib-a", "library\n  build-depends: base\n"),
            (
                "lib-b",
                "library\n  build-depends: lib-a\ntest-suite t\n  build-depends: lib-b\n",
            ),
            (
                "app",
                "executable app\n  build-depends: lib-b\ntest-suite app-test\n  build-depends: app\n",
            ),
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(
                root.join(name).join(format!("{name}.cabal")),
                format!("name: {name}\nversion: 0.1\n{body}"),
            )
            .unwrap();
        }
        let project = Project::load(root).unwrap();
        (dir, project)
    }

    #[test]
    fn selection_targets() {
        assert!(Selection::Single.targets().is_empty());
        assert_eq!(Selection::All.targets(), vec!["all"]);
        assert_eq!(
            Selection::Members(vec!["a".into(), "b".into()]).targets(),
            vec!["a", "b"]
        );
        assert!(Selection::All.is_everything());
        assert!(!Selection::Members(vec!["a".into()]).is_everything());
    }

    #[test]
    fn test_selection_drops_members_without_suites() {
        let (_dir, project) = workspace();
        let out = Output::new();
        assert_eq!(
            members_with_tests(&project, Selection::All, &out),
            Selection::Members(vec!["lib-b".into(), "app".into()])
        );
        assert_eq!(
            members_with_tests(&project, Selection::Members(vec!["lib-a".into()]), &out),
            Selection::Nothing
        );
    }
}
