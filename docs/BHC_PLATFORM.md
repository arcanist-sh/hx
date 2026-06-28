# BHC Platform Snapshots

The futuristic half of hx. Point your project at the [BHC (Basel Haskell Compiler)](https://bhc.dev) backend and a BHC Platform snapshot — Stackage-like package sets, curated and verified to build together under BHC — and compile Haskell on a next-generation toolchain without leaving the workflow you already know.

> **Using GHC?** See [STACKAGE.md](./STACKAGE.md) for Stackage snapshot support.
>
> **Building from source?** See [BHC_NATIVE_BUILD.md](./BHC_NATIVE_BUILD.md) for
> how hx compiles dependencies into BHC package databases.

## Configuration

Add a BHC Platform snapshot to your `hx.toml`:

```toml
[compiler]
backend = "bhc"

[bhc-platform]
snapshot = "bhc-platform-2026.1"
```

### Options

| Field | Description |
|-------|-------------|
| `snapshot` | BHC Platform snapshot identifier (e.g., `bhc-platform-2026.1`) |
| `allow_newer` | Allow packages not in the snapshot from Hackage (default: false) |
| `extra_deps` | Override specific package versions |

### Example with extra deps

```toml
[bhc-platform]
snapshot = "bhc-platform-2026.1"
allow_newer = true
extra_deps = { my-custom-lib = "1.0.0" }
```

## CLI Commands

### List available snapshots

```bash
hx bhc-platform list
```

Output:

```
  Available  BHC Platform snapshots

  bhc-platform-2026.1 (BHC 0.1.0, GHC-compat 9.8.2, 70 packages)

Set a platform with: hx bhc-platform set <platform>
```

### Get snapshot information

```bash
hx bhc-platform info bhc-platform-2026.1              # Show snapshot details
hx bhc-platform info bhc-platform-2026.1 --packages    # Include all packages
```

### Set snapshot for project

```bash
hx bhc-platform set bhc-platform-2026.1    # Set snapshot in hx.toml
```

This updates the `[bhc-platform].snapshot` field in your `hx.toml`.

## Snapshot Identifiers

| Format | Example | Description |
|--------|---------|-------------|
| `bhc-platform-YYYY.N` | `bhc-platform-2026.1` | Specific BHC Platform version |

## Available Snapshots

### bhc-platform-2026.1

The initial BHC Platform snapshot, curated for BHC 0.1.0 with GHC 9.8.2 compatibility.

**~70 packages** covering:

| Category | Packages |
|----------|----------|
| **Core** | base, ghc-prim, integer-gmp, template-haskell |
| **Text & ByteString** | text, bytestring, utf8-string |
| **Containers** | containers, unordered-containers, hashable, vector |
| **Transformers** | mtl, transformers, exceptions, unliftio |
| **Serialization** | aeson, yaml, binary, cereal |
| **Optics** | lens, microlens, generic-lens |
| **Parsing** | megaparsec, attoparsec, optparse-applicative |
| **HTTP & Web** | http-client, http-types, servant, servant-server, warp, wai, wai-extra |
| **Database** | persistent, esqueleto, resource-pool |
| **Numeric** | hmatrix, statistics, massiv, random |
| **Testing** | hspec, QuickCheck, tasty, hedgehog, hspec-wai |
| **Streaming** | conduit, streaming, pipes |
| **CLI & Logging** | optparse-applicative, monad-logger, fast-logger, co-log |
| **Utilities** | time, filepath, directory, process, stm, async |

## How It Works

1. BHC Platform snapshots are embedded directly in hx (no network fetch required)
2. When you run `hx lock` with a BHC Platform configured and `backend = "bhc"`, package versions from the snapshot are pinned in the resolver
3. The lockfile records which snapshot was used
4. This works alongside the existing Stackage integration — BHC Platform is used when the compiler backend is BHC

### Lock Integration

When both `[compiler].backend = "bhc"` and `[bhc-platform].snapshot` are set, running `hx lock` will:

1. Load the BHC Platform snapshot
2. Pin all snapshot package versions into the resolver
3. Resolve any additional dependencies from Hackage (if `allow_newer = true`)
4. Record the snapshot in `hx.lock`

## BHC Profiles

BHC Platform snapshots include a recommended profile. BHC profiles optimize the compiler for specific workloads:

| Profile | Use Case |
|---------|----------|
| `default` | General-purpose Haskell development |
| `server` | Web servers and network services |
| `numeric` | Scientific computing, linear algebra, statistics |
| `edge` | IoT, embedded, resource-constrained environments |

Set the profile in your `hx.toml`:

```toml
[compiler]
backend = "bhc"

[compiler.bhc]
profile = "numeric"
```

## Project Templates

hx includes BHC-optimized project templates that come pre-configured with a BHC Platform snapshot:

```bash
hx new numeric my-science    # Numeric computing project (hmatrix, vector, massiv)
hx new server my-api         # Web server project (Servant, Warp, WAI)
```

You can also create any project with the BHC backend:

```bash
hx init --backend bhc
hx new webapp my-app --backend bhc
hx new cli my-tool --backend bhc
hx new library my-lib --backend bhc
```

## Troubleshooting

### Snapshot not found

```bash
hx bhc-platform info bhc-platform-9999.1
# error: Failed to load platform: unknown BHC Platform snapshot
```

Solution: Check available snapshots with `hx bhc-platform list`

### Package not in snapshot

If a dependency isn't in the snapshot:

1. Set `allow_newer = true` to allow Hackage packages
2. Or specify the package version in `extra_deps`

```toml
[bhc-platform]
snapshot = "bhc-platform-2026.1"
allow_newer = true
extra_deps = { some-package = "1.0.0" }
```

### BHC not installed

```
error: BHC is not installed
  Install BHC with: hx toolchain install --bhc latest
```

BHC Platform snapshots require the BHC compiler to be installed. Install it with `hx toolchain install --bhc latest`.

### Using BHC Platform with GHC backend

BHC Platform snapshots are only applied when `[compiler].backend = "bhc"`. If you're using the GHC backend, use [Stackage snapshots](./STACKAGE.md) instead.
