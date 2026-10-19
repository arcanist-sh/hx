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

The user-facing table of every location (with all three platforms and the
override variables) is the "Where hx Stores Things" section of the top-level
`README.md`; `hx config path --all` prints the effective values.

All locations are computed in one place, `hx_cache::paths`. Other crates must
call these functions instead of building `~/.cache/hx/...` themselves, so the
overrides apply everywhere:

| Function | Default (Linux) |
|----------|-----------------|
| `global_cache_dir()` | `~/.cache/hx` |
| `cabal_store_dir()` | `<cache>/cabal/store` |
| `solver_cache_dir()` | `~/.cache/hx/solver` |
| `package_store_dir()` | `~/.cache/hx/packages` |
| `index_mirror_dir()` | `~/.cache/hx/index` |
| `stackage_cache_dir()` | `~/.cache/hx/stackage` |
| `bhc_platform_cache_dir()` | `~/.cache/hx/bhc-platform` |
| `script_cache_dir()` | `~/.cache/hx/scripts` |
| `global_config_dir()` / `global_config_file()` | `~/.config/hx` / `.../config.toml` |
| `global_plugin_dirs()` | `~/.config/hx/plugins` (+ legacy dir on macOS/Windows) |
| `hx_home_dir()`, `toolchain_dir()`, `toolchain_bin_dir()` | `~/.hx`, `~/.hx/toolchains`, `~/.hx/bin` |
| `bhc_home_dir()` | `~/.bhc` |

On macOS and Windows, `solver`/`packages`/`index` default to `~/.cache/hx/...`
and `stackage`/`bhc-platform`/`scripts` to `<platform cache>/hx/...`, the
locations earlier releases used (`LegacyRoot`). Once `HX_CACHE_DIR` or
`XDG_CACHE_HOME` is set, all of them live under the one cache root.

For tests, `HxPaths::resolve(env, &base)` takes an environment lookup closure
and hand-built `BaseLocations`, so layouts for every platform can be checked
without touching the process environment.

## Why `~/.hx` for Toolchains?

Toolchains use `~/.hx/toolchains` instead of platform-specific directories because:

1. **No spaces in path** - GHC's Makefile doesn't handle paths with spaces well
2. **Consistent across platforms** - Same relative path everywhere
3. **Easy to find** - Predictable location for debugging

## Ensuring Directories Exist

```rust
use hx_cache::ensure_dir;

// Create directory if it doesn't exist
ensure_dir(&global_cache_dir()?)?;
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
| `HX_HOME` | Override `~/.hx` location (toolchains, tool symlinks) |
| `HX_CACHE_DIR` | Override the cache root (used as-is) |
| `XDG_CACHE_HOME` | Cache root becomes `$XDG_CACHE_HOME/hx` |
| `HX_STORE_DIR` | Override Cabal store |
| `XDG_CONFIG_HOME` | Config dir becomes `$XDG_CONFIG_HOME/hx` |
| `HX_CONFIG_FILE` | Override the config file |
