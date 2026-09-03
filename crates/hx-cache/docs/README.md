# hx-cache

Cache management, fingerprinting, and artifact storage for hx.

## Overview

`hx-cache` manages:

- **Global cache** - Shared across all projects
- **Project cache** - Per-project `.hx/` directory
- **Toolchain storage** - `~/.hx/toolchains/`
- **Build artifacts** - Compiled `.o` and `.hi` files
- **Source fingerprinting** - Change detection

## Directory Layout

```
~/.hx/                          # hx home directory
├── toolchains/                 # Installed GHC/Cabal versions
│   ├── ghc/
│   │   └── 9.8.2/
│   └── cabal/
│       └── 3.12.1.0/
├── bin/                        # Symlinks to active versions
│   ├── ghc
│   └── cabal
└── manifest.json               # Toolchain manifest

~/.cache/hx/                    # Global cache (Linux)
~/Library/Caches/io.raskell.hx/  # Global cache (macOS)
├── store/
│   ├── index.json              # Build cache index
│   └── {fingerprint}/          # Cached builds
│       └── artifacts/
├── packages/
│   └── index.json              # Package resolution cache
└── sources/
    └── {fingerprint}/          # Source snapshots

project/.hx/                    # Project-local cache
├── build/                      # Build artifacts
├── fingerprint                 # Source fingerprint
└── build-state.json            # Build state tracking
```

## Quick Start

```rust
use hx_cache::{
    global_cache_dir, toolchain_dir, cabal_store_dir,
    compute_source_fingerprint, BuildState, StoreIndex,
};

// Get cache directories
let cache = global_cache_dir()?;
let toolchains = toolchain_dir()?;
let store = cabal_store_dir()?;

// Compute source fingerprint
let fingerprint = compute_source_fingerprint(&project_root)?;
println!("Source hash: {}", fingerprint.hash);

// Check build cache
let index = StoreIndex::load()?;
if index.has_cached_build(&fingerprint.hash) {
    println!("Build is cached!");
}
```

## Documentation

- [Directories](./directories.md) - Cache directory layout
- [Fingerprinting](./fingerprinting.md) - Source change detection
- [Build State](./build-state.md) - Incremental build tracking
- [Artifacts](./artifacts.md) - Compiled file caching
