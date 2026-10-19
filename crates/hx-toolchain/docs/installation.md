# Installation

hx can install GHC and Cabal directly without requiring ghcup.

## Download URLs

The archive URL, unpack directory and SHA-256 come from ghcup's release
metadata when it lists the version for this machine; on Linux that also
picks the build for your distribution (see [Release Metadata](./metadata.md)).
Otherwise, and whenever the metadata cannot be fetched, hx builds the URL
itself:

### GHC
```
https://downloads.haskell.org/~ghc/{version}/ghc-{version}-{suffix}.tar.xz
```

`{suffix}` is the target triple on macOS and Windows and the
`{arch}-deb10-linux` build on Linux.

### Cabal
```
https://downloads.haskell.org/cabal/cabal-install-{version}/cabal-install-{version}-{platform}.tar.xz
```

## Installing GHC

```rust
use hx_toolchain::ghc::{download_and_install_ghc, DownloadOptions};

let options = DownloadOptions {
    force: false,        // Don't reinstall if exists
    set_active: true,    // Set as default version
    show_progress: true, // Show download progress
};

let result = download_and_install_ghc("9.8.2", options).await?;
println!("Installed to: {}", result.path.display());
```

### DownloadOptions

```rust
pub struct DownloadOptions {
    /// Reinstall even if version exists
    pub force: bool,
    /// Set as active/default version
    pub set_active: bool,
    /// Show download progress bar
    pub show_progress: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            force: false,
            set_active: true,
            show_progress: true,
        }
    }
}
```

### InstallResult

```rust
pub struct InstallResult {
    /// Installed version
    pub version: GhcVersion,
    /// Installation path
    pub path: PathBuf,
    /// How it was installed
    pub source: InstallSource,
}

pub enum InstallSource {
    Ghcup,
    DirectDownload,
}
```

## Installing Cabal

```rust
use hx_toolchain::cabal::{download_and_install_cabal, DownloadOptions};

let result = download_and_install_cabal(
    "3.12.1.0",
    DownloadOptions::default(),
).await?;
```

## Smart Installation

The `install_ghc_smart` and `install_cabal_smart` functions try multiple methods:

```rust
use hx_toolchain::{install_ghc_smart, install_cabal_smart};

// Tries: 1) Direct download, 2) ghcup (if available)
let ghc_result = install_ghc_smart("9.8.2", options).await?;
let cabal_result = install_cabal_smart("3.12.1.0", options).await?;
```

### Smart Install Algorithm

1. **Check if already installed** - Skip if version exists (unless `force`)
2. **Try direct download** - Download from downloads.haskell.org
3. **Fallback to ghcup** - If direct fails and ghcup is available
4. **Update manifest** - Record installation in manifest.json
5. **Create symlinks** - Link to `~/.hx/bin/`

## Installation Directory

Tools are installed to `~/.hx/toolchains/`:

```
~/.hx/
├── toolchains/
│   ├── ghc/
│   │   ├── 9.8.2/
│   │   │   └── bin/
│   │   │       ├── ghc
│   │   │       ├── ghc-pkg
│   │   │       └── ...
│   │   └── 9.6.4/
│   │       └── bin/
│   └── cabal/
│       └── 3.12.1.0/
│           └── bin/
│               └── cabal
├── bin/                    # Symlinks to active versions
│   ├── ghc -> ../toolchains/ghc/9.8.2/bin/ghc
│   └── cabal -> ../toolchains/cabal/3.12.1.0/bin/cabal
└── manifest.json           # Tracks all installations
```

## Installation Process

### GHC Installation Steps

1. **Download** - Fetch tarball from downloads.haskell.org
2. **Verify** - Check the SHA-256 from ghcup metadata, or from the published
   `SHA256SUMS`; installs without either are refused unless
   `HX_ALLOW_UNVERIFIED_DOWNLOADS=1`
3. **Extract** - Unpack to temporary directory
4. **Configure** - Run `./configure --prefix=~/.hx/toolchains/ghc/{version}`
5. **Install** - Run `make install`
6. **Update manifest** - Record in manifest.json
7. **Create symlinks** - If set_active is true

### Cabal Installation Steps

1. **Download** - Fetch tarball
2. **Verify** - Same checksum rules as GHC
3. **Extract** - Unpack (single binary)
4. **Install** - Copy to `~/.hx/toolchains/cabal/{version}/bin/`
5. **Update manifest** - Record in manifest.json
6. **Create symlinks** - If set_active is true

## Progress Display

With `show_progress: true`:

```
   Downloading GHC 9.8.2...
   [========================================] 100% (245 MB)
   Installing GHC 9.8.2...
   Configuring...
   Building...
   Installing...
    Installed GHC 9.8.2 to ~/.hx/toolchains/ghc/9.8.2
```

## Error Handling

```rust
use hx_toolchain::ghc::{download_and_install_ghc, GhcError};

match download_and_install_ghc("9.8.2", options).await {
    Ok(result) => println!("Installed: {}", result.path.display()),
    Err(GhcError::DownloadFailed { url, source }) => {
        eprintln!("Download failed: {}", url);
    }
    Err(GhcError::ConfigureFailed { stderr }) => {
        eprintln!("Configure failed: {}", stderr);
    }
    Err(GhcError::UnsupportedPlatform { platform }) => {
        eprintln!("No binary for: {}", platform);
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

## Supported Versions

Any version ghcup's metadata lists can be installed; `hx toolchain list
--available` shows them. Without metadata hx falls back to its built-in
lists (`KNOWN_GHC_VERSIONS`, `KNOWN_CABAL_VERSIONS`), and versions outside
both can still be installed when the built-in URL scheme matches.

Check if a version is in the built-in list:

```rust
use hx_toolchain::ghc::is_known_version;

if is_known_version("9.8.2") {
    println!("Version 9.8.2 is available for download");
}
```
