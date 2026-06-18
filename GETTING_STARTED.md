# Getting Started with hx

This guide walks you through installing hx and using it to build your first Haskell project.

## Installation

### Prerequisites

You need either:
- An existing GHC/Cabal installation, OR
- hx will guide you through installing them

### Install hx

**macOS/Linux:**

```bash
curl -fsSL https://arcanist.sh/hx/install.sh | sh
```

**Windows:**

```
winget install raskell-io.hx
```

**From source (requires Rust):**

```bash
cargo install --git https://github.com/arcanist-sh/hx hx-cli
```

### Verify Installation

```bash
hx --version
hx doctor
```

The `hx doctor` command checks your system and shows what's installed and what's missing, with fix suggestions for any issues.

## Your First Project

### Create a New Project

```bash
hx init hello
cd hello
```

This creates:
```
hello/
  hx.toml           # Project configuration
  hello.cabal       # Cabal package file
  src/
    Main.hs         # Your entry point
  .gitignore
  .editorconfig
```

### Build and Run

```bash
hx build    # Compile the project
hx run      # Build and execute
```

You should see:
```
Hello, Haskell!
```

### Watch Mode

For rapid development, use watch mode to auto-rebuild on file changes:

```bash
hx watch
```

Edit `src/Main.hs` and save - hx will automatically rebuild.

## Adding Dependencies

Add packages from Hackage:

```bash
hx add text              # Add text package
hx add aeson ">=2.0"     # Add with version constraint
hx add --dev hspec       # Add as dev dependency
```

Lock dependencies for reproducible builds:

```bash
hx lock     # Generate hx.lock
hx sync     # Build with locked versions
```

## Common Workflows

### Testing

```bash
hx test                     # Run all tests
hx test --pattern "Unit"    # Filter by pattern
hx watch --test             # Auto-run tests on changes
```

### Code Quality

```bash
hx fmt          # Format code with fourmolu
hx fmt --check  # Check formatting without changes
hx lint         # Run hlint
hx lint --fix   # Apply automatic fixes
```

### Exploring Dependencies

```bash
hx tree                 # Show dependency tree
hx why text             # Show why a package is needed
hx outdated             # Check for updates
hx info aeson           # Show package details
```

### Documentation

```bash
hx docs         # Generate documentation
hx docs --open  # Generate and open in browser
```

## Project Configuration

hx uses `hx.toml` for configuration:

```toml
[project]
name = "hello"
kind = "bin"

[toolchain]
ghc = "9.8.2"

[build]
optimization = 1
warnings = true

[format]
formatter = "fourmolu"

[lint]
hlint = true
```

## Toolchain Management

### Install GHC Versions

```bash
hx toolchain list              # Show available versions
hx toolchain install 9.8.2     # Install specific version
hx toolchain use 9.8.2         # Switch to version
```

### Check Status

```bash
hx toolchain status    # Show installed versions
```

## IDE Integration

Generate configuration for your editor:

```bash
hx ide setup    # Generate hie.yaml for HLS
```

## Troubleshooting

### Check System Health

```bash
hx doctor
```

This shows:
- Missing tools with installation commands
- Version mismatches
- Configuration issues

### Clean Build

If you hit strange build issues:

```bash
hx clean        # Remove build artifacts
hx build        # Fresh build
```

### Verbose Output

For debugging:

```bash
hx build --verbose
```

## Next Steps

- Read the [full command reference](README.md#commands)
- Learn about [lockfiles and reproducibility](README.md#lockfile)
- Explore [project templates](README.md#project-management): `hx new webapp myapp`
- Set up [global configuration](README.md#global-configuration)

## Getting Help

```bash
hx --help           # General help
hx build --help     # Command-specific help
hx doctor           # Diagnose issues
```
