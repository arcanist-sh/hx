# Lockfile (`hx.lock`) reference

`hx.lock` makes a build reproducible and verifiable: it records exactly which
sources a build uses, the hash each source must have, and the context the
dependency plan was resolved for. Commit it.

| Command | What it does |
|---------|--------------|
| `hx lock` | Resolve dependencies and write `hx.lock` (versions, hashes, revisions, index state) |
| `hx lock --cabal` | Same, using `cabal freeze` as the resolver |
| `hx lock --check` | Verify `hx.lock` without changing it; non-zero exit on any mismatch (for CI) |
| `hx fetch` | Download every locked source in parallel and verify it against its hash |
| `hx build`, `test`, `run`, `check`, `repl` | Build with the locked versions, flags and index state when `hx.lock` is current |
| `hx build --locked` | Same, but a missing or out-of-date `hx.lock` is an error, and cached sources must match their hashes |
| `hx sync` | Build with the locked versions, flags and index state |
| `hx sync --offline` | Same, but only from cached sources; fails up front if anything is missing |

## Example

```toml
version = 2
created_at = "2026-10-19T08:09:08Z"

[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"

[plan]
compiler_id = "ghc-9.8.2"
platform = "aarch64-macos"
index_state = "2026-10-19T07:18:30Z"
hash = "sha256:6215fda95e4f4094a0473432803f67c1f412aea9e55fb20436c6391b52a15c3e"

[[packages]]
name = "containers"
version = "0.8.1"
source = "hackage"
sha256 = "9bd0f5156306b04a799c32ff49a6129f7c8dd6ec96c6ddeb7885975ffffc9e99"
revision = 0
cabal_sha256 = "11cc66be32854dc6b9c04c1e4f9a3ecb4939399a3b54bf00c6c085d538470b12"

[[packages]]
name = "deepseq"
version = "1.5.0.0"
source = "ghc"
```

## Fields

### Top level

| Field | Description |
|-------|-------------|
| `version` | Lockfile format version (currently `2`, see [Versioning](#versioning)) |
| `created_at` | When the pins last changed. Re-running `hx lock` without changes leaves the file untouched, so this only moves when the content does |

### `[toolchain]`

| Field | Description |
|-------|-------------|
| `ghc` | GHC version detected when locking |
| `cabal` | Cabal version detected when locking |
| `bhc` | BHC version (BHC projects only: `[compiler].version`, else the detected `bhc`) |
| `bhc_profile` | BHC optimization profile from `[compiler.bhc].profile` (BHC projects only) |

### `[plan]`

| Field | Description |
|-------|-------------|
| `compiler_id` | Compiler the plan was resolved for, e.g. `ghc-9.8.2`. `.cabal` conditionals such as `impl(ghc >= 9.6)` were evaluated against it |
| `platform` | Platform the plan was resolved for (`<arch>-<os>`). `os(...)`/`arch(...)` conditionals were evaluated against it |
| `index_state` | Hackage index state (RFC 3339) the plan was resolved against. Builds pass it to Cabal as `--index-state`, so they see the same `.cabal` revisions |
| `snapshot` | Stackage snapshot, if `[stackage].snapshot` is set |
| `bhc_platform` | BHC Platform snapshot, if `[bhc-platform].snapshot` is set on a BHC project |
| `hash` | Fingerprint over all of the above and every package pin (used for build caching) |

### `[[packages]]`

Packages are sorted by name, and each field is written only when it has a
value, so diffs stay small.

| Field | Description |
|-------|-------------|
| `name`, `version` | Exact package version |
| `source` | `hackage`, `stackage` (a Hackage package pinned by the snapshot), `ghc` (the installed GHC's own copy, never fetched), `git`, or `path` |
| `sha256` | SHA-256 of the source tarball (`hackage`/`stackage`), lowercase hex |
| `revision` | Hackage `.cabal` revision the plan used (`0` = as uploaded) |
| `cabal_sha256` | SHA-256 of that `.cabal` revision |
| `flags` | Cabal flag assignments, `{ flag = true/false }` |
| `git`, `rev`, `subdir` | Repository URL, full 40-character commit and subdirectory (`git` sources) |
| `path` | Directory relative to the project root (`path` sources) |

**Why revisions are pinned separately.** Hackage trustees can publish a
revised `.cabal` file for a release (usually to fix version bounds) without
touching the tarball. The tarball hash alone therefore does not pin a build's
metadata; `revision` + `cabal_sha256` do, and `index_state` makes Cabal pick
that revision.

**Where the hashes come from.** `hx lock` reads them from the Hackage index
itself: every release's signed `package.json` entry carries its tarball hash,
and each `.cabal` revision is a separate index entry. No tarball is downloaded
to lock. With `--cabal`, hx reads Cabal's own index, at the `index-state` the
freeze file records if it has one.

**Packages fixed by GHC.** When `ghc-pkg` for the locked GHC version is on
`PATH`, the native resolver pins the compiler's built-in packages
(`base`, `template-haskell`, ...) and everything they depend on in the global
package database (e.g. `deepseq`, `pretty`) to the installed versions, recorded
with `source = "ghc"`. Cabal cannot choose other versions of these, so a plan
that ignored them could not be built.

`flags` are recorded by `hx lock --cabal` (from the freeze file). The native
resolver evaluates flags but does not yet record them, and `hx lock` does not
yet produce `git`/`path` entries; the format and `hx lock --check` support
them so that importers and future resolvers can.

## Versioning

`hx lock` writes format **v2**. hx still reads **v1** lockfiles (old files keep
working; a v1 `hash = "sha256:..."` is read as `sha256`), and the next
`hx lock` upgrades the file in place, reporting
`Upgraded hx.lock from format v1 to v2`. A v1 lockfile has no source hashes,
so `hx sync --offline` refuses it and `hx lock --check` reports it as out of
date. A lockfile from a newer hx (format above v2) is rejected with a request
to upgrade hx instead of being misread.

## Offline and air-gapped builds

1. **Lock** (online): `hx lock`, and commit `hx.lock`.
2. **Fetch** (online): `hx fetch` downloads every locked Hackage tarball into
   hx's source cache (`~/.cache/hx/packages`), verifies each against its
   `sha256`, and copies it into Cabal's download cache (`$CABAL_DIR/packages`,
   `~/.cabal/packages` or `~/.cache/cabal/packages`, whichever Cabal uses). If
   Cabal's copy of the Hackage index is missing, `hx fetch` runs
   `cabal update`.
3. **Build** (offline): `hx sync --offline`, or `HX_OFFLINE=1 hx sync`.

Before building, `hx sync --offline` checks that every locked source is in
Cabal's cache with the pinned hash and that Cabal's index is present. If not,
it stops and lists what is missing:

```
error: 2 locked source packages are not available offline
  - containers-0.8.1
  - data-default-0.8.0.2
fix: Run `hx fetch` while online to download and verify them
```

hx then runs `cabal build` with `--index-state` and one `--constraint` per
locked package. It does not pass Cabal's own `--offline` flag: cabal-install
(checked with 3.12.1.0) rejects every package that is not yet built in the
store under `--offline`, even when its tarball is already cached, so first
builds would always fail. With every source cached and the index state
pinned, Cabal does not need the network. This was verified by building a
project into an empty store with all HTTP(S) proxies pointed at a dead port.

`hx fetch` downloads from `https://hackage.haskell.org` by default. Set
`HX_HACKAGE_URL` to use a mirror with Hackage's
`package/<name>-<version>/<name>-<version>.tar.gz` layout, including a local
`file://` directory.

## Hash mismatches

A tarball that does not match its `sha256` is a hard error in `hx fetch`,
`hx sync` and `hx lock --check`, and is never built. A mismatching download
is discarded; a mismatching cached file is left in place for inspection:

```
error: hash mismatch for pkg-1.0
  expected: 770e6076...376c (hx.lock)
  found   : b5c1fb2e...8c73 (download, discarded)
```

Hackage never changes a published tarball, so a mismatch means a corrupted
download, a tampered mirror or cache, or an edited lockfile. Metadata-only
Hackage revisions do not change the tarball; they are pinned separately via
`revision`/`cabal_sha256`.

## Builds use the lockfile

`hx build`, `hx test`, `hx run`, `hx check` and `hx repl` pass cabal the same
pins as `hx sync` (`--index-state` and a `--constraint` per locked package and
flag) whenever `hx.lock` exists and still describes the project. They compare
it with the project first, using only local files:

- every dependency in the `.cabal` files is locked, at a version inside its
  declared range;
- workspace members, the Stackage snapshot, a GHC pin in `hx.toml` and the BHC
  profile are unchanged;
- the active GHC is the one the plan was resolved for (`compiler_id`).

If any of that fails, the build prints what changed, suggests `hx lock`, and
lets cabal plan on its own, so adding a dependency never blocks a build.
Without an `hx.lock` cabal plans as usual.

cabal cannot build at an index state newer than its own copy of the Hackage
index, and `hx lock` refreshes hx's index rather than cabal's. When cabal's
index is older than `index_state`, the build runs `cabal update` first. If that
does not help (for example without a network), the build keeps the locked
versions and drops the index state, with a warning; `hx sync` and `--locked`
stop instead.

With `--locked` (or `HX_LOCKED=1`), a missing, unreadable, old-format or
out-of-date `hx.lock` stops the build with exit code `3`, and every cached
source tarball must match its `sha256` (as in `hx sync`), so a mismatch stops it
with exit code `1`. Use it in CI next to `hx lock --check`:

```yaml
- run: hx lock --check
- run: hx test --locked
```

The pins apply to cabal builds. Native builds (`--native`) already read
`hx.lock` for their dependencies, and the BHC backend resolves against its
BHC Platform snapshot.

## `hx lock --check` in CI

```yaml
- run: hx lock --check
```

It exits `3` and lists each problem when:

- `hx.lock` is missing, unreadable, or uses an older format;
- a dependency in a `.cabal` file is not locked, or its locked version no
  longer satisfies the declared range;
- workspace members, the Stackage snapshot, the pinned GHC (`[toolchain].ghc`
  vs `compiler_id`) or the BHC profile changed since locking;
- a Hackage package has no `sha256`, or a `git` package is not pinned to a
  full commit;
- the Hackage index (Cabal's if present, else hx's), read as of
  `index_state`, records a different tarball hash or `.cabal` revision hash
  than the lockfile;
- a tarball in hx's or Cabal's cache does not match its `sha256`.

If the index is older than the lockfile and cannot confirm a pin, that is
reported as a warning rather than a failure.
