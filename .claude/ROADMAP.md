# HX Roadmap

## v0.1.0 - Initial Release ✅

### Done Definition

hx v0.1.0 is shippable when:
- [x] `hx init` creates a project that builds on at least Linux/macOS
- [x] `hx toolchain status` works robustly
- [x] `hx build/test/run/repl` work for a basic project
- [x] `hx lock` produces deterministic lockfile
- [x] `hx sync` enforces the lock
- [x] `hx doctor` gives actionable fixes for missing tools/mismatch

### Milestone A: Scaffolding ✅
- [x] Create Rust workspace with all crates
- [x] Implement `hx --version`, `hx help`
- [x] Config loading (hx.toml parsing)
- [x] Project root detection
- [x] Structured logging with `tracing`
- [x] UI helpers (spinners, progress)

### Milestone B: Toolchain Detection ✅
- [x] `hx toolchain status` - detect ghc, cabal, ghcup, hls
- [x] `hx doctor` with actionable fixes

### Milestone C: Build/Test/Run ✅
- [x] `hx build`, `hx test`, `hx run`, `hx repl`, `hx check`
- [x] Stable build directories
- [x] Friendly error messages

### Milestone D: Project Initialization ✅
- [x] `hx init` with --bin, --lib, --name, --dir, --ci flags
- [x] Generate hx.toml, .gitignore, .cabal, source files

### Milestone E: Lock/Sync ✅
- [x] `hx lock` - generate hx.lock with fingerprint
- [x] `hx sync` - enforce lockfile during build

### Milestone F: Format/Lint ✅
- [x] `hx fmt` (fourmolu/ormolu)
- [x] `hx lint` (hlint with --fix)

### Milestone G: Doctor Diagnostics ✅
- [x] Platform-specific native dependency checks
- [x] Better Cabal error extraction
- [x] Actionable fix commands

---

## v0.2.0 - Feature Complete ✅

### Performance & Caching
- [x] Shared build store keyed by fingerprints
- [x] `hx cache status|prune|clean` commands
- [x] Source fingerprinting for incremental builds
- [x] `hx fetch` for parallel dependency downloads

### Workspace Support
- [x] Multi-package repos (cabal.project parsing)
- [x] Single lockfile for workspaces
- [x] `--package` flag for workspace commands

### Toolchain Management
- [x] `hx toolchain install` via ghcup
- [x] `hx toolchain use` for per-project pins
- [x] Automatic toolchain installation on mismatch

### Native Dependency Resolver
- [x] `hx-solver` crate with PVP version parsing
- [x] Hackage 01-index.tar.gz parsing
- [x] Backtracking dependency resolution
- [x] Cycle detection
- [x] Binary caching of PackageIndex

### Native Package Fetching
- [x] Direct Hackage downloads with parallel concurrency
- [x] SHA256 verification
- [x] Progress tracking with ETA

### Build Plan Generation
- [x] Topological sorting of dependencies
- [x] Pre-installed package detection
- [x] Build fingerprint calculation

### Hackage Index Mirroring
- [x] `hx index update|status|clear`
- [x] HTTP conditional requests (ETag, If-Modified-Since)
- [x] Auto-update when stale

### Project Templates
- [x] `hx new webapp|cli|library`
- [x] `hx new --template <git-url>`
- [x] Template variable substitution

### Artifact Caching
- [x] Content-addressed storage for .o/.hi files
- [x] `hx cache artifacts status|prune|clear`

### Module Dependency Analysis
- [x] Import statement parsing
- [x] Topological ordering for compilation
- [x] Parallel compilation grouping

### Native Build Orchestration
- [x] `hx build --native` for direct GHC invocation
- [x] Module dependency graph extraction
- [x] Parallel compilation with dependency tracking
- [x] Package database integration

### Documentation Generation
- [x] `hx docs` with --open, --deps, --serve flags

### IDE Integration
- [x] `hx ide setup|status`
- [x] Auto-generate hie.yaml
- [x] HLS version compatibility checks

### Package Publishing
- [x] `hx publish` with pre-publish checks
- [x] Hackage upload, dry-run mode, --docs flag

### Changelog Generation
- [x] `hx changelog` from git commits
- [x] Conventional commits support
- [x] --preview and --unreleased flags

### Nix Integration
- [x] `hx nix flake|shell` for Nix file generation
- [x] Generate flake.nix/shell.nix from hx.toml

### Profiling Integration
- [x] `hx profile` with --heap, --time flags
- [x] Build with -prof, run with RTS options
- [x] Parse and display profiling output

### Single-File Scripts
- [x] `hx script <file.hs>`
- [x] Parse dependencies from file header
- [x] Cache compiled scripts

### Stack Project Import
- [x] `hx import --from stack|cabal`
- [x] Parse stack.yaml, convert resolver to GHC version

### Package Search
- [x] `hx search <query>` with fuzzy matching
- [x] --limit and --detailed flags

### Dependency Audit
- [x] `hx audit` for vulnerability scanning
- [x] Detect outdated/deprecated packages
- [x] License checking, --fix flag

---

## v0.3.0 - Native Build & Beyond

### Full Native Build Replacement ✅
- [x] Complete GHC invocation without cabal dependency
  - [x] Enhanced cabal file parsing (extensions, source dirs, c-sources, ghc-options)
  - [x] Package source extraction from Hackage tarballs
  - [x] Native package compilation from source with GHC
  - [x] Library creation with ar/libtool
  - [x] Package registration file generation
  - [x] Package database management with ghc-pkg
  - [x] Full dependency build orchestration (FullNativeBuilder)
  - [x] Topological ordering of package builds
  - [x] Pre-installed package detection
  - [x] Package cache with content-addressed keys
- [x] Drop cabal as runtime dependency for simple builds

### Native Build Advanced Features ✅
- [x] Preprocessor integration
  - [x] Alex lexer generator (.x → .hs)
  - [x] Happy parser generator (.y/.ly → .hs)
  - [x] hsc2hs C FFI bindings (.hsc → .hs)
  - [x] Automatic detection and invocation before compilation
  - [x] Support in both project and dependency builds
- [x] Custom Setup.hs support
  - [x] Parse custom-setup stanza from .cabal files
  - [x] Compile Setup.hs with setup-depends
  - [x] Execute Setup configure/build/copy/register
  - [x] Integration with FullNativeBuilder
- [x] Persistent compilation server
  - [x] `hx server start|stop|status|restart` commands
  - [x] GHCi-based incremental reloading
  - [x] Sub-second rebuild potential via :reload

### Watch Mode ✅
- [x] `hx watch` for auto-rebuild on file changes
  - [x] File system monitoring (notify crate)
  - [x] Debounced rebuilds
  - [x] Test re-run on change
  - [x] Clear terminal between runs

### Dependency Visualization ✅
- [x] `hx deps graph` command
  - [x] Graphviz DOT output
  - [x] ASCII tree fallback
  - [x] JSON output format
  - [x] Simple list format
  - [x] Filter by depth
  - [x] Highlight specific packages
  - [x] Direct-only filter
- [x] `hx deps tree` convenience alias
- [x] `hx deps list` convenience alias

### Language Server Features ✅
- [x] Built-in diagnostics without HLS
  - [x] Type error extraction from GHC
  - [x] Warning aggregation
  - [x] Quick-fix suggestions
- [x] `hx lsp` for minimal language server

### Plugin System ✅
- [x] User-defined commands via hooks
  - [x] Pre/post build, test, run, clean, lock hooks
  - [x] Custom command registration
  - [x] Steel (Scheme) plugin runtime
  - [x] `hx plugins list|status|run` commands
  - [x] Plugin API for project info, shell commands, file operations
  - [x] Thread-local context for implicit plugin state access

### GHC Version Manager ✅
- [x] Replace ghcup dependency
  - [x] Direct GHC binary downloads from downloads.haskell.org
  - [x] Version switching with `hx toolchain use`
  - [x] Per-project GHC installations via hx.toml
  - [x] `hx toolchain list` with --available/--installed flags
  - [x] `hx toolchain install <version>` with --set/--force/--ghcup options
  - [x] `hx toolchain remove <version>` with confirmation
  - [x] PATH injection for builds using resolved GHC
  - [x] Graceful fallback to ghcup when direct download fails

### Binary Distribution ✅
- [x] `hx dist` command
  - [x] Build release binary for target platform
  - [x] Strip debug symbols
  - [x] Create tar.gz/zip archives with metadata
  - [x] Bundle shell completions (bash, fish, zsh, powershell)
  - [x] SHA256 checksum generation
- [x] `hx dist formula` - Generate Homebrew formula
- [x] `hx dist install-script` - Generate shell installation script

### Test Coverage ✅
- [x] `hx coverage` command
  - [x] Integration with hpc (Haskell Program Coverage)
  - [x] Build and test with `--enable-coverage`
  - [x] Find and combine .tix/.mix files
  - [x] Text report with `hpc report`
  - [x] HTML coverage reports with `hpc markup`
  - [x] Coverage thresholds (--threshold flag)
  - [x] JSON output for CI integration (--json flag)
  - [x] Module exclusion (--exclude flag)
  - [x] Browser open support (--open flag)

### Stackage Snapshot Support ✅
- [x] Stackage integration in resolver
  - [x] Parse LTS and Nightly snapshot identifiers
  - [x] Fetch snapshot metadata from Stackage API
  - [x] Pin package versions from snapshots
  - [x] Local snapshot caching
- [x] `hx stackage` commands
  - [x] `hx stackage list` - Show available snapshots
  - [x] `hx stackage info <snapshot>` - Show snapshot details
  - [x] `hx stackage set <snapshot>` - Set project snapshot
- [x] `hx lock --snapshot` shorthand
- [x] GHC version validation against snapshot

### Cross-Compilation Support ✅
- [x] `--target` flag for builds
  - [x] `hx build --target <triple>`
  - [x] `hx test --target <triple>`
  - [x] `hx run --target <triple>`
- [x] Common target triple support
  - [x] x86_64-linux-gnu, aarch64-linux-gnu
  - [x] x86_64-unknown-linux-musl, aarch64-unknown-linux-musl
  - [x] x86_64-apple-darwin, aarch64-apple-darwin
  - [x] x86_64-w64-mingw32
- [x] Native build cross-compilation (`--native --target`)

---

## v0.4.0 - BHC Foundation ✅

### BHC Compiler Backend ✅
- [x] `CompilerBackend` trait abstraction (`hx-compiler` crate)
- [x] `hx-bhc` crate implementing BHC backend
- [x] `[compiler]` section in hx.toml for backend configuration
- [x] `--backend` flag on build/test/run to override compiler selection
- [x] BHC profiles: default, server, numeric, edge
- [x] BHC diagnostics parsing

---

## v0.5.0 - Deep BHC Integration ✅

### BHC Test & Run ✅
- [x] Full `hx test --backend bhc` implementation
  - [x] BHC manifest generation
  - [x] Backend detection and version checking
  - [x] Test argument construction with pattern/package/target support
- [x] Full `hx run --backend bhc` implementation
  - [x] BHC manifest generation
  - [x] RunOptions with args, package, target, verbose

### `--backend` on Init & New ✅
- [x] `hx init --backend bhc` sets compiler backend in hx.toml
- [x] `hx new webapp|cli|library --backend bhc` with backend config in templates
- [x] `{{backend_config}}` template substitution system

### BHC Project Templates ✅
- [x] `hx new numeric <name>` - Numeric computing template
  - [x] BHC numeric profile with tensor_fusion enabled
  - [x] hmatrix, vector, statistics, massiv dependencies
  - [x] Example matrix/vector computations
  - [x] Property tests with QuickCheck
- [x] `hx new server <name>` - Web server template
  - [x] BHC server profile
  - [x] Servant API with Warp server
  - [x] WAI middleware (logging)
  - [x] Environment-based configuration
  - [x] hspec-wai integration tests

### BHC Platform Curated Snapshots ✅
- [x] `BhcPlatform` snapshot type in solver
  - [x] `SnapshotId` parsing for `bhc-platform-YYYY.N` format
  - [x] Embedded TOML snapshot data via `include_str!`
- [x] `bhc-platform-2026.1` initial snapshot (~70 curated packages)
- [x] `[bhc-platform]` config section in hx.toml (snapshot, allow_newer, extra_deps)
- [x] `hx bhc-platform list` - List available snapshots
- [x] `hx bhc-platform info <platform> [--packages]` - Show snapshot details
- [x] `hx bhc-platform set <platform>` - Set snapshot for project
- [x] Lock/resolver integration: pin BHC Platform package versions

### Documentation ✅
- [x] `docs/BHC_PLATFORM.md` — Developer-facing BHC Platform guide
- [x] Updated `docs/GETTING_STARTED.md` — BHC in data flow, crate map, "where to start"
- [x] Updated `docs/STACKAGE.md` — Cross-reference to BHC Platform
- [x] Updated `docs/BENCHMARKS.md` — Version bump to 0.5.0
- [x] hx.raskell.io-docs: new `commands/bhc-platform.md` command reference
- [x] hx.raskell.io-docs: new `features/bhc-platform.md` feature guide
- [x] hx.raskell.io-docs: updated `commands/new.md` (numeric/server templates)
- [x] hx.raskell.io-docs: updated `commands/init.md` (--backend example)
- [x] hx.raskell.io-docs: updated `configuration/hx-toml.md` ([bhc-platform] section)
- [x] hx.raskell.io-docs: updated `features/compiler-backends.md` (test/run, templates, platform)
- [x] hx.raskell.io-docs: updated `quickstart.md` (specialized templates)
- [x] hx.raskell.io: updated BHC blog post for v0.5.0

---

## v0.6.0 - BHC Platform Ecosystem

The pivot point. hx stops being "a nice wrapper around GHC/Cabal" and starts
becoming the integrated toolchain — where hx + BHC is to Haskell what
cargo + rustc is to Rust.

### BHC Platform Expansion ✅
- [x] `bhc-platform-2026.2` snapshot (expanded package set, ~120 packages)
  - [x] Broader web coverage: scotty, yesod-core, IHP-core
  - [x] Broader data coverage: cassava, xlsx, postgresql-simple
  - [x] Broader crypto: crypton, tls, x509
- [x] Remote snapshot registry
  - [x] `hx bhc-platform update` — fetch latest snapshot index from registry
  - [x] Snapshot TOML served from `https://bhc-platform.raskell.io/`
  - [x] Offline fallback to last-fetched + embedded snapshots
  - [x] Signed snapshots (Ed25519 signature verification)
- [x] Snapshot diffing
  - [x] `hx bhc-platform diff <old> <new>` — show added/removed/upgraded packages
  - [x] Migration hints when upgrading snapshots
- [x] Custom/overlay snapshots
  - [x] `[bhc-platform] overlay = "path/to/overlay.toml"` for org-private packages
  - [x] Merge strategy: overlay wins over base snapshot

### BHC-Native Build Pipeline ✅
- [x] `hx build` invokes BHC directly (no Cabal intermediary for BHC projects)
  - [x] hx owns the full build graph: resolve → fetch → compile → link
  - [x] Module dependency extraction from BHC (not GHC)
  - [x] Parallel compilation via BHC's native parallelism
- [x] Incremental compilation aware of BHC's interface files
- [x] Content-addressed build cache shared between BHC projects
  - [x] `hx cache` stores compiled BHC artifacts keyed by (package, version, profile, flags)
  - [x] Cross-project cache sharing (like cargo's target/ but smarter)
- [x] BHC-native REPL
  - [x] `hx repl --backend bhc` with BHC interactive mode
  - [x] Hot-reload of modules without full recompilation

### Zero-Config Experience ✅
- [x] `hx new <name>` defaults to BHC when BHC is installed and no GHC present
- [x] `hx toolchain install` installs BHC + hx-managed toolchain as one unit
  - [x] Single command from zero to building: `hx toolchain install && hx new my-app && cd my-app && hx run`
- [x] Auto-select BHC Platform snapshot matching installed BHC version
- [x] `hx doctor` checks BHC Platform compatibility, suggests upgrades

---

## v0.7.0 - Cargo Parity

The moment hx achieves full cargo-level ergonomics. A new Haskell developer
installs hx, types three commands, and has a running project with deps resolved,
tests passing, and formatting applied. No Stack. No Cabal. No GHCup.
Just `hx`.

### hx-native Package Registry
- [ ] `hx publish` to raskell.io registry (BHC-verified packages)
  - [ ] Package upload with BHC build verification
  - [ ] Automated doc generation and hosting
  - [ ] Provenance attestation (who built, when, from which source)
- [ ] `hx search` against raskell.io registry
- [ ] `hx add <package>` resolves from raskell.io first, Hackage fallback
- [ ] Yanking, ownership transfer, team-based permissions

### hx-native Resolver
- [ ] Replace hx-solver's Hackage-based resolution with BHC Platform-aware resolver
  - [ ] Resolver understands BHC profiles (packages may have profile-specific bounds)
  - [ ] Resolver understands BHC-specific flags and build configurations
  - [ ] Sub-second resolution for Platform-pinned projects
- [ ] `hx lock` produces fully reproducible builds without network access
  - [ ] Content hashes for all sources in lockfile
  - [ ] Lockfile includes BHC version, profile, platform snapshot

### Binary Installation
- [x] `hx install <package>` — build and install a binary to `~/.hx/bin/`
  - [x] Like `cargo install`: fetch, build, link, put on PATH
  - [ ] Pre-built binary cache (download instead of compile when available)
  - [x] `hx install --list` to see installed binaries
  - [x] `hx uninstall <package>`

### Workspaces 2.0
- [x] `hx workspace` commands for multi-package monorepos (`init`, `add`, `rm`, `list`, `affected`)
  - [x] `hx workspace init` — create workspace root
  - [x] `hx workspace add <path>` — add member
  - [ ] Per-member BHC profiles (server member uses `server`, lib uses `default`) — needs BHC builds to support workspaces first
- [ ] Workspace-level BHC Platform snapshot (shared across all members)
- [x] Selective rebuilds: `--changed [BASE]` on build/test/check selects affected members + reverse deps

### Build Scripts
- [ ] `[build] script = "build.hs"` — custom build logic in Haskell
  - [ ] Replaces Setup.hs with a simpler, hx-aware API
  - [ ] Access to project config, dependency info, BHC compiler flags
  - [ ] Runs in a sandbox with only declared dependencies

---

## v0.8.0 - Production Grade

### Reproducible Environments
- [ ] `hx env` — Nix-like environment pinning without Nix
  - [ ] `hx env lock` — capture exact toolchain + deps + system libs
  - [ ] `hx env shell` — enter reproducible shell
  - [ ] `hx env export` — generate Docker/OCI image from environment
- [ ] Hermetic builds: same source + same lockfile = identical binary (bit-for-bit)

### Deploy Pipeline
- [ ] `hx deploy` — build, package, and ship
  - [ ] `hx deploy docker` — generate optimized multi-stage Dockerfile
  - [ ] `hx deploy lambda` — package for AWS Lambda
  - [ ] `hx deploy fly` — deploy to Fly.io
  - [ ] `hx deploy static` — produce fully static binary (musl)

### Integrated Profiling
- [ ] `hx profile` with BHC-native profiling
  - [ ] Flame graphs from BHC runtime
  - [ ] Allocation tracking per module
  - [ ] `hx profile --web` — browser-based profiling viewer
- [ ] `hx bench` with automatic regression detection
  - [ ] Store historical benchmarks in `.hx/benchmarks/`
  - [ ] CI integration: fail build on performance regression

### Language Server
- [ ] `hx lsp` as full language server (not just HLS wrapper)
  - [ ] BHC-powered type checking (faster than GHC for BHC projects)
  - [ ] Completion, go-to-definition, find-references from BHC's API
  - [ ] Inline type hints, inlay hints
  - [ ] Code actions: add import, fill hole, case split

---

## Future — Nice to Have

These are valuable but not on the critical path to "cargo for Haskell."
They'll happen when the core story is solid.

### Cloud Builds
- [ ] Remote build execution
  - [ ] Build server protocol
  - [ ] Distributed compilation
  - [ ] Result caching and sharing
  - [ ] `hx build --remote` for offloading to build farm

### Package Ecosystem Analysis
- [ ] Dependency health scores (maintenance, test coverage, download stats)
- [ ] Supply chain audit: transitive dependency risk analysis
- [ ] License compatibility matrix across full dependency tree

### IDE Plugins
- [ ] First-party VS Code extension (hx + BHC LSP)
- [ ] IntelliJ/IDEA plugin
- [ ] Neovim plugin with native LSP configuration

### WebAssembly Story
- [ ] `hx build --target wasm` as first-class workflow
  - [ ] WASI preview 2 support
  - [ ] Browser target with JS interop
  - [ ] wasm-opt integration for size optimization

---

## Vision: cargo for Haskell

The thesis is straightforward: Rust succeeded not just because of the language,
but because `cargo` made the entire experience feel *inevitable*. You never
wonder how to start a project, add a dependency, run tests, or publish a
library. It just works.

Haskell has a better type system than almost any mainstream language. But the
tooling story — GHCup, Cabal, Stack, Hackage, Stackage, hie.yaml, HLS
configuration — is the #1 reason people bounce off it. Every year, "Haskell
setup hell" costs the ecosystem more potential users than any language feature
could attract.

**hx + BHC is the answer.** The strategic arc:

```
v0.1–v0.3:  Wrap       hx orchestrates GHC, Cabal, GHCup, HLS
v0.4–v0.5:  Tame       hx adds BHC as alternative backend, curated snapshots
v0.6–v0.7:  Own        hx builds BHC projects natively, owns the registry
v0.8+:      Complete   hx is the single tool from `install` to `deploy`
```

### What "cargo for Haskell" means concretely

| Cargo experience | hx equivalent | Status |
|------------------|---------------|--------|
| `rustup install` | `hx toolchain install` | ✅ Done |
| `cargo new` | `hx new` | ✅ Done |
| `cargo build` | `hx build` | ✅ Done |
| `cargo test` | `hx test` | ✅ Done |
| `cargo run` | `hx run` | ✅ Done |
| `cargo fmt` | `hx fmt` | ✅ Done |
| `cargo clippy` | `hx lint` | ✅ Done |
| `cargo add` | `hx add` | ✅ Done |
| `cargo publish` | `hx publish` | ✅ Done (Hackage) |
| `cargo install` | `hx install` | v0.7.0 |
| `crates.io` | raskell.io registry | v0.7.0 |
| Rust editions | BHC Platform snapshots | ✅ Foundation done |
| `cargo bench` | `hx bench` | ✅ Done |
| `cargo doc` | `hx docs` | ✅ Done |

### The end state

```bash
# A new developer's entire Haskell experience:
curl -fsSL https://hx.raskell.io/install.sh | sh   # installs hx + BHC
hx new server my-api                                 # scaffolds project
cd my-api
hx run                                               # builds and runs
hx test                                              # runs tests
hx add aeson servant                                 # adds dependencies
hx lock                                              # locks versions
hx publish                                           # ships to registry
```

No Stack. No Cabal. No GHCup. No Stackage. No hie.yaml.
Just `hx`.

---

## Command Namespace (Current)

```
hx init [--bin|--lib] [--ci] [--backend bhc|ghc]
hx build [--package <name>] [--native] [--target <triple>] [--backend bhc|ghc]
hx test [--package <name>] [--target <triple>] [--backend bhc|ghc]
hx run [--package <name>] [--target <triple>] [--backend bhc|ghc]
hx repl
hx check [--backend bhc|ghc]

hx new module|test|benchmark <name>
hx new webapp|cli|library <name> [--backend bhc|ghc]
hx new numeric <name>
hx new server <name>
hx new --template <git-url> <name>
hx bench [--package <name>]
hx docs [--open] [--deps] [--serve]
hx publish [--dry-run] [--docs]
hx changelog [--unreleased] [--output <file>]

hx fmt [--check]
hx lint [--fix]
hx doctor

hx lock [--cabal] [--snapshot <name>]
hx fetch [-j <jobs>]
hx sync
hx clean [--global]
hx cache status|prune|clean

hx add <package>
hx rm <package>

hx ide setup|status

hx toolchain status
hx toolchain list [--available] [--installed]
hx toolchain install <version> [--set] [--force] [--ghcup]
hx toolchain remove <version> [-y]
hx toolchain use <version>

hx index update [--force]
hx index status
hx index clear

hx stackage list [--lts] [--nightly] [--limit <n>]
hx stackage info <snapshot> [--packages]
hx stackage set <snapshot>

hx bhc-platform list
hx bhc-platform info <platform> [--packages]
hx bhc-platform set <platform>
hx bhc-platform update
hx bhc-platform diff <old> <new>

hx config show|path|edit|init
hx config set <key> <value>
hx config get <key>

hx nix flake|shell [--output <file>]
hx profile [--heap] [--time] <args>
hx script <file.hs> [args]
hx import --from stack|cabal
hx search <query> [--limit <n>]
hx audit [--fix] [--ignore <advisory>]
hx watch [--test] [--clear] [--debounce <ms>]

hx deps graph [--format dot|tree|list|json] [--depth <n>] [--highlight <pkgs>]
hx deps tree [--depth <n>]
hx deps list [--direct]

hx lsp [--tcp <port>]

hx plugins list
hx plugins status
hx plugins run <script> [args]

hx dist [--target <triple>] [--output <dir>] [--strip] [--completions]
hx dist formula [--version <ver>] [--output <file>]
hx dist install-script [--version <ver>] [--output <file>]

hx coverage [--html] [--open] [--output <dir>] [--threshold <pct>] [--json]

hx server start
hx server stop
hx server status
hx server restart

hx completions generate|install <shell>
hx completions manpages [--output <dir>]
hx upgrade [--check] [--target <version>]

# v0.7.0+
hx install <package>
hx uninstall <package>
hx workspace init|add
hx env lock|shell|export
hx deploy docker|lambda|fly|static
```

---

## Opinionated Defaults

| Setting | Default |
|---------|---------|
| Compiler | BHC (when installed), GHC fallback |
| Formatter | fourmolu |
| Linter | hlint |
| Build system | hx-native (BHC) / cabal fallback (GHC) |
| Store/build dirs | hx-managed |
| Lockfile format | TOML |
| Error output | short + fixes |
| Verbose output | behind `--verbose` |
| Package registry | raskell.io (Hackage fallback) |
