//! Multi-package workspaces.
//!
//! A workspace is an hx project whose root holds a `cabal.project` listing
//! several local packages. `cabal.project` is the single source of truth for
//! membership: Cabal reads it directly, users already edit it by hand, and
//! `hx import --from stack` generates it. hx never keeps a second member list
//! in `hx.toml` that could drift out of sync; `hx workspace add/rm` edit the
//! `packages:` field in place and leave the rest of the file untouched.
//!
//! This module holds the pure pieces: reading and rewriting the `packages:`
//! field, resolving its entries (directories, `.cabal` paths and globs) to
//! member packages, and summarising each member's `.cabal` file so callers can
//! build the dependency graph between members.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use tracing::debug;

/// The Cabal project file that lists workspace members.
pub const CABAL_PROJECT: &str = "cabal.project";

/// Files at the workspace root whose change affects every member.
///
/// They configure the whole build (package set, constraints, compiler, the
/// lockfile), so a change to any of them invalidates every member.
pub const WORKSPACE_GLOBAL_FILES: &[&str] = &[
    "cabal.project",
    "cabal.project.local",
    "cabal.project.freeze",
    "hx.toml",
    "hx.lock",
];

/// What hx knows about a member from its `.cabal` file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CabalSummary {
    /// The `name:` field (empty if absent).
    pub name: String,
    /// The `version:` field, if present.
    pub version: Option<String>,
    /// Every package named in any `build-depends` field, in any component or
    /// conditional branch. Over-approximating is deliberate: the set is used
    /// to decide what a change can affect, where a missed edge is a bug and
    /// an extra edge only costs a rebuild.
    pub dependencies: BTreeSet<String>,
    /// Whether the package has a public `library` stanza.
    pub has_library: bool,
    /// Names of `executable` stanzas.
    pub executables: Vec<String>,
    /// Names of `test-suite` stanzas.
    pub test_suites: Vec<String>,
}

/// Summarise a `.cabal` file for workspace purposes.
///
/// This is a lightweight scan, not a full parser: it reads the package name
/// and version, the component stanzas, and the package names in every
/// `build-depends` field. Conditionals are not evaluated (see
/// [`CabalSummary::dependencies`]).
pub fn scan_cabal(content: &str) -> CabalSummary {
    let mut summary = CabalSummary::default();
    // Indentation of the active `build-depends` field, if any.
    let mut depends_indent: Option<usize> = None;
    let mut depends_buffer = String::new();

    fn flush(buffer: &mut String, summary: &mut CabalSummary) {
        for dep in buffer.split(',') {
            if let Some(name) = dependency_name(dep) {
                summary.dependencies.insert(name);
            }
        }
        buffer.clear();
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        if let Some(field_indent) = depends_indent {
            if indent > field_indent {
                depends_buffer.push(',');
                depends_buffer.push_str(trimmed);
                continue;
            }
            flush(&mut depends_buffer, &mut summary);
            depends_indent = None;
        }

        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            if key.eq_ignore_ascii_case("build-depends") {
                depends_indent = Some(indent);
                depends_buffer.push_str(value);
                continue;
            }
            if indent == 0 && !key.contains(char::is_whitespace) {
                let value = value.trim();
                if key.eq_ignore_ascii_case("name") {
                    summary.name = value.to_string();
                } else if key.eq_ignore_ascii_case("version") && !value.is_empty() {
                    summary.version = Some(value.to_string());
                }
                continue;
            }
        }

        if indent == 0 {
            let mut words = trimmed.split_whitespace();
            let keyword = words.next().unwrap_or("").to_ascii_lowercase();
            let name = words.next().map(str::to_string);
            match (keyword.as_str(), name) {
                ("library", None) => summary.has_library = true,
                ("executable", Some(n)) => summary.executables.push(n),
                ("test-suite", Some(n)) => summary.test_suites.push(n),
                _ => {}
            }
        }
    }
    flush(&mut depends_buffer, &mut summary);

    // A package's own name shows up when its executable or test suite depends
    // on its library; that is not an edge between members.
    let own = summary.name.clone();
    summary.dependencies.remove(&own);
    summary
}

/// The package name at the start of one `build-depends` entry
/// (`lib-a ^>=0.1`, `lib-a:internal`, `lib-a:{a, b}` all yield `lib-a`).
fn dependency_name(entry: &str) -> Option<String> {
    let name: String = entry
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!name.is_empty() && name.chars().any(|c| c.is_ascii_alphabetic())).then_some(name)
}

/// A field in a `cabal.project` file: the line it starts on, the line after
/// its last continuation line, and the entries it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldSpan {
    start: usize,
    end: usize,
    entries: Vec<String>,
}

/// Locate a top-level field (e.g. `packages`) in `cabal.project` content.
///
/// A field starts at column 0 with `name:`; every following line that is
/// indented (or is an indented comment) continues it. Entries are separated
/// by whitespace or commas.
fn find_field(lines: &[&str], field: &str) -> Option<FieldSpan> {
    let start = lines.iter().position(|line| {
        !line.starts_with(char::is_whitespace)
            && line
                .split_once(':')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(field))
    })?;

    let mut entries = Vec::new();
    let mut push_entries = |text: &str| {
        let text = text.split("--").next().unwrap_or("");
        entries.extend(
            text.split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        );
    };

    let (_, first) = lines[start].split_once(':').unwrap_or(("", ""));
    push_entries(first);

    let mut end = start + 1;
    while end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() || !line.starts_with(char::is_whitespace) {
            break;
        }
        if !line.trim_start().starts_with("--") {
            push_entries(line);
        }
        end += 1;
    }

    Some(FieldSpan {
        start,
        end,
        entries,
    })
}

/// The entries of the `packages:` field, exactly as written.
pub fn package_entries(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    find_field(&lines, "packages")
        .map(|f| f.entries)
        .unwrap_or_default()
}

/// The entries of the `optional-packages:` field, exactly as written.
pub fn optional_package_entries(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    find_field(&lines, "optional-packages")
        .map(|f| f.entries)
        .unwrap_or_default()
}

/// Normalise an entry or path for comparison: drop `./`, trailing slashes and
/// `.` components (`./lib-a/` and `lib-a` are the same member).
pub fn normalize_entry(entry: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for component in Path::new(entry).components() {
        match component {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The member directory an entry names: the entry itself, or the parent of a
/// `.cabal` file path.
fn entry_dir(entry: &str) -> PathBuf {
    let path = normalize_entry(entry);
    if path.extension().is_some_and(|e| e == "cabal") {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        path
    }
}

/// Whether an entry is a glob pattern rather than a literal path.
pub fn is_glob(entry: &str) -> bool {
    entry.contains(['*', '?', '[', '{'])
}

/// Whether an entry is a remote or tarball source rather than a local
/// directory (`https://…`, `foo-1.0.tar.gz`); those are not members.
fn is_remote_or_archive(entry: &str) -> bool {
    entry.contains("://") || entry.ends_with(".tar.gz") || entry.ends_with(".tgz")
}

/// Render `packages:` with `entries`, one per line.
fn render_packages_field(entries: &[String]) -> Vec<String> {
    let mut out = vec!["packages:".to_string()];
    out.extend(entries.iter().map(|e| format!("  {e}")));
    out
}

/// Rebuild `content` with the `packages:` field replaced by `entries`.
///
/// Everything outside the field (other fields, `package` stanzas, comments)
/// is preserved byte for byte.
fn with_packages_field(content: &str, entries: &[String]) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    match find_field(&lines, "packages") {
        Some(span) => {
            out.extend(lines[..span.start].iter().map(|s| s.to_string()));
            out.extend(render_packages_field(entries));
            out.extend(lines[span.end..].iter().map(|s| s.to_string()));
        }
        None => {
            // No field yet: put it first, after any leading comment header.
            let header = lines
                .iter()
                .take_while(|l| l.trim_start().starts_with("--"))
                .count();
            out.extend(lines[..header].iter().map(|s| s.to_string()));
            out.extend(render_packages_field(entries));
            if lines.get(header).is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.extend(lines[header..].iter().map(|s| s.to_string()));
        }
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

/// Add `entry` to the `packages:` field, creating the field if needed.
///
/// Returns `None` when an entry naming the same directory is already listed.
pub fn add_package_entry(content: &str, entry: &str) -> Option<String> {
    let mut entries = package_entries(content);
    let dir = entry_dir(entry);
    if entries.iter().any(|e| !is_glob(e) && entry_dir(e) == dir) {
        return None;
    }
    entries.push(entry.to_string());
    Some(with_packages_field(content, &entries))
}

/// Remove the literal entry naming `dir` (relative to the workspace root)
/// from the `packages:` field.
///
/// Returns `None` when no literal entry names it (it may be matched by a glob,
/// which `hx` will not rewrite).
pub fn remove_package_entry(content: &str, dir: &Path) -> Option<String> {
    let entries = package_entries(content);
    let dir = normalize_entry(&dir.to_string_lossy());
    let kept: Vec<String> = entries
        .iter()
        .filter(|e| is_glob(e) || entry_dir(e) != dir)
        .cloned()
        .collect();
    (kept.len() != entries.len()).then(|| with_packages_field(content, &kept))
}

/// A member resolved from a `cabal.project` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMember {
    /// Member directory, relative to the workspace root (empty for the root).
    pub path: PathBuf,
    /// The member's `.cabal` file (absolute).
    pub cabal_file: PathBuf,
    /// The entry that produced it.
    pub entry: String,
}

/// Resolve `packages:` and `optional-packages:` entries to member packages.
///
/// Literal directories, `.cabal` file paths and globs are supported; remote
/// and tarball entries are skipped. A missing `optional-packages` entry is not
/// an error (that is what optional means); a missing `packages` entry is
/// skipped with a debug log, as Cabal itself will report it.
pub fn resolve_members(root: &Path, content: &str) -> Vec<ResolvedMember> {
    let mut members: Vec<ResolvedMember> = Vec::new();
    let entries = package_entries(content)
        .into_iter()
        .chain(optional_package_entries(content));

    for entry in entries {
        if is_remote_or_archive(&entry) {
            debug!("Skipping non-local cabal.project entry: {}", entry);
            continue;
        }
        let candidates: Vec<PathBuf> = if is_glob(&entry) {
            let pattern = root.join(normalize_entry(&entry));
            match glob::glob(&pattern.to_string_lossy()) {
                Ok(paths) => {
                    let mut paths: Vec<PathBuf> = paths.flatten().collect();
                    paths.sort();
                    paths
                }
                Err(e) => {
                    debug!("Invalid glob in cabal.project ({}): {}", entry, e);
                    Vec::new()
                }
            }
        } else {
            vec![root.join(normalize_entry(&entry))]
        };

        for candidate in candidates {
            let resolved = if candidate.extension().is_some_and(|e| e == "cabal") {
                candidate
                    .is_file()
                    .then(|| (candidate.parent().map(Path::to_path_buf), candidate.clone()))
                    .and_then(|(dir, file)| dir.map(|d| (d, file)))
            } else if candidate.is_dir() {
                find_cabal_file(&candidate).map(|file| (candidate.clone(), file))
            } else {
                None
            };

            let Some((dir, cabal_file)) = resolved else {
                debug!("No package at cabal.project entry: {}", candidate.display());
                continue;
            };
            if members.iter().any(|m| m.cabal_file == cabal_file) {
                continue;
            }
            let path = dir.strip_prefix(root).unwrap_or(&dir);
            members.push(ResolvedMember {
                path: normalize_entry(&path.to_string_lossy()),
                cabal_file,
                entry: entry.clone(),
            });
        }
    }
    members
}

/// Find the `.cabal` file in a directory (the first, in name order, so the
/// choice is deterministic).
pub fn find_cabal_file(dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "cabal") && p.is_file())
        .collect();
    files.sort();
    files.into_iter().next()
}

/// Package directories directly below `root` (one level) that contain a
/// `.cabal` file, plus `root` itself when it does. Used by `hx workspace init`
/// to adopt an existing multi-package layout. Hidden and build directories
/// are skipped.
pub fn discover_package_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if find_cabal_file(root).is_some() {
        dirs.push(PathBuf::new());
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    children.sort();
    for child in children {
        let Some(name) = child.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || name == "dist-newstyle" || !child.is_dir() {
            continue;
        }
        if find_cabal_file(&child).is_some() {
            dirs.push(PathBuf::from(name));
        }
    }
    dirs
}

/// Render a `cabal.project` entry for a member directory relative to the root.
pub fn entry_for_dir(dir: &Path) -> String {
    let dir = normalize_entry(&dir.to_string_lossy());
    if dir.as_os_str().is_empty() {
        ".".to_string()
    } else {
        format!("./{}", dir.to_string_lossy().replace('\\', "/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const LIB_B_CABAL: &str = r#"cabal-version: 3.0
name:          lib-b
version:       0.2.0

common shared
  build-depends: base >=4 && <5

library
  import:          shared
  exposed-modules: LibB
  build-depends:
      lib-a ^>=0.1
    , text
  if flag(fast)
    build-depends: containers

test-suite lib-b-test
  type:          exitcode-stdio-1.0
  main-is:       Spec.hs
  build-depends: lib-b, hspec, test-utils:helpers
"#;

    #[test]
    fn scan_cabal_reads_name_version_components_and_all_deps() {
        let s = scan_cabal(LIB_B_CABAL);
        assert_eq!(s.name, "lib-b");
        assert_eq!(s.version.as_deref(), Some("0.2.0"));
        assert!(s.has_library);
        assert_eq!(s.test_suites, vec!["lib-b-test"]);
        assert!(s.executables.is_empty());
        let deps: Vec<&str> = s.dependencies.iter().map(String::as_str).collect();
        // Conditional, common-stanza and test-suite deps all count; the
        // package's own name does not.
        assert_eq!(
            deps,
            vec!["base", "containers", "hspec", "lib-a", "test-utils", "text"]
        );
    }

    #[test]
    fn scan_cabal_executables() {
        let s = scan_cabal(
            "name: app\nversion: 1\n\nexecutable app\n  main-is: Main.hs\n  build-depends: base, lib-b\n",
        );
        assert_eq!(s.executables, vec!["app"]);
        assert!(!s.has_library);
        assert!(s.dependencies.contains("lib-b"));
    }

    #[test]
    fn package_entries_single_line_and_continuations() {
        let content = "-- comment\npackages: ./lib-a lib-b,\n          ./app/\n  -- note\n          apps/*\n\npackage lib-a\n  ghc-options: -Wall\n";
        assert_eq!(
            package_entries(content),
            vec!["./lib-a", "lib-b", "./app/", "apps/*"]
        );
    }

    #[test]
    fn package_entries_ignores_package_stanzas_and_other_fields() {
        // The old parser treated any colon-free line after `packages:` as a
        // continuation, picking up junk from later stanzas.
        let content = "packages: ./a\nwith-compiler: ghc-9.8.2\n\nsource-repository-package\n  type: git\n  location: https://example.com/x\n";
        assert_eq!(package_entries(content), vec!["./a"]);
    }

    #[test]
    fn add_entry_preserves_rest_of_file() {
        let content = "-- header\npackages: ./lib-a\n\nwith-compiler: ghc-9.8.2\n";
        let out = add_package_entry(content, "./lib-b").unwrap();
        assert_eq!(
            out,
            "-- header\npackages:\n  ./lib-a\n  ./lib-b\n\nwith-compiler: ghc-9.8.2\n"
        );
        assert_eq!(package_entries(&out), vec!["./lib-a", "./lib-b"]);
    }

    #[test]
    fn add_entry_is_idempotent_across_spellings() {
        let content = "packages: lib-a/\n";
        assert!(add_package_entry(content, "./lib-a").is_none());
        assert!(add_package_entry(content, "./lib-a/lib-a.cabal").is_none());
    }

    #[test]
    fn add_entry_creates_field_after_header_comments() {
        let out = add_package_entry("-- hx workspace\n\nwith-compiler: ghc\n", "./a").unwrap();
        assert_eq!(
            out,
            "-- hx workspace\npackages:\n  ./a\n\nwith-compiler: ghc\n"
        );
        let out = add_package_entry("", "./a").unwrap();
        assert_eq!(out, "packages:\n  ./a\n");
    }

    #[test]
    fn remove_entry_by_dir_and_refuses_globs() {
        let content = "packages: ./lib-a ./lib-b libs/*\n";
        let out = remove_package_entry(content, Path::new("lib-b")).unwrap();
        assert_eq!(package_entries(&out), vec!["./lib-a", "libs/*"]);
        assert!(remove_package_entry(content, Path::new("libs/x")).is_none());
    }

    fn write_pkg(root: &Path, dir: &str, name: &str) {
        let d = root.join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join(format!("{name}.cabal")),
            format!("name: {name}\nversion: 0.1\n"),
        )
        .unwrap();
    }

    #[test]
    fn resolve_members_handles_dirs_cabal_files_and_globs() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_pkg(root, "lib-a", "lib-a");
        write_pkg(root, "libs/x", "x");
        write_pkg(root, "libs/y", "y");
        write_pkg(root, "app", "app");
        let content = "packages: ./lib-a\n  libs/*\n  app/app.cabal\n  ./missing\n  https://example.com/p.tar.gz\noptional-packages: ./vendor/*\n";
        let members = resolve_members(root, content);
        let paths: Vec<_> = members.iter().map(|m| m.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("lib-a"),
                PathBuf::from("libs/x"),
                PathBuf::from("libs/y"),
                PathBuf::from("app"),
            ]
        );
        assert_eq!(members[1].entry, "libs/*");
    }

    #[test]
    fn resolve_members_root_package_and_dedup() {
        let tmp = tempdir().unwrap();
        write_pkg(tmp.path(), "", "root-pkg");
        let members = resolve_members(tmp.path(), "packages: . ./ ./root-pkg.cabal\n");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].path, PathBuf::new());
    }

    #[test]
    fn discover_package_dirs_finds_children() {
        let tmp = tempdir().unwrap();
        write_pkg(tmp.path(), "b", "b");
        write_pkg(tmp.path(), "a", "a");
        write_pkg(tmp.path(), ".hidden", "h");
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        assert_eq!(
            discover_package_dirs(tmp.path()),
            vec![PathBuf::from("a"), PathBuf::from("b")]
        );
    }

    #[test]
    fn entry_for_dir_renders_relative_paths() {
        assert_eq!(entry_for_dir(Path::new("lib-a")), "./lib-a");
        assert_eq!(entry_for_dir(Path::new("./libs/x/")), "./libs/x");
        assert_eq!(entry_for_dir(Path::new("")), ".");
    }
}
