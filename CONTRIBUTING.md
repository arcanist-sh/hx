# Contributing to hx

Thank you for considering contributing to hx.

hx aims to make Haskell development fast, reliable, and enjoyable. It's the tool we wish existed when we started writing Haskell. Contributions that align with this mission are welcome.

This document explains how to contribute effectively.

---

## Guiding principles

Before writing code, understand what hx optimizes for:

- **Fast feedback loop** — Common workflows must be snappy
- **Deterministic builds** — Lockfiles and frozen plans, always
- **Boring + reliable** — No surprises in CI or enterprise
- **Excellent errors** — Actionable messages, minimal noise
- **Pragmatic adoption** — Wrap existing tools first, replace later

hx follows the "Astral strategy": wrap → tame → replace. We orchestrate existing Haskell tools (GHC, Cabal, GHCup, HLS) before considering replacements.

---

## AI-assisted development

We believe that software in the future will be increasingly maintained with AI assistance. hx embraces this transformation.

This is not a contradiction to our goal of building boring, stable tooling. It is an enabler. AI assistance allows us to:

- **Maintain quality** — Consistent application of coding standards and architectural rules
- **Adapt quickly** — Respond to security issues, dependency updates, and ecosystem changes
- **Stay relevant** — Participate in the fundamental transformation society is undergoing in the post-AI age
- **Lower barriers** — Make contributing accessible to those who work differently

To support AI-assisted contribution, we maintain structured context in [`.claude/CLAUDE.md`](.claude/CLAUDE.md). This includes:
- Project architecture and crate responsibilities
- Coding standards and patterns
- Error handling philosophy ("The Astral Effect")
- Testing guidelines

Whether you contribute with AI assistance or without, the standards remain the same. Code must be correct, well-tested, and aligned with hx's principles.

The tools change. The principles do not.

---

## What belongs in hx

Changes that are generally welcome:
- Performance improvements (faster builds, lower latency)
- Better error messages with actionable fixes
- New commands that improve developer workflows
- Improved toolchain detection and management
- Cross-platform compatibility fixes
- Tests, benchmarks, and documentation
- Bug fixes with regression tests

Changes that require discussion:
- New configuration options (we prefer convention over configuration)
- New external tool integrations
- Changes to lockfile format
- Anything that increases startup time

---

## What does *not* belong in hx

hx intentionally stays focused on the Haskell development workflow.

The following do **not** belong in hx:
- Language features (that's GHC's job)
- Package hosting (that's Hackage's job)
- Complex build system logic (we wrap Cabal)
- IDE features beyond HLS orchestration
- Platform-specific features that can't work everywhere

When in doubt, ask: "Does this make `hx build` faster or more reliable?"

---

## Code standards

### Rust

- Edition 2024, Rust 1.96.0+
- `cargo fmt` before every commit
- `cargo clippy -- -D warnings` must pass
- No `.unwrap()` in library code — use proper error handling

### Crate organization

| Crate | Responsibility |
|-------|----------------|
| `hx-cli` | Thin command dispatch, no business logic |
| `hx-core` | Shared types and orchestration |
| `hx-config` | Parse `hx.toml`, detect project root |
| `hx-lock` | Lockfile read/write |
| `hx-toolchain` | GHC/Cabal/GHCup/HLS detection and installation |
| `hx-cabal` | Cabal invocation and output parsing |
| `hx-solver` | Dependency resolution |
| `hx-doctor` | Diagnostics and fix suggestions |

Each crate should have a single, clear responsibility.

### Error handling

Errors must be actionable. Every error should include:
1. What failed (short summary)
2. Why it failed (context)
3. How to fix it (command or instruction)

```
error: GHC version mismatch
  expected: 9.8.2 (from hx.toml)
  found: 9.6.4

fix: Run `hx toolchain install 9.8.2`
```

See [`.claude/rules/errors.md`](.claude/rules/errors.md) for the full error UX guidelines.

---

## Testing expectations

Any non-trivial change should include:
- Unit tests for new functionality
- Integration tests for new commands
- Regression tests for bug fixes
- Documentation updates if behavior changes

### Running tests

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p hx-config

# Integration tests
cargo test -p hx-cli --test e2e_workflows

# Benchmarks
cargo bench -p hx-cli
cargo bench -p hx-solver
```

Coverage targets:
- `hx-config`, `hx-lock`: 80%+
- `hx-solver`, `hx-cabal`, `hx-toolchain`: 70%+
- `hx-cli`: 60%+

---

## How to propose changes

### For bug fixes

1. Open an issue describing the bug
2. Include reproduction steps
3. Submit a PR with the fix and a regression test

### For new features

1. Open an issue describing the problem you're solving
2. Explain why it belongs in hx (not in Cabal, GHCup, etc.)
3. Wait for feedback before implementing
4. Submit a PR with tests and documentation

Discussion is preferred over premature implementation.

### For documentation

Documentation improvements can be submitted directly as PRs. No issue required.

---

## Development setup

```bash
# Clone the repository
git clone https://github.com/raskell-io/hx.git
cd hx

# Install Rust (if needed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Or use mise
mise install

# Build
cargo build

# Run tests
cargo test --workspace

# Run the CLI
cargo run -p hx-cli -- --help
```

---

## Pull request checklist

Before submitting:

- [ ] Code compiles without warnings (`cargo build`)
- [ ] All tests pass (`cargo test --workspace`)
- [ ] Code is formatted (`cargo fmt`)
- [ ] Clippy is happy (`cargo clippy -- -D warnings`)
- [ ] New functionality has tests
- [ ] Documentation is updated if needed
- [ ] Commit messages are clear and descriptive

---

## Code of conduct

Be respectful, precise, and patient.

hx values thoughtful contributions over fast ones. Take time to understand the codebase, ask questions, and consider how your change affects the whole.

We're building tools for the long term. There's no rush.
