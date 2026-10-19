# Workspaces

A workspace is one hx project made of several Haskell packages: a monorepo with
a shared toolchain, one build directory, and one lockfile. hx builds on Cabal's
own multi-package projects, so a workspace is an ordinary `cabal.project` that
both `cabal` and `hx` understand.

```
my-workspace/
├── hx.toml          # Workspace settings: toolchain, build, plugins (shared)
├── hx.lock          # One lockfile for every member
├── cabal.project    # The member list
├── lib-a/
│   └── lib-a.cabal
├── lib-b/           # depends on lib-a
│   └── lib-b.cabal
└── app/             # depends on lib-b
    └── app.cabal
```

## Creating a workspace

```bash
hx workspace init            # Make the current directory a workspace root
hx workspace init monorepo   # ...or a new/other directory
```

`init` writes `hx.toml` and `cabal.project` if they don't exist yet. Package
directories that already sit directly below the root (and the root itself, if
it has a `.cabal` file) become members automatically, so running it in an
existing multi-package repository adopts the layout as is. Existing files are
never overwritten.

To add a new package:

```bash
hx init --lib lib-c
hx workspace add lib-c
```

## Managing members

| Command | What it does |
|---------|--------------|
| `hx workspace add <path>...` | Add package directories (each with a `.cabal` file) to `cabal.project` |
| `hx workspace rm <name\|path>...` | Remove members from `cabal.project`; files stay where they are |
| `hx workspace list [--json]` | Members with version, path, and the members each one depends on |
| `hx workspace affected [BASE]` | Print the members `--changed` would select (see below) |

```console
$ hx workspace list
NAME   VERSION  PATH   DEPENDS ON
app    0.1.0    app    lib-b
lib-a  0.1.0    lib-a  -
lib-b  0.1.0    lib-b  lib-a
```

`hx workspace rm` warns when other members depend on the one you remove, since
their dependency will then be resolved from Hackage instead.

### Where membership lives

`cabal.project` is the single source of truth for membership. Cabal reads it
directly, you can edit it by hand, and `hx import --from stack` generates it.
`hx.toml` does not keep a second member list that could drift out of sync;
`hx workspace add/rm` only rewrite the `packages:` field and leave the rest of
the file (constraints, `package` stanzas, comments) as it was.

hx understands these `packages:` (and `optional-packages:`) entries:

- directories: `./lib-a`, `lib-a/`
- `.cabal` file paths: `./lib-a/lib-a.cabal`
- globs: `libs/*`, `*/*.cabal`

Remote and tarball entries are not workspace members and are ignored. A member
matched by a glob can't be removed with `hx workspace rm`; narrow the glob by
hand instead.

### Settings

The root `hx.toml` applies to every member: `[toolchain]`, `[build]`,
`[plugins]` and the rest. If a member directory has its own `hx.toml` (for
example one created by `hx init` before it joined the workspace), hx still uses
the workspace root when you run commands inside it, the same way Cargo does.
The member's file is not read.

## Building, testing and running

| Where / how | What gets built |
|-------------|-----------------|
| `hx build` at the root | Every member (`cabal build all`) |
| `hx build -p lib-a -p app` | The named members (repeatable) |
| `hx build` inside `lib-b/` (or any subdirectory) | Only `lib-b` |
| `hx build --changed [BASE]` | Members affected by changes (see below) |

`hx check` takes the same `-p`/`--changed` options. `hx test` takes
`--package` (`-p` means `--pattern` there) and `--changed`. It skips members
that have no `test-suite`, so `cabal test` never fails just because a library
has no tests.

Commands always run from the workspace root, even when you start them inside a
member. That way they share one build directory, the root `hx.lock`, and the
root toolchain pin.

`hx run` runs the member you're standing in, else the only member with an
executable. If several members have one, it asks you to choose with
`hx run -p <name>`.

## Selective rebuilds: `--changed`

```bash
hx test --changed                # Members touched by uncommitted changes
hx test --changed origin/main    # Members a branch touches (CI)
hx build --changed origin/main
```

`--changed` asks git which files changed and maps them to members:

1. **Changed files** are the tracked changes (staged or not) plus untracked
   files. They are compared to `HEAD` by default. When you give a `BASE`, they
   are compared to the merge base of `BASE` and `HEAD`, so a branch is measured
   from where it forked and not against whatever landed on `BASE` since.
2. **Each file belongs to the member whose directory contains it.** Files
   outside every member, such as `README.md` or CI config, affect nothing.
3. **Workspace-wide files** (`cabal.project`, `cabal.project.local`,
   `cabal.project.freeze`, `hx.toml` and `hx.lock` at the root) affect every
   member.
4. **Reverse dependencies are added.** A change to `lib-a` also selects
   `lib-b` and `app`, because they depend on it directly or transitively.

If nothing is affected, the command says so and exits successfully without
invoking Cabal.

**Why this matters mostly for tests.** Cabal already rebuilds incrementally:
building `app` recompiles `lib-a` only when it changed, and an unchanged member
is a quick no-op. So `hx build --changed` saves only Cabal's per-member
up-to-date checks. The big saving is in `hx test --changed`, which skips the
test suites of every unaffected member. That's where CI time goes in a large
workspace.

Dependencies between members are read from every `build-depends` field in each
member's `.cabal` file: all components and all conditional branches. This
over-approximates on purpose. An extra edge only costs a rebuild, while a
missed one would skip a test that should have run.

## Locking

`hx lock` at the root (or anywhere inside the workspace) writes one `hx.lock`
for the whole workspace. Dependencies are collected from every member's
`.cabal` file. Members are never sent to the Hackage solver, even when they
depend on each other. They're recorded separately in the lockfile:

```toml
[workspace]
is_workspace = true

[[workspace.packages]]
name = "lib-a"
version = "0.1.0"
path = "lib-a"
```

After `hx workspace add` or `rm`, run `hx lock` again. `hx sync` warns when
the member set has changed since the lockfile was written.

## Current limitations

- **Per-member compiler settings.** All members share the root `[compiler]`
  configuration. Per-member BHC profiles (a `server` member next to a
  `default` library) are planned but not supported yet.
- **BHC and `--native` builds** work on single-package projects only; in a
  workspace use the default Cabal-backed build.
- **`--changed` needs git.** Outside a git repository (or in one with no
  commits) it reports an error with a suggested fix.
