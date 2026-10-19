<div align="center">

<h1 align="center">
  <img src=".github/static/hx-logo.svg" alt="hx" height="80" />
</h1>

<p align="center">
  <strong>Haskell, finally fast.</strong><br>
  <em>One opinionated, batteries-included toolchain for Haskell.</em><br>
  <em>Fast, unified, futuristic.</em>
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
  <a href="https://docs.arcanist.sh/hx/docs/">Documentation</a> •
  <a href="https://arcanist.sh/hx/features/">Features</a> •
  <a href="https://arcanist.sh/hx/benchmarks/">Benchmarks</a> •
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

</div>

---

Haskell's tooling is powerful — and scattered. Starting a project means juggling `ghcup`, `cabal`, `stack`, `fourmolu`, `hlint`, and `hpc`, each with its own flags, config, and failure modes.

**hx** collapses all of it into a single, fast binary. Written in Rust, it drives the tools you already trust behind one coherent interface — with deterministic builds, errors that tell you how to fix them, and a feedback loop quick enough to stay out of your way.

> **One command to install. One file to configure. One tool to learn.**

<div align="center">
  <img src=".github/static/demo.gif" alt="hx demo" width="600" />
</div>

## Why hx?

- **Batteries included** — Build, test, run, format, lint, profile, and publish. Watch mode, coverage reports, and IDE setup. Everything in one tool.
- **Managed toolchains** — Automatic GHC installation and per-project version pinning in `hx.toml`. No ghcup required.
- **Deterministic builds** — TOML lockfiles with fingerprint verification. Reproducible across machines and CI — correct by construction, not by luck.
- **Excellent errors** — Actionable messages with fix suggestions. `hx doctor` diagnoses your setup and tells you exactly what to do.
- **Drop-in compatible** — Works with existing `.cabal` files and Hackage. Import from Stack or Cabal — your workflow stays the same, just better.
- **Extensible** — Plugin system with Steel (Scheme), pre/post-build hooks, and Nix integration.
- **Agent-ready** — A built-in MCP server (`hx mcp`), an [AGENTS.md](AGENTS.md), and a Claude skill let AI agents drive the whole toolchain.
- **Blazingly fast** — Native builds bypass Cabal overhead: ~5.6× faster cold builds, ~4.8× faster no-op rebuilds, measured on hx 0.9.2 — see [docs/BENCHMARKS.md](docs/BENCHMARKS.md).
- **Futuristic** — A first-class [BHC](docs/BHC_PLATFORM.md) backend: a next-generation Haskell compiler with curated Platform snapshots.

## Installation

### From releases

```bash
# macOS/Linux
curl -fsSL https://arcanist.sh/hx/install.sh | sh

# Or download from releases
https://github.com/arcanist-sh/hx/releases
```

### Windows

```
winget install raskell-io.hx
```

### From source

```bash
cargo install --git https://github.com/arcanist-sh/hx hx-cli
```

### If you already use Helix

The Helix editor's binary is also called `hx`. When the installer finds an
`hx` on your `PATH` that isn't hx, it installs as **`hxs`** instead so both
stay reachable, and tells you it did. Every command below then works the
same, just spelled `hxs`.

To take the `hx` name anyway:

```bash
curl -fsSL https://arcanist.sh/hx/install.sh | HX_BINARY_NAME=hx sh
```

Or to choose the name yourself:

```bash
curl -fsSL https://arcanist.sh/hx/install.sh | sh -s -- --binary-name hxs
```

Installing from source or from a release archive gives you `hx` regardless;
rename the binary if it would collide. `hx doctor` reports either direction
of collision if one appears later.

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
hx build                     # Build the project (all packages in a workspace)
hx build --package mylib     # Build a single workspace member
hx build --release           # Build with optimizations
hx build --native            # Use native GHC build (no cabal)
hx build -j4                 # Parallel build with 4 jobs
hx check                     # Fast type-check
hx clean                     # Clean build artifacts
```

> **Workspaces** — For a `cabal.project` with several local packages, `hx build`
> and `hx test` build and test every member. Use `--package <name>` to target a
> single member.

### Running & Testing

```bash
hx run                       # Build and run the executable
hx run -- arg1 arg2          # Pass arguments to the program
hx repl                      # Start an interactive GHCi session
hx test                      # Run tests (all packages in a workspace)
hx test --package mylib      # Test a single workspace member
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
hx toolchain install --bhc <version> # Install the BHC toolchain
hx toolchain remove 9.6.4    # Remove a GHC version

hx toolchain use 9.8.2       # Switch GHC version
hx toolchain use project     # Use project's toolchain

hx stackage list             # List Stackage snapshots
hx bhc-platform list         # List BHC Platform snapshots
hx server start              # Start the persistent build server
hx server status             # Show build server status
```

### Installing Executables

Like `cargo install`: build a package's executables and put them in
`~/.hx/bin` (add that directory to your `PATH`).

```bash
hx install hlint             # Latest version from Hackage
hx install fourmolu@0.18.0   # A specific version
hx install --git <url> --rev v1.2 # From a git repository
hx install --path .          # From a local project
hx install --ghc 9.8.2 hlint # Build with a specific (installed) GHC
hx install --list            # Show installed packages and executables
hx uninstall hlint           # Remove exactly what `hx install` put there
```

The build runs `cabal install` with hx's GHC and Cabal; hx records what it
installed in `~/.hx/installed.toml`. Re-running `hx install` for an installed
version is a no-op (use `--force` to rebuild). An executable that would
overwrite one owned by another package, or a file `hx install` didn't put
there, is reported instead of replaced, unless you pass `--force`.

### IDE & Agent Integration

```bash
hx ide setup                 # Generate hie.yaml for HLS
hx ide status                # Check IDE configuration
hx lsp                       # Start language server
hx mcp                       # Start an MCP server for AI agents (JSON-RPC/stdio)
```

`hx mcp` exposes build/test/run/lock/doctor/dependency tools over the
[Model Context Protocol](https://modelcontextprotocol.io). See
[AGENTS.md](AGENTS.md) for how to drive hx from an AI agent, the Claude skill at
[`.claude/skills/hx/SKILL.md`](.claude/skills/hx/SKILL.md) for slash-command
workflows, and the [`llms.txt`](https://arcanist.sh/hx/llms.txt) project map
(served at `arcanist.sh/hx/llms.txt`) for a compact, agent-friendly overview.

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
hx import --from cabal       # Import a Cabal project (cabal.project or bare .cabal)
hx import --from stack       # Import from stack.yaml
hx nix flake                 # Generate flake.nix
hx nix shell                 # Generate shell.nix
hx completions install       # Auto-install completions for your shell
hx completions generate bash # Generate completions to stdout
hx upgrade                   # Upgrade hx to latest version
```

`hx import --from cabal` works with a full `cabal.project` or just a bare
`.cabal` file — in the latter case the directory is adopted as a single-package
project. `hx import --from stack` adopts an existing Stack project. Running a
project command outside an hx project but inside a Cabal or Stack project
reports "project not found" and suggests `hx import` to adopt it.

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

hx supports global configuration at `~/.config/hx/config.toml` (Linux), `~/Library/Application Support/io.raskell.hx/config.toml` (macOS), or `%APPDATA%\raskell\hx\config\config.toml` (Windows).

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
hx config path --all     # Show every location hx uses, and why
hx config edit           # Open config file in your $EDITOR
hx config init           # Create default config file
hx config set <key> <val> # Set a configuration value
hx config get <key>      # Get a configuration value

# Examples:
hx config set toolchain.ghc 9.8.2
hx config set build.optimization 2
hx config set format.formatter ormolu
```

## Where hx Stores Things

`hx config path --all` prints every location below as it applies on your
machine, with the setting that chose it (`default`, `$XDG_CACHE_HOME`,
`$HX_HOME`, ...).

### Defaults

| What | Linux | macOS | Windows |
|------|-------|-------|---------|
| Global config file | `~/.config/hx/config.toml` | `~/Library/Application Support/io.raskell.hx/config.toml` | `%APPDATA%\raskell\hx\config\config.toml` |
| Global plugins | `~/.config/hx/plugins` | `~/Library/Application Support/io.raskell.hx/plugins` | `%APPDATA%\raskell\hx\config\plugins` |
| Cache root (artifacts, store indexes) | `~/.cache/hx` | `~/Library/Caches/io.raskell.hx` | `%LOCALAPPDATA%\raskell\hx\cache` |
| Cabal store | `~/.cache/hx/cabal/store` | `<cache root>/cabal/store` | `<cache root>\cabal\store` |
| Resolution cache | `~/.cache/hx/solver` | `~/.cache/hx/solver` | `%USERPROFILE%\.cache\hx\solver` |
| Package sources and native package store | `~/.cache/hx/packages` | `~/.cache/hx/packages` | `%USERPROFILE%\.cache\hx\packages` |
| Hackage index mirror | `~/.cache/hx/index` | `~/.cache/hx/index` | `%USERPROFILE%\.cache\hx\index` |
| Stackage snapshots | `~/.cache/hx/stackage` | `~/Library/Caches/hx/stackage` | `%LOCALAPPDATA%\hx\stackage` |
| BHC Platform snapshots | `~/.cache/hx/bhc-platform` | `~/Library/Caches/hx/bhc-platform` | `%LOCALAPPDATA%\hx\bhc-platform` |
| `hx script` builds | `~/.cache/hx/scripts` | `~/Library/Caches/hx/scripts` | `%LOCALAPPDATA%\hx\scripts` |
| GHC / Cabal / HLS installs | `~/.hx/toolchains` | `~/.hx/toolchains` | `%USERPROFILE%\.hx\toolchains` |
| Active tool symlinks | `~/.hx/bin` | `~/.hx/bin` | `%USERPROFILE%\.hx\bin` |
| BHC installs | `~/.bhc` | `~/.bhc` | `%USERPROFILE%\.bhc` |
| Per-project build state | `<project>/.hx/` | `<project>/.hx/` | `<project>\.hx\` |

On macOS and Windows a few caches keep the location earlier releases used, so
upgrading never strands downloaded or built data. Plugins in the pre-0.9.3
global location (`~/Library/Application Support/hx/plugins`,
`%APPDATA%\hx\plugins`) are still loaded. Toolchains live under `~/.hx`
on every platform because GHC's build system cannot handle the space in
`Application Support`.

### Overrides

| Variable | Moves |
|----------|-------|
| `HX_CACHE_DIR` | Every cache location, which then lives directly under it (`$HX_CACHE_DIR/solver`, `$HX_CACHE_DIR/cabal/store`, ...) |
| `XDG_CACHE_HOME` | Same as `HX_CACHE_DIR`, rooted at `$XDG_CACHE_HOME/hx` (any platform) |
| `HX_STORE_DIR` | The Cabal store only |
| `XDG_CONFIG_HOME` | Config file and global plugins, to `$XDG_CONFIG_HOME/hx` (any platform) |
| `HX_CONFIG_FILE` / `--config-file` | The global config file only |
| `HX_HOME` | `~/.hx`: toolchains and tool symlinks |

`HX_*` variables take precedence over `XDG_*` ones. Empty values, and
`XDG_*` values that are not absolute paths, are ignored. `XDG_DATA_HOME`
does not move `~/.hx`, so existing toolchain installs stay where they are;
set `HX_HOME` to relocate them, and move the directory yourself.

`hx cache clean` removes every cache location listed above, including the
older-layout ones.

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

The native solver evaluates `.cabal` conditionals (`os`, `arch`, `impl(ghc)`,
and `flag`) and excludes dependencies of disabled components (`buildable: False`),
so platform- and compiler-specific packages never leak into the lockfile.
Single-package projects (including bare `.cabal` adoptions) get a real lockfile
too, so `hx why`, `hx deps`, and `hx outdated` work there as well.

Packages with a custom `Setup.hs` (`build-type: Custom`) build correctly via
delegation to Cabal.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `HX_VERBOSE` | Enable verbose output |
| `HX_QUIET` | Suppress output |
| `HX_NO_COLOR` | Disable colored output |
| `HX_CONFIG_FILE` | Path to config file |
| `HX_CACHE_DIR` | Cache directory location (see [Where hx Stores Things](#where-hx-stores-things)) |
| `HX_STORE_DIR` | Cabal store location |
| `HX_HOME` | hx home (default `~/.hx`): toolchains, `bin` and the `hx install` manifest |
| `XDG_CACHE_HOME`, `XDG_CONFIG_HOME` | Honoured on every platform |
| `HX_AUTO_INSTALL` | Auto-install missing toolchains |
| `HX_NO_AUTO_INSTALL` | Never auto-install toolchains |
| `HX_ALLOW_UNVERIFIED_DOWNLOADS` | Allow toolchain installs without a published checksum |
| `HX_BHC_PLATFORM_PUBKEY` | Pinned Ed25519 key for BHC Platform snapshot verification |
| `HX_BHC_PLATFORM_URL` | BHC Platform snapshot registry URL (default `https://arcanist.sh/bhc-platform/index.json`) |
| `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` | Route hx's downloads through a proxy (system proxy settings are also honoured) |
| `SSL_CERT_FILE` / `SSL_CERT_DIR` | On Linux, read CA certificates from these locations instead of the system store. hx trusts the operating system's certificate store plus a bundled Mozilla root set, so a CA installed system-wide (for example a corporate TLS proxy's) is trusted on every platform |

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

hx follows the [Astral](https://astral.sh/) playbook — earn trust by improving what exists before replacing any of it:

1. **Wrap first** — Drive the tools you already rely on (GHC, Cabal) instead of reimplementing them.
2. **Tame second** — Layer on better UX, error messages, and workflows until the rough edges disappear.
3. **Replace last** — Swap in faster paths (native builds) and a next-generation compiler ([BHC](docs/BHC_PLATFORM.md)) only where they earn their place.

The destination: a Haskell toolchain that's **fast, unified, and futuristic** — without asking you to throw away what already works.

## License

MIT — part of [arcanist.sh](https://arcanist.sh).
