# Version Management

hx supports multiple installed toolchain versions.

## Listing Installed Versions

```rust
use hx_toolchain::ghc::list_installed;

let versions = list_installed()?;
for ghc in versions {
    println!("GHC {} at {}", ghc.version, ghc.path.display());
}
```

## Getting Active Version

```rust
use hx_toolchain::ghc::get_active;

if let Some(version) = get_active()? {
    println!("Active GHC: {}", version);
} else {
    println!("No GHC version is active");
}
```

## Setting Active Version

```rust
use hx_toolchain::ghc::set_active;

set_active("9.8.2")?;
// Creates symlink: ~/.hx/bin/ghc -> ~/.hx/toolchains/ghc/9.8.2/bin/ghc
```

## Removing Versions

```rust
use hx_toolchain::ghc::remove_ghc;

remove_ghc("9.6.4")?;
// Removes ~/.hx/toolchains/ghc/9.6.4/
```

## Toolchain Manifest

Installed versions are tracked in `~/.hx/toolchains/manifest.json`:

```json
{
  "ghc_installations": [
    {
      "version": "9.8.2",
      "path": "/Users/user/.hx/toolchains/ghc/9.8.2"
    },
    {
      "version": "9.6.4",
      "path": "/Users/user/.hx/toolchains/ghc/9.6.4"
    }
  ],
  "cabal_installations": [
    {
      "version": "3.12.1.0",
      "path": "/Users/user/.hx/toolchains/cabal/3.12.1.0"
    }
  ],
  "active_ghc": "9.8.2",
  "active_cabal": "3.12.1.0"
}
```

## ToolchainManifest

```rust
pub struct ToolchainManifest {
    /// All installed GHC versions
    pub ghc_installations: Vec<InstalledGhc>,
    /// All installed Cabal versions
    pub cabal_installations: Vec<InstalledCabal>,
    /// Currently active GHC version
    pub active_ghc: Option<String>,
    /// Currently active Cabal version
    pub active_cabal: Option<String>,
}

pub struct InstalledGhc {
    pub version: GhcVersion,
    pub path: PathBuf,
}

pub struct InstalledCabal {
    pub version: CabalVersion,
    pub path: PathBuf,
}
```

## Manifest Operations

```rust
use hx_toolchain::ghc::ToolchainManifest;

// Load manifest
let manifest = ToolchainManifest::load()?;

// Check installed versions
for ghc in &manifest.ghc_installations {
    println!("{}: {}", ghc.version, ghc.path.display());
}

// Add installation
let mut manifest = ToolchainManifest::load()?;
manifest.add_ghc(InstalledGhc {
    version: "9.8.2".parse()?,
    path: PathBuf::from("/path/to/ghc"),
});
manifest.save()?;
```

## Symlink Management

Active versions are exposed via symlinks in `~/.hx/bin/`:

```
~/.hx/bin/
├── ghc          -> ../toolchains/ghc/9.8.2/bin/ghc
├── ghc-9.8.2    -> ../toolchains/ghc/9.8.2/bin/ghc
├── ghc-pkg      -> ../toolchains/ghc/9.8.2/bin/ghc-pkg
├── ghci         -> ../toolchains/ghc/9.8.2/bin/ghci
├── cabal        -> ../toolchains/cabal/3.12.1.0/bin/cabal
└── cabal-3.12   -> ../toolchains/cabal/3.12.1.0/bin/cabal
```

### Creating Symlinks

```rust
use hx_toolchain::ghc::create_symlinks;

// Create symlinks for a specific version
create_symlinks("9.8.2")?;
```

### Symlink Strategy

1. **Versionless symlink** (`ghc`) - Points to active version
2. **Versioned symlink** (`ghc-9.8.2`) - Always available
3. **Related tools** - `ghc-pkg`, `ghci`, `runghc`, etc.

## CLI Commands

```bash
# Installed versions plus ghcup's recommended and latest releases
hx toolchain list

# Every current release (from ghcup metadata, or the built-in list offline)
hx toolchain list --available

# Only installed versions
hx toolchain list --installed

# Install a version
hx toolchain install --ghc 9.8.2

# Set active version
hx toolchain use --ghc 9.8.2

# Remove a version
hx toolchain remove --ghc 9.6.4

# Show current status
hx toolchain status
```

## Version Resolution

When a project specifies a GHC version:

```rust
use hx_toolchain::ghc::{resolve_ghc, ResolutionConfig};

let config = ResolutionConfig {
    prefer_ghcup: false,    // Prefer hx-managed
    allow_prerelease: false,
};

let resolved = resolve_ghc("9.8.2", &config)?;
println!("Using GHC at: {}", resolved.path.display());
```

### Resolution Order

1. Check hx-managed: `~/.hx/toolchains/ghc/{version}/`
2. Check ghcup-managed: `~/.ghcup/ghc/{version}/`
3. Check system PATH

## PATH Configuration

Add `~/.hx/bin` to your PATH:

```bash
# ~/.bashrc or ~/.zshrc
export PATH="$HOME/.hx/bin:$PATH"
```

This enables:
```bash
$ ghc --version
The Glorious Glasgow Haskell Compilation System, version 9.8.2
```
