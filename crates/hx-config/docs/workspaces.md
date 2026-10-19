# Workspaces

hx supports multi-package workspaces (monorepos). User-facing documentation
lives in [docs/WORKSPACES.md](../../../docs/WORKSPACES.md); this page covers the
`hx-config` API.

## Membership

A project is a workspace when its root `cabal.project` lists at least one local
package. `cabal.project` is the only member list — `hx.toml` does not duplicate
it. `Project::load` resolves the `packages:` and `optional-packages:` fields
(directories, `.cabal` paths and globs) via
[`workspace::resolve_members`](../src/workspace.rs).

```rust
let project = Project::load(&root)?;

if project.is_workspace() {
    for pkg in &project.workspace_packages {
        println!("{} {:?} at {}", pkg.name, pkg.version, pkg.path.display());
    }
}
```

## WorkspacePackage

```rust
pub struct WorkspacePackage {
    pub name: String,              // `name:` from the .cabal file
    pub path: PathBuf,             // relative to the root; empty for `packages: .`
    pub cabal_file: PathBuf,
    pub version: Option<String>,
    pub dependencies: Vec<String>, // every build-depends name, all components/branches
    pub executables: Vec<String>,
    pub test_suites: Vec<String>,
}
```

## Member graph and selection

| Method | Purpose |
|--------|---------|
| `local_deps(name)` | Members `name` depends on directly |
| `local_reverse_deps(name)` | Members that depend on `name` directly |
| `with_reverse_deps(names)` | `names` plus all transitive dependents, in workspace order |
| `member_owning(path)` | Member whose directory contains a root-relative path |
| `member_at(dir)` | Member containing an absolute directory (ignores a root member) |
| `affected_by(files)` | `AffectedMembers::All` for workspace-wide files, else the touched members plus dependents |

## Root discovery

`find_project_root` walks up to the nearest `hx.toml`. If that directory is
itself a member of an enclosing workspace (an ancestor with `hx.toml` and a
`cabal.project` listing it), the workspace root is returned instead, so a
member's own `hx.toml` never splits the workspace.

## Editing cabal.project

`workspace::add_package_entry` and `workspace::remove_package_entry` rewrite
only the `packages:` field and preserve the rest of the file. Entries matched
by a glob are never rewritten (`remove_package_entry` returns `None`).
