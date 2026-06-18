# Stackage Snapshots

Pin a whole dependency universe with one line. Point `hx.toml` at a [Stackage](https://www.stackage.org/) snapshot and every build resolves against a curated, known-compatible set of package versions — reproducible today and a year from now.

> **Using BHC?** See [BHC_PLATFORM.md](./BHC_PLATFORM.md) for BHC Platform curated snapshots, the BHC equivalent of Stackage.

## Configuration

Add a snapshot to your `hx.toml`:

```toml
[stackage]
snapshot = "lts-22.28"
```

### Options

| Field | Description |
|-------|-------------|
| `snapshot` | Stackage snapshot identifier (e.g., `lts-22.28`, `nightly-2024-01-15`) |
| `allow_newer` | Allow packages not in the snapshot from Hackage (default: false) |
| `extra_deps` | Override specific package versions |

### Example with extra deps

```toml
[stackage]
snapshot = "lts-22.28"
allow_newer = true
extra_deps = { aeson = "2.2.0.0", text = "2.1" }
```

## CLI Commands

### List available snapshots

```bash
hx stackage list              # Show recent LTS and Nightly snapshots
hx stackage list --lts        # Show only LTS snapshots
hx stackage list --nightly    # Show only Nightly snapshots
hx stackage list --limit 20   # Show more snapshots
```

### Get snapshot information

```bash
hx stackage info lts-22.28           # Show snapshot details
hx stackage info lts-22.28 --packages  # Include all packages
hx stackage info nightly             # Latest nightly info
```

### Set snapshot for project

```bash
hx stackage set lts-22.28    # Set snapshot in hx.toml
hx stackage set nightly      # Use latest nightly
```

### Lock with snapshot (shorthand)

```bash
hx lock --snapshot lts-22.28  # Set snapshot and lock in one step
```

## Snapshot Identifiers

| Format | Example | Description |
|--------|---------|-------------|
| `lts-X.Y` | `lts-22.28` | Specific LTS version |
| `lts-X` | `lts-22` | Latest minor in LTS major version |
| `lts` | `lts` | Latest LTS |
| `nightly-YYYY-MM-DD` | `nightly-2024-01-15` | Specific nightly |
| `nightly` | `nightly` | Latest nightly |

## GHC Version Compatibility

Each Stackage snapshot is built against a specific GHC version. hx will warn you if your project's GHC version doesn't match the snapshot:

```
warning: Project uses GHC 9.6.4 but snapshot lts-22.28 uses GHC 9.6.3
         Consider updating [toolchain].ghc to "9.6.3" in hx.toml
```

## How It Works

1. When you run `hx lock` with a snapshot configured, hx fetches the snapshot metadata from Stackage
2. Package versions from the snapshot are pinned in the resolver
3. The lockfile records which snapshot was used
4. Packages are marked with `source = "stackage"` in the lockfile

## Caching

Snapshots are cached locally in `~/.cache/hx/snapshots/` (or equivalent on your platform). This allows offline resolution after the initial fetch.

## Troubleshooting

### Snapshot not found

```bash
hx stackage info lts-99.99
# error: Failed to load snapshot: snapshot not found
```

Solution: Check available snapshots with `hx stackage list`

### Package not in snapshot

If a dependency isn't in the snapshot:

1. Set `allow_newer = true` to allow Hackage packages
2. Or specify the package version in `extra_deps`

```toml
[stackage]
snapshot = "lts-22.28"
allow_newer = true
extra_deps = { some-new-package = "1.0.0" }
```

### GHC version mismatch

If you get build errors due to GHC incompatibility:

1. Update your `[toolchain].ghc` to match the snapshot
2. Or choose a snapshot that matches your GHC version

```bash
hx stackage info lts-22  # Check which GHC version this uses
```
