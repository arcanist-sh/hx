# hx-toolchain

Toolchain detection, management, and direct installation for hx.

## Overview

`hx-toolchain` provides:

- **Detection** - Find installed GHC, Cabal, GHCup, and HLS
- **Direct download** - Install GHC and Cabal without ghcup
- **Version management** - Switch between installed versions
- **Auto-install** - Seamless project-specific toolchains
- **Platform support** - Linux, macOS, Windows
- **Release metadata** - Versions, tags, bindist URLs and checksums from ghcup's published metadata, with built-in tables offline

## Modules

| Module | Description |
|--------|-------------|
| `detect` | Tool detection and version parsing |
| `ghc` | GHC download, installation, and management |
| `cabal` | Cabal download and installation |
| `check` | Toolchain requirement validation |
| `install` | Smart installation (ghcup or direct) |
| `metadata` | ghcup release metadata: fetch, cache, bindist selection |

## Quick Start

```rust
use hx_toolchain::{Toolchain, ensure_toolchain, AutoInstallPolicy};

// Detect installed tools
let toolchain = Toolchain::detect().await;

if toolchain.ghc.status.is_found() {
    println!("GHC: {}", toolchain.ghc.status.version().unwrap());
}

// Ensure project requirements are met
ensure_toolchain(
    &toolchain,
    Some("9.8.2"),     // Required GHC
    Some("3.12.1.0"),  // Required Cabal
    AutoInstallPolicy::Always,
).await?;
```

## Key Feature: Zero-Dependency Installation

hx can install GHC and Cabal directly, without requiring ghcup:

```rust
use hx_toolchain::ghc::{download_and_install_ghc, DownloadOptions};

// Install GHC 9.8.2
let result = download_and_install_ghc(
    "9.8.2",
    DownloadOptions::default(),
).await?;

println!("Installed to: {}", result.path.display());
```

## Documentation

- [Detection](./detection.md) - Finding installed tools
- [Installation](./installation.md) - Installing GHC and Cabal
- [Version Management](./versions.md) - Managing multiple versions
- [Auto-Install](./auto-install.md) - Project-specific toolchains
- [Platform Support](./platforms.md) - Cross-platform details
- [Release Metadata](./metadata.md) - Versions, URLs and checksums from ghcup
