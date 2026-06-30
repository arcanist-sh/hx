//! hx - Haskell Toolchain CLI
//!
//! A fast, opinionated, batteries-included toolchain for Haskell.

// TODO: Fix these collapsible_if patterns throughout the codebase
#![allow(clippy::collapsible_if)]

use anyhow::Result;
use clap::Parser;

mod cli;
mod commands;
mod plugins;
mod styles;
mod templates;

use cli::Cli;

/// Stack size for the thread that drives the async runtime.
///
/// The OS default main-thread stack is only ~1 MB on Windows (vs ~8 MB on
/// Linux/macOS). Deep async + `serde_json` work — notably the `hx mcp` server
/// — overflows it there, exiting with `STATUS_STACK_OVERFLOW` (0xC00000FD). Run
/// everything on a thread with a generous, explicit stack so every platform
/// behaves like Unix.
const MAIN_STACK_SIZE: usize = 16 * 1024 * 1024;

fn main() -> Result<()> {
    let worker = std::thread::Builder::new()
        .name("hx-main".into())
        .stack_size(MAIN_STACK_SIZE)
        .spawn(run)
        .expect("spawn hx-main thread");
    worker.join().expect("hx-main thread panicked")
}

fn run() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(MAIN_STACK_SIZE)
        .build()?;
    runtime.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize telemetry based on verbosity
    hx_telemetry::init(cli.global.verbose);

    // Enable warnings if not quiet
    if cli.global.quiet == 0 {
        hx_warnings::enable();
    }

    // Run the command
    let exit_code = commands::run(cli).await?;

    std::process::exit(exit_code);
}
