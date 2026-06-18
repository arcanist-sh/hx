# Cross-Compilation

Ship a Haskell binary for any target from the machine you already have. One flag — `hx build --target` — and hx handles the toolchain wiring, so building for Linux ARM from your Mac is just another build.

## Usage

Use the `--target` flag to specify the target triple:

```bash
hx build --target aarch64-linux-gnu
hx build --target x86_64-unknown-linux-musl
hx build --target aarch64-apple-darwin
```

The `--target` flag is also available for test and run commands:

```bash
hx test --target aarch64-linux-gnu
hx run --target x86_64-unknown-linux-musl
```

## Common Target Triples

| Target | Description |
|--------|-------------|
| `x86_64-linux-gnu` | 64-bit Linux (glibc) |
| `aarch64-linux-gnu` | ARM64 Linux (glibc) |
| `x86_64-unknown-linux-musl` | 64-bit Linux (musl, static) |
| `aarch64-unknown-linux-musl` | ARM64 Linux (musl, static) |
| `x86_64-apple-darwin` | 64-bit macOS (Intel) |
| `aarch64-apple-darwin` | ARM64 macOS (Apple Silicon) |
| `x86_64-w64-mingw32` | 64-bit Windows |

## Requirements

Cross-compilation requires:

1. **Cross-compiler GHC**: A GHC installation configured to target the desired platform
2. **Target toolchain**: C compiler and linker for the target platform
3. **Target libraries**: System libraries for the target platform

### Installing a Cross-Compiler

GHC cross-compilers are not as common as native compilers. Options include:

1. **Build from source**: Configure GHC with `--target=<triple>`
2. **Pre-built binaries**: Check [ghcup](https://www.haskell.org/ghcup/) for available cross-compilers
3. **Docker/containers**: Use a container with the cross-compilation toolchain

### Example: Cross-compile from macOS to Linux

```bash
# Install the cross-compiler toolchain (varies by platform)
# Then build:
hx build --target x86_64-linux-gnu
```

## Native Build Support

The `--native` flag also supports cross-compilation:

```bash
hx build --native --target aarch64-linux-gnu
```

This passes the `-target=<triple>` flag directly to GHC.

## Limitations

- Cross-compilation support depends on having a properly configured cross-compiler
- Some packages with C dependencies may require additional setup
- Running cross-compiled binaries requires the target platform or an emulator

## Troubleshooting

### "ghc: unrecognized flag: -target"

Your GHC installation may not support cross-compilation. You need a GHC built as a cross-compiler.

### Missing target libraries

Ensure you have the target platform's libraries installed. For Linux targets, this typically means:

```bash
# Debian/Ubuntu
apt-get install gcc-aarch64-linux-gnu

# macOS (via Homebrew)
brew install aarch64-linux-gnu-gcc
```

### Cabal cross-compilation issues

If using cabal-based builds, ensure your cabal version supports cross-compilation (3.6+).
