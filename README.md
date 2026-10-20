<div align="center">

<a href="https://arcanist.sh/hx/">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/static/hx-lockup-dark.svg">
    <img src=".github/static/hx-lockup.svg" alt="hx" height="72">
  </picture>
</a>

<h3>The whole Haskell toolchain. One fast binary.</h3>

<p>
  <a href="https://github.com/arcanist-sh/hx/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/arcanist-sh/hx?color=179299&label=release&style=flat-square"></a>
  <a href="https://crates.io/crates/hx-cli"><img alt="crates.io" src="https://img.shields.io/crates/v/hx-cli?color=179299&style=flat-square"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-179299?style=flat-square"></a>
</p>

<p>
  <a href="https://arcanist.sh/hx/">Website</a> ·
  <a href="https://docs.arcanist.sh/hx/">Documentation</a> ·
  <a href="https://arcanist.sh/hx/benchmarks/">Benchmarks</a> ·
  <a href="https://arcanist.sh/hx/blog/">Blog</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

</div>

---

hx installs GHC and Cabal, builds, tests, formats and lints your code, pins
every dependency by hash, and publishes your packages. It is written in Rust,
starts in milliseconds, and works with the `.cabal` files and Hackage you
already use.

```bash
curl -fsSL https://arcanist.sh/hx/install.sh | sh
hx init my-app && cd my-app
hx run
```

## Six tools, one interface

A Haskell project usually means ghcup for the compiler, cabal or Stack to
build, fourmolu and hlint for style, hpc for coverage, and a set of shell
scripts to hold it together. hx gives all of it one command line and one
config file.

| Task | Today | With hx |
|------|-------|---------|
| Install a compiler | `ghcup install ghc 9.12.2` | `hx toolchain install 9.12.2` |
| Build and test | `cabal build && cabal test` | `hx build && hx test` |
| Format and lint | `fourmolu -i src && hlint src` | `hx fmt && hx lint` |
| Pin dependencies | `cabal freeze` | `hx lock` |
| Measure coverage | `cabal test --enable-coverage` | `hx coverage --html` |
| Install a tool | `cabal install hlint` | `hx install hlint` |

hx drives GHC and Cabal rather than replacing them. Your `.cabal` files stay
the source of truth, and the project still builds with plain cabal.

## What you get

- **The current GHC, the day it ships.** hx reads GHC, Cabal and HLS releases
  and checksums from the metadata ghcup publishes, verifies every download,
  and picks the build made for your Linux distribution, Alpine included. Pin
  versions per project in `hx.toml`; no ghcup or Stack install required.
- **Same inputs, same build.** `hx.lock` pins every dependency by the SHA-256
  of its source tarball and its exact Hackage revision, with the index state,
  compiler and platform. `hx fetch` verifies every source, `hx sync --offline`
  builds without a network, and `hx lock --check` fails CI when the lockfile
  drifts.
- **Workspaces.** `cabal.project` stays the single member list. Commands cover
  every member at the root or the current one inside it, and
  `hx test --changed main` runs only what a branch affects.
- **Fast where you feel it.** On a three-module project, hx 0.9.2 starts
  6.0× faster than cabal, cold-builds 5.6× faster and finishes a no-op rebuild
  4.8× faster ([method and numbers](https://arcanist.sh/hx/benchmarks/)).
- **Built for the agent in the loop.** `hx mcp` serves build, test, run, lock,
  doctor and dependency tools over the Model Context Protocol, with structured
  errors and exit codes. See [AGENTS.md](AGENTS.md) and the
  [llms.txt](https://arcanist.sh/hx/llms.txt) project map.
- **Everything else you reach for.** `hx install` (the `cargo install` of
  Haskell), publishing to Hackage or the signed
  [arcanist.sh registry](https://arcanist.sh/registry/), `hx import` for
  existing Cabal and Stack projects, watch mode, coverage, profiling, HLS
  setup, Steel plugins that only run once you trust the project, and release
  tooling (`hx dist`, Homebrew formulas, Nix flakes).
- **One switch to a second compiler.** hx is the front end for
  [BHC](https://arcanist.sh/bhc/), the Basel Haskell Compiler: set
  `backend = "bhc"` and build and test with the same commands.

## Installation

| Platform | Command |
|----------|---------|
| macOS and Linux | `curl -fsSL https://arcanist.sh/hx/install.sh \| sh` |
| Homebrew | `brew install arcanist-sh/tap/hx` (installs as `hxs`) |
| Cargo | `cargo install hx-cli` |
| Nix | `nix run github:arcanist-sh/hx` (also on FlakeHub as `arcanist-sh/hx`) |
| aqua | `aqua g -i arcanist-sh/hx` |
| Windows | `winget install raskell-io.hx`, or the zip from the [latest release](https://github.com/arcanist-sh/hx/releases/latest) |
| Container | `docker run --rm ghcr.io/arcanist-sh/hx --help` |

The install script puts a single binary in `~/.local/bin`. On x86_64 Linux it
is the fully static build, so it runs on any distribution; the other Linux
builds need glibc 2.28 or newer (Debian 10, Ubuntu 20.04, RHEL 8). Release
binaries cover macOS (Apple silicon and Intel), Linux (x86_64 and arm64) and
Windows (x86_64). `hx upgrade` moves to the newest release.

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

### Workspaces

```bash
hx workspace init            # Make this directory a multi-package workspace root
hx workspace add ./lib-a     # Add a member (cabal.project stays the member list)
hx workspace list            # Members, versions, dependencies between them
hx build -p lib-a -p app     # Build selected members
hx test --changed origin/main  # Test only members a branch affects (+ dependents)
```

At the root, `hx build`/`test`/`check` cover every member. Inside a member
directory they target that member, still using the root lockfile and
toolchain. See [docs/WORKSPACES.md](docs/WORKSPACES.md).

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
hx lock --check              # Verify hx.lock (for CI; non-zero exit on mismatch)
hx sync                      # Build with locked dependencies
hx fetch                     # Download + verify locked sources in parallel
hx sync --offline            # Build from cached sources only (after hx fetch)

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
hx search aeson --registry arcanist  # Search the arcanist.sh registry instead
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
hx toolchain list            # Installed, recommended and latest GHC/Cabal
hx toolchain list --available # All available versions
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

#### Package registries

Hackage stays the default. You can also target a package registry such as
the [arcanist.sh registry](https://arcanist.sh/registry/)
(`https://registry.arcanist.sh`), or any URL. Plain cabal consumes these registries as a secure (hackage-security)
`repository`, so builds work without hx. See [docs/REGISTRY.md](docs/REGISTRY.md).

```bash
hx registry login --registry arcanist          # store an API token (prompted, kept 0600)
hx publish --registry arcanist                 # same checks, then upload the sdist
hx registry info my-lib                        # versions, owners, BHC verification
hx registry yank my-lib 1.2.0 [--undo]         # cabal avoids yanked versions
hx registry owner add my-lib alice
hx registry token create ci --scope publish --package my-lib
hx registry add --registry arcanist            # add the repository stanza to cabal.project
```

Name registries in `hx.toml` or the global `config.toml`:

```toml
[registries.arcanist]
url = "https://registry.arcanist.sh"   # built in; override it here, e.g. for a local server
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

The `hx.lock` file ensures reproducible builds. It pins every dependency's
exact version, the SHA-256 of its source tarball, the Hackage `.cabal`
revision it was resolved with, and the Hackage index state:

```toml
version = 2
created_at = "2026-10-19T08:09:08Z"

[toolchain]
ghc = "9.8.2"
cabal = "3.12.1.0"

[plan]
compiler_id = "ghc-9.8.2"
platform = "x86_64-linux"
index_state = "2026-10-19T07:18:30Z"
hash = "sha256:..."

[[packages]]
name = "containers"
version = "0.8.1"
source = "hackage"
sha256 = "9bd0f5156306b04a799c32ff49a6129f7c8dd6ec96c6ddeb7885975ffffc9e99"
revision = 0
cabal_sha256 = "11cc66be32854dc6b9c04c1e4f9a3ecb4939399a3b54bf00c6c085d538470b12"
```

`hx sync` builds with exactly these pins. For offline or air-gapped builds,
run `hx fetch` once while online (it downloads and verifies every source),
then `hx sync --offline` (or `HX_OFFLINE=1`). In CI, `hx lock --check` fails
when hx.lock is stale or a cached source does not match its hash. See
[docs/LOCKFILE.md](docs/LOCKFILE.md) for the full format and workflow.

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
| `HX_OFFLINE` | Work offline: never refresh ghcup release metadata (use the cache or built-in tables), and build `hx sync` from cached sources only (like `--offline`) |
| `HX_GHCUP_METADATA_URL` | Alternative ghcup metadata file (mirror, or `file://` path) |
| `HX_BHC_PLATFORM_PUBKEY` | Pinned Ed25519 key for BHC Platform snapshot verification |
| `HX_BHC_PLATFORM_URL` | BHC Platform snapshot registry URL (default `https://arcanist.sh/bhc-platform/index.json`) |
| `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` | Route hx's downloads through a proxy (system proxy settings are also honoured) |
| `SSL_CERT_FILE` / `SSL_CERT_DIR` | On Linux, read CA certificates from these locations instead of the system store. hx trusts the operating system's certificate store plus a bundled Mozilla root set, so a CA installed system-wide (for example a corporate TLS proxy's) is trusted on every platform |
| `HX_REGISTRY_TOKEN` | API token for `--registry` commands (overrides `credentials.toml`) |
| `HX_HACKAGE_URL` | Hackage mirror `hx fetch` downloads from (`file://` mirrors work) |

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
| `hx-registry` | Package registry client (publish, search, tokens) |

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

The destination is the whole Haskell toolchain in one fast binary, without asking you to throw away what already works.

## License

MIT. hx is part of [arcanist.sh](https://arcanist.sh): Haskell tooling for the age of AI-authored code.
