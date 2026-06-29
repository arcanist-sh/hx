# BHC Native Builds & Package Databases

How hx builds Haskell projects and their dependencies with the **BHC (Basel
Haskell Compiler)** backend — separate compilation, package databases, and the
flags that tie them together.

> **New here?** See [BHC_PLATFORM.md](./BHC_PLATFORM.md) for choosing a BHC
> Platform snapshot and [GETTING_STARTED.md](./GETTING_STARTED.md) for project
> basics.

## Overview

With `backend = "bhc"`, hx owns the build graph: it resolves dependencies,
compiles each one from source into a **package database** of compiled interface
files, and then builds your project against that database. This mirrors what
GHC + `ghc-pkg` do, using BHC's separate-compilation pipeline.

```toml
[compiler]
backend = "bhc"
```

```bash
hx build --backend bhc --native     # build deps from source, then the project
```

## Separate compilation in BHC

BHC compiles one module at a time and emits a binary **interface** (`.bhi`)
alongside the object file:

```bash
bhc -c Data/Split.hs --hidir build/hi --odir build/o
# writes build/hi/Data/Split.bhi  and  build/o/Data/Split.o
```

Artifacts are laid out by **module path**, so `Data.Split` becomes
`Data/Split.bhi` / `Data/Split.o`. A later module (or project) can then be
type-checked or compiled against that interface without the dependency's source
being present:

```bash
bhc check Main.hs --package-db build/hi      # resolves `import Data.Split`
```

`bhc check` and `bhc -c` both consult package databases, so the same dependency
set is used whether you are type-checking or compiling.

## Package databases

A **package database** is a directory BHC searches for interfaces. Two layouts
are supported.

### Flat layout

The directory directly contains the `.bhi` tree, exactly as emitted by
`bhc -c --hidir <dir>`:

```
build/hi/
  Data/Split.bhi
```

Point `bhc` at it with `--package-db build/hi`. Any interface found this way
resolves. This is convenient for ad-hoc builds.

### Registered layout (what hx produces)

hx installs each package and writes a GHC-style registration file
(`<package-id>.conf`) into the database directory:

```
<cache>/bhc-<version>/package.db/
  mysplit-1.0.0-ab12cd34ef56.conf
  text-2.1.0-....conf
```

Each `.conf` records the package's identity and where its interfaces live:

```
name: mysplit
version: 1.0.0
id: mysplit-1.0.0-ab12cd34ef56
import-dirs: <cache>/bhc-<version>/lib/mysplit-1.0.0-ab12cd34ef56/lib
exposed-modules: Data.Split
depends:
```

When BHC reads a registered database it honors these fields:

- **`import-dirs`** — where the package's `.bhi` files live.
- **`exposed-modules`** — only these modules resolve from the package; a module
  not listed is hidden even if its `.bhi` happens to exist.
- **`depends`** — the package's own dependencies (see scoping below).

## Flags reference

These BHC flags are what hx passes; you can also use them directly.

| Flag | Applies to | Meaning |
|------|-----------|---------|
| `--package-db <dir>` | check, build, `-c`, repl | A package database directory (flat or registered). Repeatable. |
| `--package-id <id>` | check, build, `-c`, repl | Expose a specific registered package (and its `depends` closure). Repeatable. |
| `--package-dir <dir>` | check | Dependency **source** root, resolved without a database (see below). Repeatable. |
| `-I`, `--import-path <dir>` | check, build, `-c`, repl | Additional module/source search paths. Repeatable. |
| `--hidir <dir>` | `-c` | Output directory for `.bhi` interfaces. |
| `--odir <dir>` | `-c` | Output directory for `.o` objects. |
| `--tensor-fusion` | build, `-c` | Request the tensor fusion pipeline (see note). |

`--package-db`, `--package-id`, and `--tensor-fusion` are global, so they may
appear before or after a subcommand (`bhc check Main.hs --package-db db` and
`bhc --package-db db check Main.hs` are equivalent).

### Visibility scoping with `--package-id`

By default every package registered in the supplied databases is visible. When
one or more `--package-id` flags are given, visibility narrows to the
**transitive closure** of those ids over each package's `depends:` field:

- Selecting package **P** also exposes everything P depends on.
- Selecting a dependency of P does **not** expose P.
- Module resolution still respects `exposed-modules` for every visible package.

This matches GHC semantics and lets hx expose exactly a project's direct
dependencies while their transitive deps remain resolvable.

### Resolving against source roots without a database

For quick checks or vendored dependencies you can skip the database entirely and
point `bhc check` at dependency **source** directories:

```bash
bhc check app/src --package-dir deps/split/src --package-dir deps/text/src
```

The dependency modules are parsed and checked so the target's imports resolve,
but they are not reported in the results — only the modules under the target
paths are. This is the no-network path: no `.bhi`, no registration, just source.

### Tensor fusion

`--tensor-fusion` is accepted for toolchain compatibility. Tensor fusion is
governed by the **numeric** profile (which always fuses); the flag requests the
fusion pipeline explicitly and is a no-op on the per-module `-c` path. Set the
profile for numeric work:

```toml
[compiler.bhc]
profile = "numeric"
```

## How `hx build --backend bhc --native` works

1. **Resolve** — hx reads the cached resolution from `hx.lock` (run `hx lock`
   first) for the project's dependencies. BHC builtin packages (`base`,
   `ghc-prim`, …) are treated as pre-installed and are never fetched or built.
2. **Fetch** — dependency sources are fetched from Hackage.
3. **Plan** — a topologically ordered build plan is generated.
4. **Build dependencies** — each package is compiled from source with
   `bhc -c` into `<cache>/bhc-<version>/lib/<package-id>/lib`, its interfaces
   are installed there, and a `.conf` is registered in the package database.
   Each package is compiled **against the in-progress database**, so a
   dependency can resolve the dependencies built before it.
5. **Build the project** — the local project is compiled with `--package-db`
   pointing at the database and a `--package-id` for each direct dependency.

If the project has no external dependencies, no cached resolution exists, or
fetching fails, hx falls back to a **local-only** build (compiling just the
project's own modules). So offline builds and projects without a lockfile keep
working.

### Running

```bash
hx run --backend bhc --native -- <args>
```

`hx run --native` performs the build above and then **executes the produced
native binary**, forwarding any program arguments. The native path is required
to run code that calls into dependencies: the interpreter path (`hx run`
without `--native`) only has interface (`.bhi`) information for imported
packages, not their compiled bodies.

### Where things live

| Path | Contents |
|------|----------|
| `<cache>/bhc-<version>/package.db/` | Registered `.conf` files (the package database) |
| `<cache>/bhc-<version>/lib/<id>/lib/` | A package's installed `.bhi` interfaces and `libHS<id>.a` |
| `<project>/.hx/bhc-native-build/` | The project's own build output |

`<cache>` is the platform cache directory (e.g. `~/Library/Caches/hx` on macOS,
`~/.cache/hx` on Linux).

## REPL

`hx` launches the BHC REPL via `bhc repl`, forwarding profile, import paths, and
package databases:

```bash
bhc --import-path src repl --package-db <db> --package-id <id>
```

Inside the REPL:

- `:load Foo.hs` resolves the file against the configured `--import-path`
  directories when it is not found relative to the current directory.
- `:show packages` lists the configured package databases, package ids, and
  import paths.

> The REPL's evaluator is still under development and does not yet resolve
> imports out of package-database interfaces; the flags above are accepted and
> wired through for forward compatibility and `:load` resolution.

## Status

| Capability | State |
|------------|-------|
| `bhc -c` separate compilation (`.bhi`/`.o`, module-path layout) | ✅ |
| `bhc check`/build consuming flat package databases | ✅ |
| Consuming hx's registered (`.conf`) databases | ✅ |
| `--package-id` scoping + `exposed-modules` gating + transitive `depends` | ✅ |
| `--package-dir` no-network source resolution | ✅ |
| `--tensor-fusion` flag accepted | ✅ |
| REPL package/import flags | ✅ |
| hx transitive dependency build into a package DB | ✅ |
| `hx build --backend bhc --native` orchestration | ✅ |
| Live end-to-end build against Hackage | requires network; run `hx lock && hx build --backend bhc --native` to exercise |

## Troubleshooting

### `bhc native build: no cached resolution — run hx lock first`

The dependency-aware build needs a resolved dependency set. Run `hx lock`, then
build again. Without it, hx does a local-only build.

### An import is `SKIPPED` / unresolved during `bhc check`

The module's package isn't visible. Either it isn't in any supplied
`--package-db`, it isn't listed in the providing package's `exposed-modules`, or
a `--package-id` you passed scopes it out. Check `:show packages` in the REPL or
the `.conf` files in the database directory.

### BHC not installed

```
error: BHC is not installed
  Install BHC with: hx toolchain install --bhc latest
```
