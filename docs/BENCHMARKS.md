# Benchmarks

Numbers, not adjectives. "Blazingly fast" is a claim — here is the methodology and the measurements behind it, comparing hx against cabal across the workflows you run all day.

> **Measured with hx 0.7.6.** Honest summary: hx's native build path is faster than cabal on **cold builds**, **CLI startup**, and **no-op incremental rebuilds**; cabal is still faster on `clean` (see below). stack was not re-measured for this release, so its rows are omitted rather than carried over.

## Test Environment

| Property | Value |
|----------|-------|
| **hx version** | 0.7.6 |
| **GHC version** | 9.8.2 |
| **Cabal version** | 3.12.1.0 |
| **stack** | not measured |
| **Platform** | macOS, Apple M4 (10-core) |
| **Tooling** | hyperfine 1.20.0 (8–20 runs, 2–3 warmup) |
| **Date** | 2026-06-18 |

## Results

**Test project:** a simple 3-module executable (`Main.hs`, `Lib.hs`, `Utils.hs`) depending only on `base` — the case hx's native build mode targets.

| Operation | hx (`--native`) | cabal | Result |
|-----------|-----------------|-------|--------|
| CLI startup (`--help`) | **4.0 ms** | 18.0 ms | hx **4.5× faster** |
| Cold build (clean state) | **0.45 s** | 2.02 s | hx **4.4× faster** |
| Incremental (no changes) | **3.3 ms** | 18.2 ms | hx **5.4× faster** |
| Clean | 31.9 ms | **17.6 ms** | cabal 1.8× faster |

### Where hx wins — and where it doesn't

- **Cold builds (≈4.4×), CLI startup (≈4.5×), and no-op incremental rebuilds (≈5.4×) are hx's real, repeatable advantages.** The native build path constructs the module graph and invokes GHC directly, skipping cabal's package-database queries and build-plan calculation; hx is a native Rust binary with no GHC-runtime startup cost; and a no-op rebuild short-circuits before any subprocess spawns.
- **The no-op path was fixed in [#5](https://github.com/arcanist-sh/hx/issues/5).** It previously spent ~74 ms spawning `ghc`/`ghc-pkg` (toolchain + package-DB resolution) before discovering nothing had changed — 78.6 ms total, slower than cabal. A subprocess-free up-to-date check now short-circuits no-op builds, dropping that to ~3.3 ms.
- **`clean` is the one operation where cabal still wins** (hx 31.9 ms vs cabal 17.6 ms). Earlier published figures here (cabal "0.39 s" incremental, "180 ms" clean) appear to have been overstated, which inflated the old speedups.

## Native Build Mode

hx's native build mode bypasses cabal entirely for simple projects:

1. **Direct GHC invocation** — constructs the module graph and calls GHC directly
2. **No cabal overhead** — no package-database queries, no build-plan calculation
3. **Fingerprint caching** — content-hash-based incremental decisions
4. **Parallel compilation** — native parallel builds

### When Native Builds Apply

| Scenario | Native build? |
|----------|---------------|
| Single-package project | Yes |
| Only `base` dependencies | Yes |
| Multiple external dependencies | No (falls back to cabal) |
| Custom `Setup.hs` | No |
| C FFI / foreign libraries | No |

## Reproducing These Numbers

```bash
# install hyperfine
cargo install hyperfine

# create the 3-module test project
mkdir /tmp/hx-bench && cd /tmp/hx-bench
hx init bench --name bench
cat > bench.cabal << 'EOF'
cabal-version: 3.0
name: bench
version: 0.1.0.0
build-type: Simple
executable bench
    main-is: Main.hs
    other-modules: Lib, Utils
    hs-source-dirs: src
    default-language: GHC2021
    build-depends: base
EOF
printf 'module Lib (greeting) where\ngreeting = "Hello"\n' > src/Lib.hs
printf 'module Utils (format) where\nformat s = ">>> " ++ s ++ " <<<"\n' > src/Utils.hs
printf 'module Main where\nimport Lib\nimport Utils\nmain = putStrLn (format greeting)\n' > src/Main.hs

# cold build (clean before each run)
hyperfine --warmup 1 --prepare 'rm -rf .hx dist-newstyle' 'hx build --native' 'cabal build'

# incremental (no changes) — warm up first
hx build --native && cabal build
hyperfine --warmup 3 'hx build --native' 'cabal build'
```

> Note: `scripts/benchmark-comparison.sh` mis-quotes commands passed to hyperfine and currently fails; prefer the direct invocations above until it's fixed.

## Not Re-Measured for 0.7.6

The following were measured at 0.5.0 but **have not been re-run** for 0.7.6, so their old figures were removed rather than presented as current: project init, single-file-change incremental, preprocessor overhead, dependency-resolution/solver scaling, and memory usage. Contributions welcome.

## Historical Results (cold build)

| Version | Date | hx `--native` | cabal | Speedup | Source |
|---------|------|---------------|-------|---------|--------|
| 0.7.6 | 2026-06-18 | 0.45 s | 2.02 s | 4.4× | measured (hyperfine, M4) |
| 0.5.0 | 2026-02-02 | 0.48 s | 2.68 s | 5.6× | unverified (not reproduced) |

## Contributing Benchmarks

We welcome benchmark contributions:

1. Run on your hardware and submit results
2. Suggest new benchmark scenarios
3. Report unexpected performance regressions

Submit results: [GitHub Issues](https://github.com/arcanist-sh/hx/issues)
