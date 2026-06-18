# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.3] - 2026-06-18

### Changed
- CI: release workflow actions bumped to their Node 24 majors (`upload-artifact` v7, `download-artifact` v8, `action-gh-release` v3) to clear Node 20 deprecation warnings; added a grouped GitHub Actions Dependabot config so future runtime deprecations surface as PRs.

## [0.7.2] - 2026-06-18

### Changed
- Refreshed documentation and install links from the legacy `raskell.io` / `raskell-io` brand to the current `arcanist.sh` domains and `arcanist-sh` GitHub org (README, getting-started, benchmarks, contributing, and the crate `repository` metadata). Package-manager identifiers that are still published under the old namespace (e.g. the WinGet package) are left unchanged.

## [0.7.1] - 2026-06-18

### Added
- **`hx import --from stack` handles multi-package projects** - a `stack.yaml` listing local `packages:` now generates a `cabal.project` alongside `hx.toml` so hx recognizes the workspace members, instead of dropping them ([#2])

### Fixed
- **`hx import --from stack` respects the explicit `compiler:` field** - it now takes precedence over the resolver-derived GHC default ([#2])
- **`hx import --from stack` strips inline YAML comments** - trailing `# ...` annotations on extra-deps/resolver lines no longer leak into the imported version string ([#2])

[#2]: https://github.com/arcanist-sh/hx/issues/2

## [0.7.0] - 2026-06-17

### Changed
- **BHC build pipeline emits BHC-native flags** - `hx build` now generates BHC's actual CLI flags instead of GHC-style ones: `--hidir`/`--odir`, `--import-path <dir>`, `--package-db <path>`, `-O <n>`, `--Wall`/`--Werror`, and `.bhi` interface files
- Package database is read directly from the filesystem by scanning `.conf` files instead of shelling out to `bhc-pkg`, dropping the `which` dependency
- Pinned the Rust toolchain to 1.96.0 (`rust-toolchain.toml` + `mise.toml`) and set `rust-version = "1.96"` as the workspace MSRV
- Refreshed dependencies within semver and upgraded `clap_mangen` 0.2 → 0.3

### Added
- BHC builtin package mapping (`base`, `text`, `containers`, …) so packages provided by BHC's stdlib are skipped during compilation

## [0.6.0] - 2026-06-12

### Added
- **BHC-native build pipeline** - `hx build` drives BHC directly through its real CLI flags, with a native builder, package database handling, and per-package builds
- **BHC REPL backend** - `hx repl` works against BHC projects
- **Content-addressed artifact cache** - compiled artifacts keyed by SHA-256 of source, compiler version, flags, and dependency versions (`hx cache artifacts`)
- **Zero-config BHC experience** - backend auto-detection, bundled toolchain install, platform matching, and doctor checks
- **`hx plugins trust` / `hx plugins untrust`** - explicit per-project trust for local plugins (see Security)
- `Version` now models 4-part versions (e.g. cabal `3.12.1.0`) without lossy patch-component folding

### Security
- **Toolchain download verification** - GHC, Cabal, and BHC archives are now verified against their published SHA-256 checksums before installation; previously downloaded archives are re-verified before reuse. Installs refuse to proceed without a published checksum unless `HX_ALLOW_UNVERIFIED_DOWNLOADS=1` is set
- **Project-local plugins require trust** - `.hx/plugins/*.scm` scripts no longer run automatically when building a freshly cloned project; grant trust per project with `hx plugins trust` (recorded in the global config, never in the repo itself)
- **Plugin hook timeouts are enforced** - `[plugins].hook_timeout_ms` (default 5000) now actually cancels hung hooks instead of stalling builds forever; set to 0 to disable
- **Archive extraction hardening** - zip and tar extraction reject entries that would escape the destination directory (zip-slip / path traversal)
- **Verified self-update** - `hx upgrade` downloads the release archive, verifies its published `.sha256`, and only then replaces the binary
- **install.sh requires checksums** - the installer now fails instead of warning when checksum verification is impossible (override with `HX_ALLOW_UNVERIFIED=1`), and pins curl/wget to HTTPS + TLS 1.2
- **BHC Platform key pinning** - `HX_BHC_PLATFORM_PUBKEY` pins an independent Ed25519 trust root for snapshot signatures; a warning is emitted when the key comes from the registry itself
- **Honest `hx audit`** - removed the simulated vulnerability database; the command now states clearly that HSEC advisory integration is not yet available instead of fabricating results
- Cache directories are created with owner-only permissions (0700) on Unix

### Fixed
- Lockfile, cache indices, manifest, and global config are written atomically (temp file + rename), so an interrupted command can no longer corrupt them
- Watch mode ignores `dist-newstyle/`, `dist/`, `.hx/`, `.git/`, and `.stack-work/`, preventing rebuild loops when the project root is watched
- Crash fixes: compiler output lines ending in "Compiling", HTTP 304 responses without local index state, non-ASCII package names in `hx add`, and missing home directory no longer panic
- Unparseable `.cabal` version constraints now log a warning instead of being silently treated as unconstrained
- Lockfile read errors now include the file path

## [0.5.0] - 2026-02-02

### Added
- **BHC test and run commands** - Full BHC backend support for `hx test` and `hx run`, replacing previous stubs with implementations that generate manifests, detect the backend, and invoke `bhc test`/`bhc run`
- **`--backend` flag on init/new** - `hx init --backend bhc` and `hx new <template> --backend bhc` to scaffold projects with BHC from the start
- **Numeric project template** - `hx new numeric <name>` creates a BHC-optimized numeric computing project with hmatrix, vector, statistics, and massiv dependencies
- **Server project template** - `hx new server <name>` creates a BHC-optimized web server project with Servant, Warp, and WAI dependencies
- **BHC Platform curated snapshots** - Stackage-like curated package sets for BHC
  - `hx bhc-platform list` - List available BHC Platform snapshots
  - `hx bhc-platform info <platform>` - Show snapshot details and packages
  - `hx bhc-platform set <platform>` - Set snapshot for current project
  - `bhc-platform-2026.1` initial snapshot with ~70 curated packages
  - `[bhc-platform]` configuration section in hx.toml
  - Lock/resolver integration for pinning BHC Platform package versions
- **WinGet distribution** - `winget install raskell-io.hx` now available on Windows ([winget-pkgs#333584](https://github.com/microsoft/winget-pkgs/pull/333584))

### Changed
- Template system now supports `{{backend_config}}` substitution for compiler backend configuration
- `BhcPlatform` snapshot type added to the solver alongside Stackage LTS/Nightly
- `BhcPlatformConfig` added to manifest with `snapshot`, `allow_newer`, and `extra_deps` fields

### Fixed
- Pre-existing clippy warnings across hx-bhc, hx-cabal, hx-doctor, and hx-cli (collapsible_if, wildcard_in_or_patterns, too_many_arguments)

## [0.4.0] - 2026-01-18

### Added
- **BHC (Basel Haskell Compiler) support** - Alternative compiler backend
  - New `hx-compiler` crate with `CompilerBackend` trait abstraction
  - New `hx-bhc` crate implementing BHC backend
  - `[compiler]` section in hx.toml for backend configuration
  - `--backend` flag to override compiler selection
  - BHC profiles: default, server, numeric, edge
- **Comprehensive benchmarks and testing**
  - CLI benchmarks with Criterion (startup, init, doctor, config, clean, completions)
  - 15+ end-to-end integration tests for complete workflows
  - Benchmark comparison script (`scripts/benchmark-comparison.sh`)
  - 430+ unit tests across all crates

### Changed
- Compiler abstraction layer enables future compiler integrations
- Updated documentation with benchmark results and testing guide

## [0.3.6] - 2026-01-17

### Added
- **Stackage CLI commands**
  - `hx stackage list` - List available snapshots (--lts, --nightly)
  - `hx stackage info <snapshot>` - Show snapshot details
  - `hx stackage set <snapshot>` - Set snapshot for project
- **Cross-compilation enhancements**
  - `--target` flag for build, test, and run commands
  - Support for x86_64-linux-gnu, aarch64-linux-gnu, wasm32-wasi, and more
- **Stackage snapshot support in lockfiles**
  - `[toolchain] snapshot = "lts-22.7"` configuration
  - Automatic resolver selection from snapshot

### Fixed
- Improved preprocessor tool discovery (alex, happy, hsc2hs, c2hs)
- Better hsc2hs support with proper include paths

## [0.3.5] - 2026-01-17

### Added
- **Native build advanced features**
  - Preprocessor support: alex (.x), happy (.y), hsc2hs (.hsc), c2hs (.chs)
  - Parallel module compilation
  - Fingerprint-based incremental builds
  - Automatic fallback to cabal for complex projects

### Fixed
- Use hx-managed toolchain path for native builds and server
- Handle missing parent directory in ghc_path for server
- Edge case handling and robustness improvements

## [0.3.0] - 2026-01-17

### Added
- **install.sh** - One-liner installation script
- **Global configuration** - `~/.config/hx/config.toml` for user defaults
- **Smart defaults for hx init**
  - Auto-detect project name from directory
  - Intelligent default GHC version selection
  - Simplified interactive prompts
- **Shell completions auto-install**
  - Automatically install completions on first run
  - Support for bash, zsh, fish detection
- **New dependency commands**
  - `hx info <package>` - Show package details from Hackage
  - `hx list` - Alias for `hx deps list`
  - `hx tree` - Alias for `hx deps tree`
  - `hx update` - Update dependencies to latest compatible versions
  - `hx outdated` - Check for available dependency updates
  - `hx why <package>` - Show why a dependency is included
- **Enhanced hx add/remove**
  - Version constraint support: `hx add aeson@^2.2`
  - Automatic hx.toml synchronization

### Changed
- Improved CLI test infrastructure
- Better error messages with context

## [0.2.0] - 2026-01-16

### Added
- **hx-solver crate** - Native dependency resolver written in Rust
  - Version constraint parsing and solving
  - Hackage index loading and caching
  - Build plan generation
- **hx-lsp crate** - Language server protocol support
  - HLS process management
  - Diagnostic forwarding
- **Native build mode** (`hx build --native`)
  - Direct GHC invocation for simple projects
  - 5.6x faster cold builds vs cabal
  - 7.8x faster incremental builds
- **Watch mode** (`hx watch`)
  - File change detection with notify
  - Automatic rebuild on save
  - Support for `hx watch test`, `hx watch build`
- **Coverage reporting** (`hx coverage`)
  - HPC integration
  - HTML and JSON output formats
  - Threshold checking for CI
- **Server commands**
  - `hx server start` - Start HLS in background
  - `hx server stop` - Stop HLS
  - `hx server status` - Check HLS status
  - `hx server restart` - Restart HLS

### Improved
- Performance optimizations across all commands
- Better Hackage integration

## [0.1.1] - 2026-01-16

### Added
- `hx completions <shell>` - Generate shell completions for bash, zsh, fish, PowerShell
- `hx upgrade` - Self-update to latest version from GitHub releases
- `hx upgrade --check` - Check for updates without installing
- `hx upgrade --target <version>` - Install a specific version

### Improved
- Enhanced error messages with actionable fix suggestions
- Added convenience error constructors with intelligent default fixes
- Toolchain missing errors now suggest both ghcup and hx commands
- Build errors analyze content to suggest relevant fixes

## [0.1.0] - 2026-01-15

### Added

#### Core Commands
- `hx init` - Initialize new Haskell projects with `--bin`, `--lib`, `--name`, `--dir` options
- `hx build` - Build projects via Cabal with `--release`, `--jobs`, `--target` options
- `hx test` - Run tests with optional `--pattern` matching
- `hx run` - Build and run executables, passing arguments through
- `hx repl` - Start an interactive GHCi session
- `hx check` - Fast type-checking (alias to build)
- `hx clean` - Clean build artifacts with `--global` option

#### Dependency Management
- `hx lock` - Generate deterministic lockfile (`hx.lock`) with package versions and fingerprints
- `hx sync` - Build with locked dependencies, verify toolchain compatibility
- `hx add` - Add dependencies to `.cabal` file
- `hx rm` - Remove dependencies from `.cabal` file

#### Code Quality
- `hx fmt` - Format Haskell source files with fourmolu/ormolu, supports `--check` mode
- `hx lint` - Lint with hlint, supports `--fix` for auto-fixes

#### Toolchain Management
- `hx toolchain status` - Show installed GHC, Cabal, GHCup, HLS versions
- `hx toolchain install` - Install toolchain components via GHCup
- `hx toolchain use` - Switch active toolchain version or use project settings

#### Diagnostics
- `hx doctor` - Comprehensive diagnostics with actionable fix suggestions

#### Configuration
- `hx.toml` manifest format with project, toolchain, format, lint sections
- `hx.lock` TOML lockfile format with fingerprint verification
- Environment variable support: `HX_VERBOSE`, `HX_QUIET`, `HX_NO_COLOR`, `HX_CONFIG_FILE`, etc.

#### Developer Experience
- Global `--verbose`, `--quiet`, `--no-color`, `--config-file` flags
- Colored output with automatic terminal detection
- Progress spinners for long-running operations
- Structured error messages with fix suggestions
- Warning system with deduplication (`warn_user_once!`)

### Architecture
- Rust workspace with 11 crates: `hx-cli`, `hx-core`, `hx-config`, `hx-lock`, `hx-toolchain`, `hx-cabal`, `hx-cache`, `hx-doctor`, `hx-ui`, `hx-telemetry`, `hx-warnings`
- Async runtime with Tokio
- Structured logging with tracing
- UV-inspired patterns: Printer abstraction, Combine trait, EnvVars constants

### Testing
- 26 automated tests
- Integration test infrastructure with assert_cmd
- CI/CD with GitHub Actions (Linux, macOS, Windows)

[Unreleased]: https://github.com/arcanist-sh/hx/compare/v0.7.3...HEAD
[0.7.3]: https://github.com/arcanist-sh/hx/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/arcanist-sh/hx/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/arcanist-sh/hx/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/arcanist-sh/hx/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/arcanist-sh/hx/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/raskell-io/hx/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/raskell-io/hx/compare/v0.3.6...v0.4.0
[0.3.6]: https://github.com/raskell-io/hx/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/raskell-io/hx/compare/v0.3.0...v0.3.5
[0.3.0]: https://github.com/raskell-io/hx/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/raskell-io/hx/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/raskell-io/hx/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/raskell-io/hx/releases/tag/v0.1.0
