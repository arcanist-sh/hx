# Benchmarks

Numbers, not adjectives. "Blazingly fast" is a claim — here is the methodology and the measurements behind it, comparing hx against cabal across the workflows you run all day.

> **Measured with hx 0.9.2.** Honest summary: hx is faster than cabal on all four operations measured here — cold builds, CLI startup, no-op incremental rebuilds, and clean (see the per-operation notes below). stack was not re-measured for this release, so its rows are omitted rather than carried over.

## Test Environment

| Property | Value |
|----------|-------|
| **hx version** | 0.9.2 |
| **GHC version** | 9.8.2 |
| **Cabal version** | 3.12.1.0 |
| **stack** | not measured |
| **Platform** | macOS, Apple M4 (10-core) |
| **Tooling** | hyperfine 1.20.0 (8–30 runs, 1–5 warmup) |
| **Date** | 2026-09-10 |

## Results

**Test project:** a simple 3-module executable (`Main.hs`, `Lib.hs`, `Utils.hs`) depending only on `base` — the case hx's native build mode targets.

| Operation | hx (`--native`) | cabal | Result |
|-----------|-----------------|-------|--------|
| CLI startup (`--help`) | **3.2 ms** | 18.9 ms | hx **6.0× faster** |
| Cold build (clean state) | **0.52 s** | 2.89 s | hx **5.6× faster** |
| Incremental (no changes) | **3.7 ms** | 17.7 ms | hx **4.8× faster** |
| Clean | **5.2 ms** | 18.5 ms | hx **3.6× faster** |

### Where hx wins

hx is faster than cabal on all four operations measured here.

- **Cold builds (≈5.6×) and CLI startup (≈6.0×)** are inherent: the native build path invokes GHC directly, skipping cabal's package-database queries and build-plan calculation, and hx is a native Rust binary with no GHC-runtime startup cost.
- **No-op incremental rebuilds (≈4.8×)** short-circuit before any subprocess spawns — this path used to be *slower* than cabal (~78 ms spawning `ghc`/`ghc-pkg` before realizing nothing had changed) until it was fixed in [#5](https://github.com/arcanist-sh/hx/issues/5).
- **`clean` (≈3.6×)** is just `rm -rf .hx`; it no longer spins up the plugin runtime when no clean hooks are configured.
- **Honesty note:** earlier published figures (cabal "0.39 s" incremental, "180 ms" clean) were overstated. These numbers are measured fresh; cabal's no-op and clean times are dominated by its own ~18 ms process startup.
- **Absolute times drifted between runs:** the 0.9.2 cold build measured 0.52 s against 0.39 s at 0.8.0, and cabal drifted the same way (2.89 s against 2.04 s) on the same machine, so the ratio held while both tools got slower in absolute terms. Treat the ratios as the stable figure and the absolute times as machine-state dependent.

### Dependency resolution

> Measured at 0.8.0 and not re-run for 0.9.2.

`hx lock` reads a cached, pre-parsed Hackage index, so a warm lock of a project
with real dependencies is **~37 ms**. The first lock after the index changes
parses the full ~90 MB index (now evaluating each package's `.cabal`
conditionals) in **~4.9 s** — a one-time cost that is then cached for 24 hours.
Conditional evaluation, added in 0.7.14, raised that cold parse from ~4.2 s;
0.8.0 trims it back from a brief ~5.5 s regression by avoiding a per-line
allocation across the index. The warm path is unaffected.

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

> `scripts/benchmark-comparison.sh` runs the full comparison suite (startup, init, build, clean, …) and writes JSON/markdown results; the direct invocations above reproduce the headline build numbers.

## Not Re-Measured for 0.9.2

The following were measured at 0.5.0 but **have not been re-run** for 0.9.2, so their old figures were removed rather than presented as current: project init, single-file-change incremental, preprocessor overhead, and memory usage. Contributions welcome.

## Historical Results (cold build)

| Version | Date | hx `--native` | cabal | Speedup | Source |
|---------|------|---------------|-------|---------|--------|
| 0.9.2 | 2026-09-10 | 0.52 s | 2.89 s | 5.6× | measured (hyperfine, M4) |
| 0.8.0 | 2026-06-21 | 0.39 s | 2.04 s | 5.2× | measured (hyperfine, M4) |
| 0.7.7 | 2026-06-18 | 0.45 s | 2.02 s | 4.4× | measured (hyperfine, M4) |
| 0.5.0 | 2026-02-02 | 0.48 s | 2.68 s | 5.6× | unverified (not reproduced) |

## Contributing Benchmarks

We welcome benchmark contributions:

1. Run on your hardware and submit results
2. Suggest new benchmark scenarios
3. Report unexpected performance regressions

Submit results: [GitHub Issues](https://github.com/arcanist-sh/hx/issues)
