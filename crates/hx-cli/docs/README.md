# hx-cli

Command-line interface for the hx Haskell toolchain.

## Overview

`hx-cli` is the main entry point for hx, providing:

- Command parsing with Clap
- Command dispatch to handlers
- Template management for `hx new`
- Plugin system integration
- Telemetry initialization

## Commands

| Command | Description |
|---------|-------------|
| `hx init` | Initialize new project |
| `hx new` | Create project from template |
| `hx build` | Build the project |
| `hx test` | Run tests |
| `hx run` | Run the executable |
| `hx repl` | Start GHCi REPL |
| `hx lock` | Generate/update lockfile |
| `hx sync` | Sync dependencies |
| `hx clean` | Clean build artifacts |
| `hx doctor` | Diagnose environment |
| `hx toolchain` | Manage toolchains |
| `hx fmt` | Format code |
| `hx lint` | Lint code |
| `hx watch` | Watch and rebuild |
| `hx deps` | Dependency visualization |
| `hx coverage` | Test coverage |
| `hx dist` | Binary distribution |
| `hx lsp` | Start LSP server |

## CLI Structure

```rust
#[derive(Parser)]
#[command(name = "hx")]
#[command(about = "Haskell toolchain CLI")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress non-error output
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    Init(InitArgs),
    New(NewArgs),
    Build(BuildArgs),
    Test(TestArgs),
    Run(RunArgs),
    Repl(ReplArgs),
    Lock(LockArgs),
    Sync(SyncArgs),
    Clean(CleanArgs),
    Doctor(DoctorArgs),
    Toolchain(ToolchainCommands),
    Fmt(FmtArgs),
    Lint(LintArgs),
    Watch(WatchArgs),
    Deps(DepsArgs),
    Coverage(CoverageArgs),
    Dist(DistArgs),
    Lsp(LspArgs),
}
```

## Command Documentation

- [init](./commands/init.md) - Project initialization
- [build](./commands/build.md) - Building projects
- [test](./commands/test.md) - Running tests
- [toolchain](./commands/toolchain.md) - Toolchain management

## Global Options

### --verbose / -v

Enable verbose output:

```bash
hx build --verbose
```

Shows:
- Detailed build progress
- Cache hit/miss info
- Timing information
- Debug messages

### --quiet / -q

Suppress non-error output:

```bash
hx build --quiet
```

Only shows errors.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Usage error |
| 3 | Config error |
| 4 | Toolchain error |
| 5 | Build/test error |
| 6 | Hook failure |

## Module Structure

```
hx-cli/
├── src/
│   ├── main.rs           # Entry point
│   ├── cli.rs            # Clap definitions
│   ├── commands/         # Command implementations
│   │   ├── mod.rs
│   │   ├── build.rs
│   │   ├── test.rs
│   │   ├── run.rs
│   │   ├── lock.rs
│   │   └── ...
│   ├── templates/        # Project templates
│   │   ├── mod.rs
│   │   ├── bin.rs
│   │   └── lib.rs
│   ├── plugins.rs        # Plugin helper
│   └── styles.rs         # Output styling
└── docs/
```

## Templates

### Binary Template

```bash
hx new my-app --template bin
```

Creates:
```
my-app/
├── hx.toml
├── my-app.cabal
├── CHANGELOG.md
└── src/
    └── Main.hs
```

### Library Template

```bash
hx new my-lib --template lib
```

Creates:
```
my-lib/
├── hx.toml
├── my-lib.cabal
├── CHANGELOG.md
└── src/
    └── MyLib.hs
```

## Plugin Integration

Commands integrate with the plugin system:

```rust
// In command handler
let mut hooks = PluginHooks::from_project(&project, ghc_version);
if let Some(ref mut h) = hooks {
    h.initialize()?;
}

// Pre-hook
if let Some(ref mut h) = hooks {
    if !h.run_pre_hook(HookEvent::PreBuild, output) {
        return Ok(6);  // Hook failure
    }
}

// ... command logic ...

// Post-hook
if let Some(ref mut h) = hooks {
    h.run_post_build_hook(success, duration, warnings, errors, output);
}
```

## Main Entry Point

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI args
    let cli = Cli::parse();

    // Initialize telemetry
    hx_telemetry::init(cli.verbose)?;

    // Determine verbosity
    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else if cli.verbose {
        Verbosity::Verbose
    } else {
        Verbosity::Normal
    };

    let output = Output::with_verbosity(verbosity);

    // Dispatch command
    let exit_code = match cli.command {
        Commands::Build(args) => commands::build::run(args, &output).await?,
        Commands::Test(args) => commands::test::run(args, &output).await?,
        // ...
    };

    std::process::exit(exit_code);
}
```

## Configuration

CLI respects environment variables:

| Variable | Effect |
|----------|--------|
| `NO_COLOR` | Disable colored output |
| `CLICOLOR` | Control color output |
| `HX_HOME` | Override hx home directory (`~/.hx`) |
| `HX_CACHE_DIR`, `XDG_CACHE_HOME` | Move every cache location |
| `XDG_CONFIG_HOME`, `HX_CONFIG_FILE` | Move the global config |
| `RUST_LOG` | Set log level |

## Shell Completion

Generate shell completions:

```bash
# Bash
hx completions bash > /etc/bash_completion.d/hx

# Zsh
hx completions zsh > ~/.zfunc/_hx

# Fish
hx completions fish > ~/.config/fish/completions/hx.fish
```
