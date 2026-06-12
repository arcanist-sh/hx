# hx Performance Benchmarks

Comprehensive performance benchmarks comparing hx against cabal and stack.

> Note: these results were last measured with hx 0.5.0 and have not been
> re-run for newer releases.

For full benchmark documentation with methodology, see: https://hx.raskell.io/benchmarks/

## Test Environment

| Property | Value |
|----------|-------|
| **hx version** | 0.5.0 |
| **GHC version** | 9.8.2 |
| **Cabal version** | 3.12.1.0 |
| **Stack version** | 2.15.1 |
| **Platform** | macOS (Apple Silicon M4) |
| **Date** | 2026-02-02 |

## Executive Summary

| Operation | hx | cabal | stack | hx Speedup |
|-----------|-----|-------|-------|------------|
| CLI startup | **12ms** | 45ms | 89ms | 3.8x / 7.4x |
| Project init | **68ms** | 320ms | 2.1s | 4.7x / 31x |
| Cold build (simple) | **0.48s** | 2.68s | 3.2s | 5.6x / 6.7x |
| Incremental build | **0.05s** | 0.39s | 0.52s | 7.8x / 10.4x |
| Clean | **8ms** | 180ms | 95ms | 22x / 12x |

## CLI Startup Time

How fast does the tool respond to simple commands?

| Command | hx | cabal | stack |
|---------|-----|-------|-------|
| `--help` | 12ms | 45ms | 89ms |
| `--version` | 8ms | 38ms | 72ms |

**Why hx is faster:** hx is a native Rust binary with no runtime initialization overhead. Cabal and stack are Haskell binaries that must initialize the GHC runtime system.

## Project Initialization

| Tool | Time | Speedup vs hx |
|------|------|---------------|
| **hx init** | 68ms | — |
| cabal init | 320ms | 4.7x slower |
| stack new | 2.1s | 31x slower |

## Build Performance

**Test project:** Simple 3-module executable (Main.hs, Lib.hs, Utils.hs) with single `base` dependency.

### Cold Build (Clean State)

| Mode | Time | Speedup |
|------|------|---------|
| **hx build --native** | 0.48s | — |
| hx build (cabal backend) | 2.52s | 5.3x slower |
| cabal build | 2.68s | 5.6x slower |
| stack build | 3.2s | 6.7x slower |

### Incremental Build (No Changes)

| Mode | Time | Speedup |
|------|------|---------|
| **hx build --native** | 0.05s | — |
| hx build (cabal backend) | 0.35s | 7x slower |
| cabal build | 0.39s | 7.8x slower |
| stack build | 0.52s | 10.4x slower |

### Incremental Build (Single File Changed)

| Mode | Time | Speedup |
|------|------|---------|
| **hx build --native** | 0.31s | — |
| cabal build | 1.42s | 4.6x slower |
| stack build | 1.8s | 5.8x slower |

## Native Build Mode

hx's native build mode bypasses cabal entirely for simple projects:

1. **Direct GHC invocation** — constructs module graph and calls GHC directly
2. **No cabal overhead** — no package database queries, no build plan calculation
3. **Aggressive caching** — fingerprint-based caching with minimal I/O
4. **Parallel compilation** — native parallel builds without cabal's job scheduling

### When Native Builds Apply

| Scenario | Native Build? |
|----------|--------------|
| Single-package project | Yes |
| Only `base` dependencies | Yes |
| Multiple external dependencies | No (falls back to cabal) |
| Custom Setup.hs | No |
| C FFI / foreign libraries | No |

## Preprocessor Performance

| Preprocessor | File Type | Additional Time |
|--------------|-----------|-----------------|
| **alex** | `.x` | ~50ms |
| **happy** | `.y` | ~100ms |
| **hsc2hs** | `.hsc` | ~335ms |
| **c2hs** | `.chs` | ~280ms |

## Dependency Resolution

| Tool | Time | Notes |
|------|------|-------|
| **hx lock** | 1.2s | Native Rust solver |
| cabal freeze | 8.5s | Full constraint solving |
| stack lock | 0.8s | Stackage pre-computed |

### Solver Scaling (Synthetic)

| Packages | hx | cabal |
|----------|-----|-------|
| 10 | 5ms | 120ms |
| 20 | 18ms | 450ms |
| 50 | 85ms | 2.8s |
| 100 | 320ms | 12.5s |

## Clean Operations

| Tool | Time |
|------|------|
| **hx clean** | 8ms |
| cabal clean | 180ms |
| stack clean | 95ms |

## Memory Usage

| Operation | hx | cabal | stack |
|-----------|-----|-------|-------|
| CLI startup | 8 MB | 45 MB | 85 MB |
| Project init | 12 MB | 120 MB | 180 MB |
| Build (simple) | 45 MB | 250 MB | 320 MB |
| Dependency resolution | 80 MB | 450 MB | 180 MB |

## Running Benchmarks

### Quick Comparison

```bash
# Install hyperfine
cargo install hyperfine

# Run benchmark suite
./scripts/benchmark-comparison.sh
```

### Manual Benchmark

```bash
# Create test project
mkdir /tmp/hx-bench && cd /tmp/hx-bench

cat > hx.toml << 'EOF'
[project]
name = "hx-bench"
[toolchain]
ghc = "9.8.2"
EOF

cat > hx-bench.cabal << 'EOF'
cabal-version: 3.0
name: hx-bench
version: 0.1.0.0
build-type: Simple

executable hx-bench
    main-is: Main.hs
    other-modules: Lib, Utils
    hs-source-dirs: src
    default-language: GHC2021
    build-depends: base
EOF

mkdir src
echo 'module Main where; import Lib; import Utils; main = putStrLn (format greeting)' > src/Main.hs
echo 'module Lib (greeting) where; greeting = "Hello"' > src/Lib.hs
echo 'module Utils (format) where; format s = ">>> " ++ s ++ " <<<"' > src/Utils.hs

# Benchmark cold build
rm -rf .hx dist-newstyle
hyperfine --warmup 2 'hx build --native' 'cabal build'

# Benchmark incremental
hyperfine --warmup 2 'hx build --native' 'cabal build'
```

### Criterion Benchmarks

```bash
# Solver benchmarks
cargo bench -p hx-solver

# CLI benchmarks
cargo bench -p hx-cli

# View HTML reports
open target/criterion/report/index.html
```

## Historical Results

| Version | Date | Native Cold | Cabal Cold | Speedup |
|---------|------|-------------|------------|---------|
| 0.5.0 | 2026-02-02 | 0.48s | 2.68s | 5.6x |
| 0.4.0 | 2026-01-18 | 0.48s | 2.68s | 5.6x |
| 0.3.6 | 2026-01-17 | 0.48s | 2.68s | 5.6x |
| 0.3.0 | 2026-01-10 | 0.52s | 2.68s | 5.2x |
| 0.2.0 | 2025-12-15 | 0.61s | 2.70s | 4.4x |

## Contributing Benchmarks

We welcome benchmark contributions:

1. Run on your hardware and submit results
2. Suggest new benchmark scenarios
3. Report unexpected performance regressions

Submit results: [GitHub Issues](https://github.com/raskell-io/hx/issues)
