<div align="center">

<h1 align="center">
  <img src=".github/static/hx-logo.svg" alt="hx" height="80" />
</h1>

<p align="center">
  <em>A fast, opinionated, batteries-included toolchain for Haskell.</em><br>
  <em>Haskell, finally fast.</em>
</p>

<p align="center">
  <a href="https://www.rust-lang.org/">
    <img alt="Rust" src="https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white&style=for-the-badge">
  </a>
  <a href="https://www.haskell.org/">
    <img alt="Haskell" src="https://img.shields.io/badge/For-Haskell-5e5086?logo=haskell&logoColor=white&style=for-the-badge">
  </a>
  <a href="LICENSE">
    <img alt="License" src="https://img.shields.io/badge/License-MIT-c6a0f6?style=for-the-badge">
  </a>
</p>

<p align="center">
  <a href="https://hx.raskell.io/docs/">Documentation</a> •
  <a href="https://hx.raskell.io/features/">Features</a> •
  <a href="https://hx.raskell.io/benchmarks/">Benchmarks</a> •
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

</div>

---

hx is a modern toolchain CLI for Haskell, written in Rust. It wraps existing tools (GHC, Cabal, GHCup, HLS) in a fast, unified interface with excellent error messages and deterministic builds.

## Features

- **Fast feedback loop** - Common workflows are snappy with parallel builds
- **Deterministic builds** - Lockfile + frozen plans for reproducibility
- **Native builds** - Direct GHC invocation without cabal for simple projects
- **Excellent errors** - Actionable messages with fix suggestions
- **Watch mode** - Auto-rebuild on file changes
- **Test coverage** - Integrated hpc support with HTML reports
- **Plugin system** - Extensible with Steel (Scheme) scripts
- **Self-contained** - Manages GHC versions directly, no ghcup required
- **BHC backend** - Optional [Basel Haskell Compiler](docs/BHC_PLATFORM.md) backend with curated BHC Platform snapshots

<div align="center">
  <img src=".github/static/demo.gif" alt="hx demo" width="600" />
</div>

## Installation

### From releases

```bash
# macOS/Linux
curl -fsSL https://raw.githubusercontent.com/raskell-io/hx/main/install.sh | sh

# Or download from releases
https://github.com/raskell-io/hx/releases
```

### Windows

```
winget install raskell-io.hx
```

### From source

```bash
cargo install --git https://github.com/raskell-io/hx hx-cli
```

### Verify installation

```bash
hx --version
hx doctor
```

## Quick Start

```bash
# Create a new project
hx init myapp
cd myapp

# Build and run
hx build
hx run

# Lock dependencies for reproducibility
hx lock
hx sync

# Watch for changes
hx watch
```

## Commands

### Project Management

```bash
hx init                      # Initialize in current directory
hx init myapp                # Create myapp/ and initialize
hx init --lib                # Create a library project
hx init --ci                 # Include GitHub Actions workflow

hx new webapp myapp          # Create web app (Servant)
hx new cli myapp             # Create CLI app (optparse-applicative)
hx new library mylib         # Create library with docs setup
hx new template <url> <name> # Create from git template
```

### Building

```bash
hx build                     # Build the project
hx build --release           # Build with optimizations
hx build --native            # Use native GHC build (no cabal)
hx build -j4                 # Parallel build with 4 jobs
hx check                     # Fast type-check
hx clean                     # Clean build artifacts
```

### Running & Testing

```bash
hx run                       # Build and run the executable
hx run -- arg1 arg2          # Pass arguments to the program
hx repl                      # Start an interactive GHCi session
hx test                      # Run tests
hx test --pattern "Unit"     # Filter tests by pattern
hx bench                     # Run benchmarks
```

### Watch Mode

```bash
hx watch                     # Auto-rebuild on file changes
hx watch --test              # Auto-run tests on changes
hx watch --clear             # Clear terminal between runs
```

### Dependencies

```bash
hx lock                      # Generate/update hx.lock
hx lock --update             # Update all dependencies
hx sync                      # Build with locked dependencies
hx fetch                     # Pre-fetch dependencies in parallel

hx add text                  # Add a dependency
hx add aeson ">=2.0"         # Add with version constraint
hx add --dev hspec           # Add dev dependency
hx rm text                   # Remove a dependency

hx why text                  # Show why a package is a dependency
hx info aeson                # Show package details from Hackage
hx info aeson --versions     # Include all available versions
hx outdated                  # Check for outdated dependencies
hx outdated --direct         # Only show direct dependencies
hx update                    # Update dependencies (minor/patch)
hx update --major            # Allow major version updates
hx update --dry-run          # Preview updates without applying
hx update aeson text         # Update specific packages

hx tree                      # Show dependency tree
hx tree --depth 2            # Limit tree depth
hx list                      # List all dependencies
hx list --direct             # List direct dependencies only

hx deps graph --format dot   # Generate Graphviz graph (also: tree, list, json)

hx search aeson              # Search Hackage for packages
hx audit                     # Check for deprecated dependencies
hx audit --outdated          # Check for outdated packages
```

### Code Quality

```bash
hx fmt                       # Format code with fourmolu
hx fmt --check               # Check formatting without changes
hx lint                      # Run hlint
hx lint --fix                # Apply automatic fixes
```

### Test Coverage

```bash
hx coverage                  # Run tests with coverage
hx coverage --html           # Generate HTML report
hx coverage --html --open    # Generate and open in browser
hx coverage --threshold 80   # Fail if below 80% coverage
hx coverage --json           # Output JSON for CI
```

### Profiling

```bash
hx profile                   # Run with time profiling
hx profile --heap            # Run with heap profiling
hx profile --time --heap     # Both time and heap
```

### Documentation

```bash
hx docs                      # Generate documentation
hx docs --open               # Generate and open in browser
hx docs --deps               # Include dependency docs
hx docs --serve              # Serve locally on port 8080
```

### Toolchain Management

```bash
hx toolchain status          # Show installed versions
hx toolchain list            # List all available versions
hx toolchain list --installed # List installed versions only

hx toolchain install 9.8.2   # Install GHC version
hx toolchain install --set   # Install and set as active
hx toolchain remove 9.6.4    # Remove a GHC version

hx toolchain use 9.8.2       # Switch GHC version
hx toolchain use project     # Use project's toolchain

hx stackage list             # List Stackage snapshots
hx bhc-platform list         # List BHC Platform snapshots
hx server start              # Start the persistent build server
hx server status             # Show build server status
```

### IDE Integration

```bash
hx ide setup                 # Generate hie.yaml for HLS
hx ide status                # Check IDE configuration
hx lsp                       # Start language server
```

### Publishing

```bash
hx publish                   # Publish to Hackage
hx publish --dry-run         # Validate without uploading
hx publish --docs            # Include documentation
hx changelog                 # Generate CHANGELOG from commits
hx changelog --preview       # Preview without writing
```

### Distribution

```bash
hx dist                      # Build release archive
hx dist --target x86_64-unknown-linux-musl  # Cross-compile
hx dist formula              # Generate Homebrew formula
hx dist install-script       # Generate install script
```

### Utilities

```bash
hx doctor                    # Diagnose setup issues
hx script file.hs            # Run single-file script
hx import --from stack       # Import from stack.yaml
hx nix flake                 # Generate flake.nix
hx nix shell                 # Generate shell.nix
hx completions install       # Auto-install completions for your shell
hx completions generate bash # Generate completions to stdout
hx upgrade                   # Upgrade hx to latest version
```

### Cache Management

```bash
hx cache status              # Show cache statistics
hx cache artifacts           # Show artifact cache statistics
hx cache prune --days 30     # Remove entries older than 30 days
hx cache clean               # Clear entire cache
hx index update              # Update Hackage package index
hx index clear               # Remove the local index
hx index status              # Show index status
```

### Plugins

```bash
hx plugins list              # List available plugins
hx plugins status            # Show plugin system status
hx plugins run script.scm    # Run a Steel script
hx plugins trust             # Allow this project's local plugins to run
hx plugins untrust           # Revoke that permission
```

Project-local plugins (`.hx/plugins/*.scm`) only run after you trust the
project with `hx plugins trust` - cloning a repository never grants its
scripts execution rights.

## Configuration

hx uses `hx.toml` for project configuration:

```toml
[project]
name = "myapp"
kind = "bin"         # or "lib"
resolver = "cabal"

[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"
hls = "2.9.0.0"

[build]
optimization = 2     # 0, 1, or 2
warnings = true
native = false       # Use native GHC builds

[format]
formatter = "fourmolu"

[lint]
hlint = true

[plugins]
enabled = true
hook_timeout_ms = 30000

[plugins.hooks]
pre_build = ["./scripts/check.scm"]
post_test = ["./scripts/notify.scm"]
```

## Global Configuration

hx supports global configuration at `~/.config/hx/config.toml` (Linux), `~/Library/Application Support/hx/config.toml` (macOS), or `%APPDATA%\hx\config\config.toml` (Windows).

Global settings provide defaults that can be overridden by project-local `hx.toml`:

```toml
# ~/.config/hx/config.toml

[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"

[build]
optimization = 1
warnings = true

[format]
formatter = "fourmolu"

[lint]
hlint = true
```

### Managing Global Config

```bash
hx config show           # Show current global configuration
hx config path           # Show path to global config file
hx config edit           # Open config file in your $EDITOR
hx config init           # Create default config file
hx config set <key> <val> # Set a configuration value
hx config get <key>      # Get a configuration value

# Examples:
hx config set toolchain.ghc 9.8.2
hx config set build.optimization 2
hx config set format.formatter ormolu
```

## Lockfile

The `hx.lock` file ensures reproducible builds:

```toml
version = 1
created_at = "2026-01-16T00:00:00Z"

[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"

[plan]
compiler_id = "ghc-9.8.2"
platform = "x86_64-linux"
hash = "sha256:..."

[[packages]]
name = "text"
version = "2.1.1"
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `HX_VERBOSE` | Enable verbose output |
| `HX_QUIET` | Suppress output |
| `HX_NO_COLOR` | Disable colored output |
| `HX_CONFIG_FILE` | Path to config file |
| `HX_CACHE_DIR` | Cache directory location |
| `HX_AUTO_INSTALL` | Auto-install missing toolchains |
| `HX_NO_AUTO_INSTALL` | Never auto-install toolchains |
| `HX_ALLOW_UNVERIFIED_DOWNLOADS` | Allow toolchain installs without a published checksum |
| `HX_BHC_PLATFORM_PUBKEY` | Pinned Ed25519 key for BHC Platform snapshot verification |

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Usage error |
| 3 | Configuration error |
| 4 | Toolchain error |
| 5 | Build/test failure |
| 6 | Plugin hook failure |

## Architecture

hx is a Rust workspace with these crates:

| Crate | Purpose |
|-------|---------|
| `hx-cli` | Command-line interface |
| `hx-core` | Shared types and utilities |
| `hx-config` | Configuration parsing |
| `hx-lock` | Lockfile management |
| `hx-toolchain` | GHC/Cabal detection and installation |
| `hx-cabal` | Cabal wrapper and native builds |
| `hx-cache` | Build cache management |
| `hx-doctor` | Diagnostic checks |
| `hx-solver` | Native dependency resolver |
| `hx-lsp` | Language server protocol |
| `hx-plugins` | Steel plugin runtime |
| `hx-ui` | Terminal output utilities |
| `hx-warnings` | Warning system |
| `hx-telemetry` | Tracing and metrics |
| `hx-compiler` | Compiler backend abstraction (GHC, BHC) |
| `hx-bhc` | BHC (Basel Haskell Compiler) native build pipeline |

## Development

```bash
# Build
cargo build

# Run tests
cargo test --workspace

# Run clippy
cargo clippy --workspace -- -D warnings

# Format
cargo fmt

# Run hx locally
cargo run -p hx-cli -- --help
```

## Philosophy

hx follows the [Astral](https://astral.sh/) approach:

1. **Wrap first** - Use existing tools (Cabal, GHC) rather than reimplementing
2. **Tame second** - Add better UX, error messages, and workflows
3. **Replace last** - Only replace components when truly necessary (native builds)

## License

MIT
