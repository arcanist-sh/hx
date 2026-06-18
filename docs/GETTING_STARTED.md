# Getting Started with hx Development

Welcome to the hx codebase. This document will help you understand the project, its philosophy, and how to navigate the code.

---

## What is hx?

**hx** is a fast, opinionated, batteries-included toolchain CLI for Haskell, written in Rust.

The name is short, memorable, and designed to feel inevitable:

```bash
hx init        # Create a new project
hx build       # Build it
hx test        # Test it
hx run         # Run it
hx doctor      # Diagnose issues
```

hx aims to eliminate "Haskell setup hell" by providing a single tool that handles toolchains, dependencies, building, formatting, linting, and IDE integration.

> **Note**: This document is for **developers contributing to hx**. If you're looking for user documentation on how to use hx, visit **[docs.arcanist.sh/hx/docs](https://docs.arcanist.sh/hx/docs/)**.

---

## Philosophy

### North Star

Make Haskell feel like this:
- `hx toolchain install` → you can build immediately
- `hx init` → clean project skeleton, sane defaults
- `hx build/test/run` → fast, readable output
- `hx lock/sync` → deterministic dependency resolution
- `hx doctor` → eliminates setup problems
- `hx fmt/lint` → default code quality pipeline

### What We Optimize For

| Priority | Description |
|----------|-------------|
| **Fast feedback** | Common workflows must be snappy |
| **Deterministic** | Lockfiles and frozen plans, always |
| **Boring + reliable** | No surprises in CI or enterprise |
| **Excellent errors** | Actionable messages with fix suggestions |
| **Pragmatic adoption** | Wrap existing tools first |

### The Astral Strategy

hx follows a "wrap → tame → replace" approach, inspired by [Astral](https://astral.sh):

1. **Wrap** — Orchestrate existing tools (GHC, Cabal, GHCup, HLS)
2. **Tame** — Provide better UX, caching, and error messages
3. **Replace** — Only replace components when we can do significantly better

This means hx is currently an orchestrator. Under the hood, it calls:
- `ghcup` for toolchain installation
- `cabal` for building and dependency management
- `ghc` directly for native builds (simple projects)
- `bhc` (Basel Haskell Compiler) as an alternative compiler backend
- `fourmolu`/`ormolu` for formatting
- `hlint` for linting
- `haskell-language-server` for IDE features

---

## Architecture Overview

hx is a Rust workspace with 16 crates, each with a single responsibility:

```
hx/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── hx-cli/             # Command-line interface
│   ├── hx-core/            # Shared types and orchestration
│   ├── hx-config/          # Configuration parsing
│   ├── hx-lock/            # Lockfile handling
│   ├── hx-toolchain/       # GHC/Cabal/HLS management
│   ├── hx-cabal/           # Cabal invocation and parsing
│   ├── hx-solver/          # Dependency resolution
│   ├── hx-cache/           # Artifact caching
│   ├── hx-doctor/          # Diagnostics
│   ├── hx-ui/              # Progress bars, colors
│   ├── hx-lsp/             # Language server integration
│   ├── hx-plugins/         # Steel plugin system
│   ├── hx-compiler/        # Compiler backend abstraction
│   ├── hx-bhc/             # BHC compiler backend
│   ├── hx-telemetry/       # Tracing and metrics
│   └── hx-warnings/        # Warning deduplication
├── docs/                   # Project-level documentation
└── .claude/                # AI-assisted development context
```

---

## Crate Map

Each crate has its own `docs/` directory with detailed documentation.

### Core Crates

| Crate | Responsibility | Documentation |
|-------|----------------|---------------|
| **hx-cli** | Clap command parsing, minimal dispatch logic | [`crates/hx-cli/docs/`](../crates/hx-cli/docs/README.md) |
| **hx-core** | Shared types, error codes, command traits | [`crates/hx-core/docs/`](../crates/hx-core/docs/README.md) |
| **hx-config** | Parse `hx.toml`, detect project root, workspaces | [`crates/hx-config/docs/`](../crates/hx-config/docs/README.md) |

### Build System

| Crate | Responsibility | Documentation |
|-------|----------------|---------------|
| **hx-cabal** | Invoke Cabal, parse output, native builds | [`crates/hx-cabal/docs/`](../crates/hx-cabal/docs/README.md) |
| **hx-solver** | Dependency resolution, Hackage index, module graphs | [`crates/hx-solver/docs/`](../crates/hx-solver/docs/README.md) |
| **hx-lock** | Lockfile read/write, fingerprinting | [`crates/hx-lock/docs/`](../crates/hx-lock/docs/README.md) |
| **hx-cache** | Build artifacts, fingerprints, global cache | [`crates/hx-cache/docs/`](../crates/hx-cache/docs/README.md) |

### Toolchain Management

| Crate | Responsibility | Documentation |
|-------|----------------|---------------|
| **hx-toolchain** | Detect/install GHC, Cabal, GHCup, HLS | [`crates/hx-toolchain/docs/`](../crates/hx-toolchain/docs/README.md) |
| **hx-compiler** | Compiler backend abstraction trait | [`crates/hx-compiler/`](../crates/hx-compiler/) |
| **hx-bhc** | BHC (Basel Haskell Compiler) backend | [`crates/hx-bhc/`](../crates/hx-bhc/) |

### Developer Experience

| Crate | Responsibility | Documentation |
|-------|----------------|---------------|
| **hx-doctor** | Environment diagnostics, fix suggestions | [`crates/hx-doctor/docs/`](../crates/hx-doctor/docs/README.md) |
| **hx-ui** | Progress bars, spinners, colors, styling | [`crates/hx-ui/docs/`](../crates/hx-ui/docs/README.md) |
| **hx-lsp** | HLS process management, diagnostics | [`crates/hx-lsp/docs/`](../crates/hx-lsp/docs/README.md) |
| **hx-warnings** | Warning deduplication, error chains | [`crates/hx-warnings/docs/`](../crates/hx-warnings/docs/README.md) |

### Extensibility

| Crate | Responsibility | Documentation |
|-------|----------------|---------------|
| **hx-plugins** | Steel (Scheme) plugin runtime | [`crates/hx-plugins/docs/`](../crates/hx-plugins/docs/README.md) |
| **hx-telemetry** | Tracing spans, timing, metrics | [`crates/hx-telemetry/docs/`](../crates/hx-telemetry/docs/README.md) |

---

## Key Concepts

### Project Files

| File | Purpose |
|------|---------|
| `hx.toml` | Project manifest (toolchain, dependencies, settings) |
| `hx.lock` | Lockfile (pinned versions, fingerprints) |
| `.hx/` | Project-local cache and metadata |
| `*.cabal` | Cabal package description (hx reads and writes these) |

### Data Flow

```
User Command
     │
     ▼
┌─────────┐     ┌───────────┐     ┌──────────────┐
│ hx-cli  │ ──▶ │ hx-config │ ──▶ │ hx-toolchain │
└─────────┘     └───────────┘     └──────────────┘
     │                                    │
     ▼                                    ▼
┌─────────┐     ┌───────────┐     ┌──────────────┐
│ hx-core │ ──▶ │ hx-solver │ ──▶ │  hx-cabal    │
└─────────┘     └───────────┘     └──────────────┘
     │               │                    │
     ▼               ▼                    ▼
┌─────────┐   ┌────────────┐     ┌──────────────┐
│  hx-ui  │   │  hx-bhc    │     │   GHC/HLS    │
└─────────┘   └────────────┘     └──────────────┘
                    │
                    ▼
              ┌────────────┐
              │    BHC      │
              └────────────┘
```

When the compiler backend is set to BHC, `hx-bhc` handles build/test/run instead of `hx-cabal`. The `hx-solver` also integrates BHC Platform snapshots for curated package sets.

### Error Philosophy ("The Astral Effect")

Every error must be actionable:

```
error: GHC version mismatch
  expected: 9.8.2 (from hx.toml)
  found: 9.6.4

fix: Run `hx toolchain install 9.8.2`
```

See [`.claude/rules/errors.md`](../.claude/rules/errors.md) for the full error UX guidelines.

---

## Development Setup

### Prerequisites

- Rust 1.94.1+ (edition 2024)
- Git

### Quick Start

```bash
# Clone
git clone https://github.com/arcanist-sh/hx.git
cd hx

# Build
cargo build

# Run tests
cargo test --workspace

# Run the CLI
cargo run -p hx-cli -- --help

# Run a specific command
cargo run -p hx-cli -- doctor
```

### Using mise (optional)

```bash
mise install    # Installs correct Rust version
```

### Code Quality

```bash
# Format code
cargo fmt

# Lint
cargo clippy -- -D warnings

# Check all targets
cargo check --workspace --all-targets
```

---

## Common Development Tasks

### Adding a New Command

1. Add command struct in `crates/hx-cli/src/commands/`
2. Register in `crates/hx-cli/src/cli.rs`
3. Implement logic in appropriate crate (not hx-cli)
4. Add tests in the implementing crate
5. Add integration test in `crates/hx-cli/tests/`

### Adding a New Configuration Option

1. Add field to manifest struct in `crates/hx-config/src/manifest.rs`
2. Update defaults in `crates/hx-config/src/defaults.rs`
3. Document in `crates/hx-config/docs/manifest.md`
4. Add tests for parsing

### Improving Error Messages

1. Find the error type in the relevant crate
2. Add context using `thiserror`'s `#[error]` attribute
3. Add fix suggestions to the `Diagnostic` struct
4. Test the error output

---

## Testing

### Test Pyramid

| Level | Framework | Location |
|-------|-----------|----------|
| Unit | `#[test]` | `crates/*/src/` |
| Integration | `assert_cmd` | `crates/*/tests/` |
| Benchmarks | `criterion` | `crates/*/benches/` |

### Running Tests

```bash
# All tests
cargo test --workspace

# Single crate
cargo test -p hx-config

# Integration tests only
cargo test -p hx-cli --test e2e_workflows

# With output
cargo test -- --nocapture
```

### Running Benchmarks

```bash
# CLI benchmarks
cargo bench -p hx-cli

# Solver benchmarks
cargo bench -p hx-solver

# View HTML reports
open target/criterion/report/index.html
```

See [BENCHMARKS.md](./BENCHMARKS.md) for performance data.

---

## Documentation Index

### User Documentation

For end-user documentation on how to use hx, visit the official docs site:

| Resource | URL |
|----------|-----|
| **Documentation** | [docs.arcanist.sh/hx/docs](https://docs.arcanist.sh/hx/docs/) |
| **Features** | [arcanist.sh/hx/features](https://arcanist.sh/hx/features/) |
| **Benchmarks** | [arcanist.sh/hx/benchmarks](https://arcanist.sh/hx/benchmarks/) |

### Project-Level (Developer)

| Document | Description |
|----------|-------------|
| [README.md](../README.md) | Project overview and quick start |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | How to contribute |
| [CHANGELOG.md](../CHANGELOG.md) | Version history |
| [docs/BENCHMARKS.md](./BENCHMARKS.md) | Performance benchmarks |
| [docs/CROSS_COMPILATION.md](./CROSS_COMPILATION.md) | Cross-compilation guide |
| [docs/STACKAGE.md](./STACKAGE.md) | Stackage integration |
| [docs/BHC_PLATFORM.md](./BHC_PLATFORM.md) | BHC Platform snapshots |

### AI-Assisted Development

| Document | Description |
|----------|-------------|
| [.claude/CLAUDE.md](../.claude/CLAUDE.md) | Project context for AI assistants |
| [.claude/ROADMAP.md](../.claude/ROADMAP.md) | Feature roadmap |
| [.claude/rules/](../.claude/rules/) | Coding standards |

### Crate Documentation

Each crate has a `docs/README.md` with:
- Overview and responsibilities
- Module descriptions
- Quick start examples
- Links to detailed documentation

Browse them at `crates/<name>/docs/README.md`.

---

## Where to Start

### If you want to...

| Goal | Start here |
|------|------------|
| Understand the CLI | `crates/hx-cli/docs/` |
| Work on configuration | `crates/hx-config/docs/` |
| Improve build performance | `crates/hx-cabal/docs/native.md` |
| Add toolchain support | `crates/hx-toolchain/docs/` |
| Work on BHC backend | `crates/hx-bhc/src/lib.rs`, `crates/hx-compiler/src/lib.rs` |
| Add BHC Platform snapshots | `crates/hx-solver/src/bhc_platform.rs` |
| Add project templates | `crates/hx-cli/src/templates/` |
| Improve error messages | `.claude/rules/errors.md` |
| Add a new command | `crates/hx-cli/src/commands/` |

### Recommended Reading Order

1. This document (you're here)
2. [.claude/CLAUDE.md](../.claude/CLAUDE.md) — Project philosophy
3. [.claude/rules/errors.md](../.claude/rules/errors.md) — Error UX guidelines
4. [CONTRIBUTING.md](../CONTRIBUTING.md) — Contribution process
5. Crate docs for the area you're working on

---

## Getting Help

- **Issues**: [GitHub Issues](https://github.com/arcanist-sh/hx/issues)
- **Discussions**: [GitHub Discussions](https://github.com/arcanist-sh/hx/discussions)
- **Discord**: [raskell.io Discord](https://discord.gg/raskell)

Welcome to hx. We're glad you're here.
