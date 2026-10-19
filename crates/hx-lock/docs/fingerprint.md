# Fingerprinting

Lockfile fingerprinting enables cache invalidation and build skipping.

## Overview

A fingerprint is a hash that changes when the build plan changes:

```rust
let lockfile = Lockfile::from_file("hx.lock")?;
let fingerprint = lockfile.fingerprint();
// "sha256:a1b2c3d4e5f6..."
```

## What's Included

The fingerprint includes:

1. **Toolchain versions** - GHC and Cabal versions
2. **Platform** - OS and architecture
3. **Index state** - Hackage snapshot timestamp
4. **All packages** - Name and version, plus (format v2) `sha256`, `revision`, `cabal_sha256`, flags and git/path origin when present

## Fingerprint Algorithm

```rust
impl Lockfile {
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();

        // Toolchain
        if let Some(ghc) = &self.toolchain.ghc {
            hasher.update(format!("ghc:{}", ghc));
        }
        if let Some(cabal) = &self.toolchain.cabal {
            hasher.update(format!("cabal:{}", cabal));
        }

        // Platform
        if let Some(platform) = &self.plan.platform {
            hasher.update(format!("platform:{}", platform));
        }

        // Index state
        if let Some(index) = &self.plan.index_state {
            hasher.update(format!("index:{}", index));
        }

        // Packages (sorted for determinism)
        let mut packages: Vec<_> = self.packages.iter().collect();
        packages.sort_by_key(|p| &p.name);

        for pkg in packages {
            hasher.update(format!(
                "{}:{}:{}",
                pkg.name, pkg.version, pkg.source
            ));
            if let Some(hash) = &pkg.hash {
                hasher.update(hash);
            }
        }

        format!("sha256:{:x}", hasher.finalize())
    }
}
```

## Cache Invalidation

The fingerprint determines when cached builds are valid:

```rust
use hx_cache::StoreIndex;

let lockfile = Lockfile::from_file("hx.lock")?;
let fingerprint = lockfile.fingerprint();

let store = StoreIndex::load()?;
if store.has_cached_build(&fingerprint) {
    println!("Build is cached, skipping");
} else {
    println!("Running fresh build");
}
```

## Fingerprint Changes

A new fingerprint is generated when:

| Change | Effect |
|--------|--------|
| GHC version changes | New fingerprint |
| Package version changes | New fingerprint |
| Package added/removed | New fingerprint |
| Platform changes | New fingerprint |
| Index state changes | New fingerprint |

A fingerprint does NOT change when:
- hx.lock timestamp changes
- File is reformatted
- Comments change (N/A for TOML)

## Usage in Builds

```rust
// Check if build can be skipped
let lockfile = Lockfile::from_file("hx.lock")?;
let fingerprint = lockfile.fingerprint();

let build_state = BuildState::load(&project.root)?;
if build_state.matches_fingerprint(&fingerprint) {
    output.status("Fresh", project.name());
    return Ok(0);
}

// Run build...
build_state.update_fingerprint(&fingerprint);
build_state.save(&project.root)?;
```

## Plan Hash vs Fingerprint

| Concept | Source | Use |
|---------|--------|-----|
| `plan.hash` | Stored in lockfile | Identifies this specific resolution |
| `fingerprint()` | Computed on demand | Cache key for builds |

The `plan.hash` is typically set by Cabal during resolution. The fingerprint is computed by hx from the complete lockfile contents.

## Displaying Fingerprint

```bash
hx lock --fingerprint
# sha256:a1b2c3d4e5f6789...
```

Useful for debugging cache issues:

```bash
# Compare fingerprints
hx lock --fingerprint > fp1.txt
# Make changes...
hx lock --fingerprint > fp2.txt
diff fp1.txt fp2.txt
```
