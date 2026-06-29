//! CLI argument parsing.

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use hx_core::EnvVars;

use crate::styles::STYLES;

/// hx - Haskell Toolchain CLI
#[derive(Parser, Debug)]
#[command(name = "hx")]
#[command(author, version, about = "A fast, opinionated Haskell toolchain CLI")]
#[command(long_about = None)]
#[command(propagate_version = true)]
#[command(styles = STYLES)]
#[command(after_help = "Use `hx help <command>` for more information about a command.")]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Global arguments available to all commands.
#[derive(Args, Debug)]
pub struct GlobalArgs {
    /// Enable verbose output
    #[arg(short, long, global = true, env = EnvVars::HX_VERBOSE)]
    pub verbose: bool,

    /// Suppress output (use twice for complete silence)
    #[arg(short, long, global = true, action = clap::ArgAction::Count, env = EnvVars::HX_QUIET)]
    pub quiet: u8,

    /// Disable colored output
    #[arg(long, global = true, env = EnvVars::HX_NO_COLOR)]
    pub no_color: bool,

    /// Path to configuration file
    #[arg(long, global = true, env = EnvVars::HX_CONFIG_FILE)]
    pub config_file: Option<std::path::PathBuf>,

    /// Disable automatic toolchain installation
    #[arg(long, global = true, env = "HX_NO_AUTO_INSTALL")]
    pub no_auto_install: bool,

    /// Always install missing toolchain components without prompting
    #[arg(long, global = true, env = "HX_AUTO_INSTALL")]
    pub auto_install: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize a new Haskell project
    ///
    /// Examples:
    ///   hx init           # Initialize in current directory
    ///   hx init myapp     # Create myapp/ directory and initialize
    ///   hx init --lib     # Create a library instead of executable
    Init {
        /// Directory to initialize (default: current directory)
        path: Option<String>,

        /// Create a library project (default: executable)
        #[arg(long)]
        lib: bool,

        /// Override project name (default: directory name)
        #[arg(long)]
        name: Option<String>,

        /// Generate GitHub Actions CI workflow
        #[arg(long)]
        ci: bool,

        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Build the project
    Build {
        /// Build in release mode with optimizations
        #[arg(long)]
        release: bool,

        /// Number of parallel jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Target triple
        #[arg(long)]
        target: Option<String>,

        /// Build specific package (in workspace)
        #[arg(short, long)]
        package: Option<String>,

        /// Use native build (experimental)
        #[arg(long)]
        native: bool,

        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Run tests
    Test {
        /// Test pattern to match
        #[arg(short, long)]
        pattern: Option<String>,

        /// Test specific package (in workspace)
        #[arg(long)]
        package: Option<String>,

        /// Target triple for cross-compilation
        #[arg(long)]
        target: Option<String>,

        /// Use native build (experimental)
        #[arg(long)]
        native: bool,

        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Run the project
    Run {
        /// Arguments to pass to the program
        #[arg(last = true)]
        args: Vec<String>,

        /// Run specific package (in workspace)
        #[arg(short, long)]
        package: Option<String>,

        /// Target triple for cross-compilation
        #[arg(long)]
        target: Option<String>,

        /// Use native build (experimental)
        #[arg(long)]
        native: bool,

        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Start a REPL
    Repl {
        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Type-check the project (fast build)
    Check {
        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Format source code
    Fmt {
        /// Check formatting without making changes
        #[arg(long)]
        check: bool,
    },

    /// Run linter
    Lint {
        /// Apply automatic fixes
        #[arg(long)]
        fix: bool,
    },

    /// Diagnose toolchain and project issues
    Doctor,

    /// Create or update the lockfile
    Lock {
        /// Use cabal freeze instead of native solver
        #[arg(long)]
        cabal: bool,

        /// Update specific packages (or all if none specified)
        #[arg(long, num_args = 0..)]
        update: Option<Vec<String>>,

        /// Set Stackage snapshot before locking (e.g., lts-22.28, nightly)
        #[arg(long)]
        snapshot: Option<String>,
    },

    /// Pre-fetch dependencies in parallel
    Fetch {
        /// Number of parallel download jobs
        #[arg(short, long)]
        jobs: Option<usize>,
    },

    /// Build with locked dependencies
    Sync {
        /// Force sync even if lock is outdated
        #[arg(long)]
        force: bool,
    },

    /// Clean build artifacts
    Clean {
        /// Clean global cache instead of project cache
        #[arg(long)]
        global: bool,
    },

    /// Manage the build cache
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },

    /// Manage toolchain versions
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },

    /// Add a dependency
    Add {
        /// Package name
        package: String,

        /// Version constraint (e.g., ">=1.0", "^2.0", "==1.2.3")
        constraint: Option<String>,

        /// Add as a development dependency
        #[arg(long)]
        dev: bool,
    },

    /// Remove a dependency
    Rm {
        /// Package name
        package: String,
    },

    /// Show why a package is a dependency
    Why {
        /// Package name to trace
        package: String,
    },

    /// Show package information from Hackage
    Info {
        /// Package name
        package: String,

        /// Show all available versions
        #[arg(long)]
        versions: bool,
    },

    /// Check for outdated dependencies
    Outdated {
        /// Only show direct dependencies
        #[arg(long)]
        direct: bool,

        /// Show all updates (including pre-release)
        #[arg(long)]
        all: bool,
    },

    /// Update dependencies to latest versions
    Update {
        /// Package(s) to update (updates all if not specified)
        packages: Vec<String>,

        /// Only update direct dependencies
        #[arg(long)]
        direct: bool,

        /// Show what would be updated without making changes
        #[arg(long)]
        dry_run: bool,

        /// Allow major version updates
        #[arg(long)]
        major: bool,
    },

    /// Show dependency tree (alias for `deps tree`)
    Tree {
        /// Maximum depth to display (0 = unlimited)
        #[arg(long, short, default_value = "0")]
        depth: usize,

        /// Include dev dependencies
        #[arg(long)]
        dev: bool,
    },

    /// List all dependencies (alias for `deps list`)
    List {
        /// Include dev dependencies
        #[arg(long)]
        dev: bool,

        /// Show only direct dependencies
        #[arg(long)]
        direct: bool,
    },

    /// Manage shell completions
    Completions {
        #[command(subcommand)]
        command: CompletionsCommands,
    },

    /// Upgrade hx to the latest version
    Upgrade {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,

        /// Upgrade to a specific version
        #[arg(long = "target")]
        target_version: Option<String>,
    },

    /// Create a new module or file from a template
    New {
        #[command(subcommand)]
        command: NewCommands,
    },

    /// Run benchmarks
    Bench {
        /// Benchmark filter pattern
        #[arg(short, long)]
        filter: Option<String>,

        /// Save baseline for comparison
        #[arg(long)]
        save_baseline: Option<String>,

        /// Compare against baseline
        #[arg(long)]
        baseline: Option<String>,

        /// Benchmark specific package (in workspace)
        #[arg(short, long)]
        package: Option<String>,

        /// Output results as JSON
        #[arg(long)]
        json: bool,

        /// Regression threshold percentage (fail if regression exceeds this)
        #[arg(long, value_name = "PERCENT")]
        regression_threshold: Option<f64>,

        /// Save results to history for tracking over time
        #[arg(long)]
        save_history: bool,
    },

    /// Publish package to Hackage
    Publish {
        /// Perform a dry run without uploading
        #[arg(long)]
        dry_run: bool,

        /// Hackage username
        #[arg(long, env = "HACKAGE_USERNAME")]
        username: Option<String>,

        /// Hackage password
        #[arg(long, env = "HACKAGE_PASSWORD")]
        password: Option<String>,

        /// Publish documentation
        #[arg(long)]
        docs: bool,
    },

    /// Configure IDE integration (HLS, hie.yaml)
    Ide {
        #[command(subcommand)]
        command: IdeCommands,
    },

    /// Manage the Hackage package index
    Index {
        #[command(subcommand)]
        command: IndexCommands,
    },

    /// Manage Stackage snapshots
    Stackage {
        #[command(subcommand)]
        command: StackageCommands,
    },

    /// Generate documentation
    Docs {
        /// Open in browser after generating
        #[arg(long)]
        open: bool,

        /// Include dependency documentation
        #[arg(long)]
        deps: bool,

        /// Serve documentation locally
        #[arg(long)]
        serve: bool,

        /// Port for local server (default: 8080)
        #[arg(long, default_value = "8080")]
        port: u16,
    },

    /// Generate or update CHANGELOG.md from git history
    Changelog {
        /// Only show unreleased changes (since last tag)
        #[arg(long)]
        unreleased: bool,

        /// Output file (default: CHANGELOG.md)
        #[arg(long, short)]
        output: Option<String>,

        /// Include all commits (not just conventional commits)
        #[arg(long)]
        all: bool,

        /// Show changelog preview without writing
        #[arg(long)]
        preview: bool,
    },

    /// Nix/Flake integration
    Nix {
        #[command(subcommand)]
        command: NixCommands,
    },

    /// Run with profiling enabled
    Profile {
        /// Enable heap profiling
        #[arg(long)]
        heap: bool,

        /// Enable time profiling (default)
        #[arg(long)]
        time: bool,

        /// Profiling detail level (default: 2)
        #[arg(long, default_value = "2")]
        detail: u8,

        /// Arguments to pass to the program
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// Run a single-file Haskell script
    Script {
        /// Path to the Haskell script
        file: String,

        /// Arguments to pass to the script
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// Import project from another build tool
    Import {
        /// Source format to import from
        #[arg(long, value_enum)]
        from: ImportSource,

        /// Path to source configuration (default: auto-detect)
        #[arg(long)]
        path: Option<String>,
    },

    /// Search for packages on Hackage
    Search {
        /// Search query
        query: String,

        /// Maximum results to show
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Show detailed package info
        #[arg(long)]
        detailed: bool,
    },

    /// Audit dependencies for vulnerabilities and issues
    Audit {
        /// Attempt to fix issues automatically
        #[arg(long)]
        fix: bool,

        /// Ignore specific advisories
        #[arg(long)]
        ignore: Vec<String>,

        /// Check for outdated dependencies
        #[arg(long)]
        outdated: bool,

        /// Check license compatibility
        #[arg(long)]
        licenses: bool,
    },

    /// Watch for file changes and auto-rebuild
    Watch {
        /// Run tests instead of build on change
        #[arg(long, short)]
        test: bool,

        /// Clear terminal before each build
        #[arg(long, short)]
        clear: bool,

        /// Debounce delay in milliseconds (default: 500)
        #[arg(long, default_value = "500")]
        debounce: u64,

        /// Watch specific package (in workspace)
        #[arg(short, long)]
        package: Option<String>,
    },

    /// Dependency management and visualization
    Deps {
        #[command(subcommand)]
        command: DepsCommands,
    },

    /// Start the language server (for IDE integration)
    Lsp {
        /// Run on TCP port instead of stdio (for debugging)
        #[arg(long)]
        tcp: Option<u16>,
    },

    /// Run an MCP (Model Context Protocol) server exposing hx to AI agents
    ///
    /// Speaks JSON-RPC over stdio. Point an MCP client (Claude, etc.) at
    /// `hx mcp` to give an agent tools for build, test, run, lock, and more.
    Mcp,

    /// Manage plugins
    Plugins {
        #[command(subcommand)]
        command: PluginsCommands,
    },

    /// Run tests with coverage and generate reports
    Coverage {
        /// Generate HTML coverage report
        #[arg(long)]
        html: bool,

        /// Open HTML report in browser after generation
        #[arg(long)]
        open: bool,

        /// Output directory for coverage reports
        #[arg(long, short, default_value = "coverage")]
        output: std::path::PathBuf,

        /// Minimum coverage threshold (percentage, 0-100)
        #[arg(long)]
        threshold: Option<u8>,

        /// Run coverage for specific package (in workspace)
        #[arg(long, short)]
        package: Option<String>,

        /// Filter tests by pattern
        #[arg(long)]
        pattern: Option<String>,

        /// Output coverage summary as JSON
        #[arg(long)]
        json: bool,

        /// Exclude modules from coverage (comma-separated)
        #[arg(long)]
        exclude: Option<String>,
    },

    /// Create distributable release archives
    #[command(disable_version_flag = true)]
    Dist {
        #[command(subcommand)]
        command: Option<DistCommands>,

        /// Target triple (e.g., x86_64-apple-darwin)
        #[arg(long)]
        target: Option<String>,

        /// Output directory
        #[arg(long, short, default_value = "dist")]
        output: std::path::PathBuf,

        /// Strip debug symbols
        #[arg(long, default_value = "true")]
        strip: bool,

        /// Include shell completions
        #[arg(long, default_value = "true")]
        completions: bool,

        /// Version for archive name (default: from Cargo.toml)
        #[arg(long, id = "pkg_version")]
        version: Option<String>,
    },

    /// Manage global configuration
    ///
    /// Global config provides defaults for all projects.
    /// Located at ~/.config/hx/config.toml (or platform equivalent).
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Manage BHC Platform curated snapshots
    BhcPlatform {
        #[command(subcommand)]
        command: BhcPlatformCommands,
    },

    /// Persistent compilation server for fast incremental rebuilds
    ///
    /// The server keeps a GHCi process running in the background to enable
    /// sub-second incremental rebuilds via :reload instead of full builds.
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServerCommands {
    /// Start the compilation server
    Start,

    /// Stop the compilation server
    Stop,

    /// Show compilation server status
    Status,

    /// Restart the compilation server
    Restart,
}

#[derive(Subcommand, Debug)]
pub enum BhcPlatformCommands {
    /// List available BHC Platform snapshots
    List,

    /// Show information about a BHC Platform snapshot
    Info {
        /// Platform identifier (e.g., bhc-platform-2026.1)
        platform: String,

        /// Show all packages in the platform
        #[arg(long)]
        packages: bool,
    },

    /// Set the BHC Platform snapshot for the current project
    Set {
        /// Platform identifier (e.g., bhc-platform-2026.1)
        platform: String,
    },

    /// Compare two BHC Platform snapshots
    Diff {
        /// Old platform identifier (e.g., bhc-platform-2026.1)
        old: String,

        /// New platform identifier (e.g., bhc-platform-2026.2)
        new: String,
    },

    /// Fetch latest snapshot index from the registry
    Update,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Show current global configuration
    Show,

    /// Show path to global config file
    Path,

    /// Open global config in editor
    Edit,

    /// Set a configuration value
    Set {
        /// Configuration key (e.g., toolchain.ghc, build.optimization)
        key: String,

        /// Value to set
        value: String,
    },

    /// Get a configuration value
    Get {
        /// Configuration key (e.g., toolchain.ghc, build.optimization)
        key: String,
    },

    /// Initialize global config file with defaults
    Init {
        /// Overwrite existing config
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ImportSource {
    /// Import from stack.yaml
    Stack,
    /// Import from cabal.project
    Cabal,
}

#[derive(Subcommand, Debug)]
pub enum NixCommands {
    /// Generate a flake.nix file
    Flake {
        /// Output file (default: flake.nix)
        #[arg(long, short)]
        output: Option<String>,

        /// Overwrite existing file
        #[arg(long)]
        force: bool,
    },

    /// Generate a shell.nix file
    Shell {
        /// Output file (default: shell.nix)
        #[arg(long, short)]
        output: Option<String>,

        /// Overwrite existing file
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CompletionsCommands {
    /// Generate completions for a shell (output to stdout)
    Generate {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Install completions for your shell
    Install {
        /// Shell to install completions for (auto-detected if not specified)
        #[arg(long, value_enum)]
        shell: Option<Shell>,
    },

    /// Generate man pages
    Manpages {
        /// Output directory for man pages
        #[arg(long, short, default_value = "man")]
        output: std::path::PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum IdeCommands {
    /// Generate or update hie.yaml for HLS
    Setup {
        /// Overwrite existing hie.yaml
        #[arg(long)]
        force: bool,
    },

    /// Check IDE/HLS configuration status
    Status,
}

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Show cache status and statistics
    Status,

    /// Prune old cache entries
    Prune {
        /// Maximum age in days (default: 30)
        #[arg(long, default_value = "30")]
        days: u64,
    },

    /// Clean the entire cache
    Clean,

    /// Manage compiled artifact cache
    Artifacts {
        #[command(subcommand)]
        command: ArtifactCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum ArtifactCommands {
    /// Show artifact cache status
    Status,

    /// Prune old artifacts
    Prune {
        /// Maximum age in days (default: 30)
        #[arg(long, default_value = "30")]
        days: u64,
    },

    /// Clear all artifacts
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum NewCommands {
    /// Create a new Haskell module
    Module {
        /// Module name (e.g., Data.List.Extra)
        name: String,

        /// Create in src directory (default)
        #[arg(long, conflicts_with = "test")]
        src: bool,

        /// Create in test directory
        #[arg(long)]
        test: bool,
    },

    /// Create a new test module
    Test {
        /// Test module name
        name: String,
    },

    /// Create a new benchmark module
    Benchmark {
        /// Benchmark module name
        name: String,
    },

    /// Create a web application project (Servant)
    Webapp {
        /// Project name
        name: String,

        /// Target directory (defaults to project name)
        #[arg(long)]
        dir: Option<String>,

        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Create a CLI application project (optparse-applicative)
    Cli {
        /// Project name
        name: String,

        /// Target directory (defaults to project name)
        #[arg(long)]
        dir: Option<String>,

        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Create a library project with documentation setup
    Library {
        /// Project name
        name: String,

        /// Target directory (defaults to project name)
        #[arg(long)]
        dir: Option<String>,

        /// Compiler backend to use (ghc or bhc)
        #[arg(long, value_parser = parse_compiler_backend)]
        backend: Option<hx_config::CompilerBackend>,
    },

    /// Create a numeric/scientific computing project (BHC)
    Numeric {
        /// Project name
        name: String,

        /// Target directory (defaults to project name)
        #[arg(long)]
        dir: Option<String>,
    },

    /// Create a server application project (BHC + Servant)
    Server {
        /// Project name
        name: String,

        /// Target directory (defaults to project name)
        #[arg(long)]
        dir: Option<String>,
    },

    /// Create a project from a git repository template
    Template {
        /// Git repository URL or GitHub shorthand (user/repo)
        url: String,

        /// Project name
        name: String,

        /// Target directory (defaults to project name)
        #[arg(long)]
        dir: Option<String>,

        /// Git branch to use
        #[arg(long, default_value = "main")]
        branch: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum IndexCommands {
    /// Update the package index from Hackage
    Update {
        /// Force full download even if index exists
        #[arg(long)]
        force: bool,

        /// Staleness threshold in hours (0 = always update)
        #[arg(long)]
        staleness: Option<u64>,
    },

    /// Show index status and statistics
    Status,

    /// Clear the local index
    Clear {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum StackageCommands {
    /// List available Stackage snapshots
    List {
        /// Show only LTS snapshots
        #[arg(long)]
        lts: bool,

        /// Show only Nightly snapshots
        #[arg(long)]
        nightly: bool,

        /// Maximum number of snapshots to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Show information about a snapshot
    Info {
        /// Snapshot identifier (e.g., lts-22.28, nightly-2024-01-15)
        snapshot: String,

        /// Show all packages in the snapshot
        #[arg(long)]
        packages: bool,
    },

    /// Set the Stackage snapshot for the current project
    Set {
        /// Snapshot identifier (e.g., lts-22.28, nightly-2024-01-15, lts, nightly)
        snapshot: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ToolchainCommands {
    /// Show installed toolchain versions and status
    Status,

    /// List available and installed GHC versions
    List {
        /// Show available versions from known list
        #[arg(long)]
        available: bool,

        /// Show only installed versions
        #[arg(long)]
        installed: bool,
    },

    /// Install a GHC version
    #[command(disable_version_flag = true)]
    Install {
        /// GHC version to install (e.g., 9.8.2)
        #[arg(id = "ghc_version")]
        version: Option<String>,

        /// GHC version (alternative to positional)
        #[arg(long)]
        ghc: Option<String>,

        /// Cabal version to install
        #[arg(long)]
        cabal: Option<String>,

        /// HLS version to install
        #[arg(long)]
        hls: Option<String>,

        /// BHC version to install (e.g., 0.2.0 or "latest")
        #[arg(long)]
        bhc: Option<String>,

        /// Set as active version after installation
        #[arg(long)]
        set: bool,

        /// Force reinstall even if already installed
        #[arg(long)]
        force: bool,

        /// Use ghcup for installation instead of direct download
        #[arg(long)]
        ghcup: bool,
    },

    /// Remove an installed GHC version
    #[command(disable_version_flag = true)]
    Remove {
        /// GHC version to remove
        #[arg(id = "ghc_version")]
        version: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Set the active GHC version
    #[command(disable_version_flag = true)]
    Use {
        /// GHC version to activate
        #[arg(id = "ghc_version")]
        version: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum DepsCommands {
    /// Show dependency graph
    Graph {
        /// Output format: dot, tree, list, or json
        #[arg(long, short, default_value = "tree")]
        format: GraphFormat,

        /// Maximum depth to display (0 = unlimited)
        #[arg(long, short, default_value = "0")]
        depth: usize,

        /// Highlight specific packages (comma-separated)
        #[arg(long)]
        highlight: Option<String>,

        /// Output file (stdout if not specified)
        #[arg(long, short)]
        output: Option<String>,

        /// Include dev dependencies
        #[arg(long)]
        dev: bool,

        /// Show only direct dependencies
        #[arg(long)]
        direct: bool,
    },

    /// Show dependency tree (alias for graph --format tree)
    Tree {
        /// Maximum depth to display (0 = unlimited)
        #[arg(long, short, default_value = "0")]
        depth: usize,

        /// Include dev dependencies
        #[arg(long)]
        dev: bool,
    },

    /// List all dependencies
    List {
        /// Include dev dependencies
        #[arg(long)]
        dev: bool,

        /// Show only direct dependencies
        #[arg(long)]
        direct: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GraphFormat {
    /// Graphviz DOT format
    Dot,
    /// ASCII tree format
    Tree,
    /// Simple list format
    List,
    /// JSON format
    Json,
}

#[derive(Subcommand, Debug)]
pub enum PluginsCommands {
    /// List available and loaded plugins
    List,

    /// Show plugin system status
    Status,

    /// Run a plugin script directly
    Run {
        /// Path to the script file
        script: String,

        /// Arguments to pass to the script
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// Allow this project's local plugins (.hx/plugins) to run
    Trust,

    /// Revoke permission for this project's local plugins
    Untrust,
}

#[derive(Subcommand, Debug)]
pub enum DistCommands {
    /// Generate Homebrew formula
    #[command(disable_version_flag = true)]
    Formula {
        /// Version for the formula
        #[arg(long, id = "pkg_version")]
        version: Option<String>,

        /// Output file (default: stdout)
        #[arg(long, short)]
        output: Option<std::path::PathBuf>,
    },

    /// Generate installation script
    #[command(disable_version_flag = true)]
    InstallScript {
        /// Version for the script
        #[arg(long, id = "pkg_version")]
        version: Option<String>,

        /// Output file (default: stdout)
        #[arg(long, short)]
        output: Option<std::path::PathBuf>,
    },
}

/// Parse compiler backend from string.
fn parse_compiler_backend(s: &str) -> Result<hx_config::CompilerBackend, String> {
    s.parse()
}
