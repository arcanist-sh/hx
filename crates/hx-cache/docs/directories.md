# Directories

`hx-cache` provides cross-platform cache directory management.

## Directory Functions

```rust
use hx_cache::{
    global_cache_dir,
    global_config_dir,
    toolchain_dir,
    toolchain_bin_dir,
    cabal_store_dir,
};

// Global cache (platform-specific)
let cache = global_cache_dir()?;

// Global config (platform-specific)
let config = global_config_dir()?;

// Toolchain installations (~/.hx/toolchains)
let toolchains = toolchain_dir()?;

// Toolchain symlinks (~/.hx/bin)
let bin = toolchain_bin_dir()?;

// Cabal package store (<global cache>/cabal/store)
let store = cabal_store_dir()?;
```

## Platform Paths

### Linux

| Function | Path |
|----------|------|
| `global_cache_dir()` | `~/.cache/hx` |
| `global_config_dir()` | `~/.config/hx` |
| `toolchain_dir()` | `~/.hx/toolchains` |
| `toolchain_bin_dir()` | `~/.hx/bin` |
| `cabal_store_dir()` | `~/.cache/hx/cabal/store` |

### macOS

| Function | Path |
|----------|------|
| `global_cache_dir()` | `~/Library/Caches/io.raskell.hx` |
| `global_config_dir()` | `~/Library/Application Support/io.raskell.hx` |
| `toolchain_dir()` | `~/.hx/toolchains` |
| `toolchain_bin_dir()` | `~/.hx/bin` |
| `cabal_store_dir()` | `~/Library/Caches/io.raskell.hx/cabal/store` |

### Windows

| Function | Path |
|----------|------|
| `global_cache_dir()` | `%LOCALAPPDATA%\raskell\hx\cache` |
| `global_config_dir()` | `%APPDATA%\raskell\hx\config` |
| `toolchain_dir()` | `%USERPROFILE%\.hx\toolchains` |
| `toolchain_bin_dir()` | `%USERPROFILE%\.hx\bin` |
| `cabal_store_dir()` | `%USERPROFILE%\.hx\store` |

## Why `~/.hx` for Toolchains?

Toolchains use `~/.hx/toolchains` instead of platform-specific directories because:

1. **No spaces in path** - GHC's Makefile doesn't handle paths with spaces well
2. **Consistent across platforms** - Same relative path everywhere
3. **Easy to find** - Predictable location for debugging

## Ensuring Directories Exist

```rust
use hx_cache::ensure_dir;

// Create directory if it doesn't exist
let cache = ensure_dir(&global_cache_dir()?)?;
```

## Cleaning Caches

```rust
use hx_cache::{clean_global_cache, clean_project_cache};

// Remove all global cache data
clean_global_cache()?;

// Remove project's .hx directory
clean_project_cache(&project_root)?;
```

## Project Cache Layout

Each project has a `.hx/` directory:

```
project/.hx/
├── build/              # Cabal build artifacts
│   ├── build/
│   ├── cache/
│   └── packagedb/
├── native-build/       # Native build artifacts
├── fingerprint         # Source fingerprint JSON
└── build-state.json    # Build tracking state
```

## Global Store Layout

```
~/.cache/hx/cabal/store/
├── ghc-9.8.2/                    # Per-GHC-version packages
│   ├── aeson-2.2.1.0-abc123/
│   │   ├── lib/
│   │   │   └── Aeson.hi
│   │   └── share/
│   ├── text-2.1-def456/
│   └── ...
└── ghc-9.6.4/
    └── ...
```

## Cache Index Files

### Store Index

```rust
use hx_cache::StoreIndex;

// Load index
let index = StoreIndex::load()?;

// Check for cached build
if index.has_cached_build("sha256:abc123") {
    // Use cached artifacts
}

// Save after updates
index.save()?;
```

### Package Cache Index

```rust
use hx_cache::PackageCacheIndex;

let index = PackageCacheIndex::load()?;
// Contains cached dependency resolutions
```

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `HX_HOME` | Override `~/.hx` location |
| `HX_CACHE_DIR` | Override cache directory |
| `HX_STORE_DIR` | Override Cabal store |
