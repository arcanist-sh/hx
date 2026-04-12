# HX - Haskell Toolchain CLI

## Project Overview

**hx** is a fast, opinionated, batteries-included toolchain CLI for Haskell, written in Rust. The name is short, memorable, and designed to feel inevitable: `hx init`, `hx build`, `hx lock`.

## North Star

Make Haskell feel like this:
- `hx toolchain install` → you can build immediately
- `hx init` → clean project skeleton, sane defaults
- `hx build/test/run` → fast, readable output
- `hx lock/sync` → deterministic dependency resolution
- `hx doctor` → eliminates "Haskell setup hell"
- `hx fmt/lint` → default formatting/lint pipeline

## What We Optimize For

- **Fast feedback loop** - common workflows must be snappy
- **Deterministic builds** - lockfile + frozen plans
- **Boring + reliable** - no surprises in CI / enterprise
- **Excellent errors** - actionable, minimal noise
- **Pragmatic adoption** - wrap existing tools first (Astral strategy: wrap → tame → replace)

## Non-Goals (v0)

- Replacing Cabal's solver
- Replacing GHC / HLS
- Competing with Nix (must coexist)
- A new build system

## Architecture

This is a Rust workspace with the following crates:

| Crate | Responsibility |
|-------|----------------|
| `hx-cli` | Clap command parsing, dispatch to core APIs, minimal logic |
| `hx-core` | Orchestration layer, project context, shared types |
| `hx-config` | Parse `hx.toml`, project root detection, merge defaults |
| `hx-lock` | Lockfile read/write, Cabal freeze/plan conversion |
| `hx-toolchain` | Detect/install ghc, cabal, ghcup, hls |
| `hx-cabal` | Wrapper for invoking Cabal, parse build output |
| `hx-cache` | Global cache directories, artifact storage |
| `hx-doctor` | Diagnostic checks, fix action generation |
| `hx-ui` | Progress bars, spinners, consistent logging format |
| `hx-telemetry` | Tracing spans, timing, JSON logs (optional v0) |

## Key Dependencies

- **CLI**: `clap` (derive)
- **Errors**: `thiserror` + `anyhow`
- **Serialization**: `serde`, `toml`, `serde_json`
- **Process**: `tokio::process` or `std::process`
- **Async**: `tokio`
- **Logging**: `tracing`, `tracing-subscriber`
- **UI**: `indicatif` (progress), `console` (color)
- **Hashing**: `sha2`
- **Paths**: `directories`
- **HTTP**: `reqwest` (if needed)

## v0 Strategy

hx is an **orchestrator**. It calls:
- `ghcup` for toolchains
- `cabal` for build/test/run
- `haskell-language-server-wrapper` for HLS checks
- `fourmolu` / `ormolu` for formatting
- `hlint` for linting

## File Layout

```
hx/
  Cargo.toml (workspace)
  crates/
    hx-cli/
    hx-core/
    hx-config/
    hx-lock/
    hx-toolchain/
    hx-cabal/
    hx-cache/
    hx-doctor/
    hx-ui/
    hx-telemetry/
  docs/
    SPEC.md
```

## Project Files

- `hx.toml` - Project manifest (toolchain pinning, opinions)
- `hx.lock` - Lockfile (TOML format)
- `.hx/` - Project-local metadata/cache

## Build Commands

```bash
# Development
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check

# Run CLI
cargo run -p hx-cli -- --help
cargo run -p hx-cli -- --version
```

## Testing

Run all tests:
```bash
cargo test --workspace
```

Run specific crate tests:
```bash
cargo test -p hx-config
cargo test -p hx-lock
```

## Environment Setup

This project uses `mise` for environment management. Run:
```bash
mise install
```

This will install:
- Rust 1.94.1

## Links

- See `.claude/ROADMAP.md` for implementation milestones
- See `.claude/rules/` for specific coding guidelines
