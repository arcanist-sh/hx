# Platform Support

hx-toolchain supports multiple platforms for direct GHC/Cabal installation.

## Supported Platforms

| Platform | Architecture | Status |
|----------|--------------|--------|
| Linux | x86_64 | ✅ Full support |
| Linux | aarch64 | ✅ Full support |
| macOS | x86_64 | ✅ Full support |
| macOS | aarch64 (Apple Silicon) | ✅ Full support |
| Windows | x86_64 | ✅ Full support |

## Platform Enum

```rust
pub enum Platform {
    LinuxX86_64,
    LinuxAarch64,
    MacOsX86_64,
    MacOsAarch64,
    WindowsX86_64,
}

impl Platform {
    /// Detect current platform
    pub fn detect() -> Option<Self>;

    /// Get platform string for download URLs
    pub fn download_string(&self) -> &'static str;
}
```

## Platform Detection

```rust
use hx_toolchain::Platform;

let platform = Platform::detect()
    .expect("Unsupported platform");

println!("Running on: {}", platform.download_string());
// Output: "x86_64-linux" or "aarch64-apple-darwin" etc.
```

## Download URL Patterns

### GHC

```
https://downloads.haskell.org/ghc/{version}/ghc-{version}-{platform}.tar.xz
```

| Platform | URL Pattern |
|----------|-------------|
| Linux x86_64 | `ghc-9.8.2-x86_64-linux-unknown.tar.xz` |
| Linux aarch64 | `ghc-9.8.2-aarch64-linux-unknown.tar.xz` |
| macOS x86_64 | `ghc-9.8.2-x86_64-apple-darwin.tar.xz` |
| macOS aarch64 | `ghc-9.8.2-aarch64-apple-darwin.tar.xz` |
| Windows | `ghc-9.8.2-x86_64-windows.tar.xz` |

### Cabal

```
https://downloads.haskell.org/cabal/cabal-install-{version}/cabal-install-{version}-{platform}.tar.xz
```

| Platform | URL Pattern |
|----------|-------------|
| Linux x86_64 | `cabal-install-3.12.1.0-x86_64-linux-ubuntu20_04.tar.xz` |
| Linux aarch64 | `cabal-install-3.12.1.0-aarch64-linux-ubuntu20_04.tar.xz` |
| macOS x86_64 | `cabal-install-3.12.1.0-x86_64-darwin.tar.xz` |
| macOS aarch64 | `cabal-install-3.12.1.0-aarch64-darwin.tar.xz` |
| Windows | `cabal-install-3.12.1.0-x86_64-windows.zip` |

## Platform-Specific Paths

### Linux

```
~/.hx/
├── toolchains/
│   ├── ghc/
│   │   └── 9.8.2/
│   └── cabal/
│       └── 3.12.1.0/
└── bin/
```

Cache: `~/.cache/hx/`

### macOS

```
~/.hx/
├── toolchains/
│   ├── ghc/
│   │   └── 9.8.2/
│   └── cabal/
│       └── 3.12.1.0/
└── bin/
```

Cache: `~/Library/Caches/io.raskell.hx/`

### Windows

```
%USERPROFILE%\.hx\
├── toolchains\
│   ├── ghc\
│   │   └── 9.8.2\
│   └── cabal\
│       └── 3.12.1.0\
└── bin\
```

Cache: `%LOCALAPPDATA%\raskell\hx\cache\`

## GHC Installation Differences

### Linux/macOS

1. Download `.tar.xz`
2. Extract
3. Run `./configure --prefix=...`
4. Run `make install`

### Windows

1. Download `.tar.xz`
2. Extract
3. Copy files directly (no configure/make)

## PATH Separator

```rust
#[cfg(windows)]
const PATH_SEPARATOR: char = ';';

#[cfg(not(windows))]
const PATH_SEPARATOR: char = ':';
```

## Symlinks vs Copies

### Linux/macOS

Symlinks are used for version switching:
```
~/.hx/bin/ghc -> ~/.hx/toolchains/ghc/9.8.2/bin/ghc
```

### Windows

Hard copies or junctions (symlinks require admin):
```
copy %HX_TOOLCHAINS%\ghc\9.8.2\bin\ghc.exe %HX_BIN%\ghc.exe
```

## Platform-Specific Code

```rust
#[cfg(target_os = "linux")]
fn install_ghc(version: &str) -> Result<PathBuf> {
    // Linux-specific installation
    configure_and_make(version)
}

#[cfg(target_os = "macos")]
fn install_ghc(version: &str) -> Result<PathBuf> {
    // macOS-specific installation
    configure_and_make(version)
}

#[cfg(target_os = "windows")]
fn install_ghc(version: &str) -> Result<PathBuf> {
    // Windows: no configure/make needed
    extract_and_copy(version)
}
```

## Space in Paths

GHC's build system doesn't handle spaces in paths well. hx uses `~/.hx/toolchains` instead of platform-specific directories like `~/Library/Application Support/` to avoid this issue.

## Native Dependencies

### Linux

GHC requires:
- `gmp` (libgmp-dev)
- `ncurses` (libncurses-dev)
- `libffi` (libffi-dev)

### macOS

GHC requires:
- Xcode Command Line Tools
- Optionally: Homebrew for `gmp`, `libffi`

### Windows

GHC requires:
- MSYS2 environment
- MinGW-w64 toolchain

## Troubleshooting

### Linux: Missing Libraries

```bash
# Ubuntu/Debian
sudo apt install libgmp-dev libncurses-dev libffi-dev

# Fedora
sudo dnf install gmp-devel ncurses-devel libffi-devel
```

### macOS: No Xcode Tools

```bash
xcode-select --install
```

### Windows: MSYS2 Not Found

Install MSYS2 from https://www.msys2.org/ and add to PATH.
