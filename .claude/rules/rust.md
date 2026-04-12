# Rust Coding Standards

## General

- Use Rust 1.94.1 (edition 2024)
- Run `cargo fmt` before committing
- All code must pass `cargo clippy -- -D warnings`
- Prefer explicit error handling over `.unwrap()` in library code

## Crate Organization

- Keep `hx-cli` thin - minimal logic, only dispatch
- Put shared types in `hx-core`
- Each crate should have a clear, single responsibility

## Error Handling

- Use `thiserror` for library error types
- Use `anyhow` for CLI-level error propagation
- Create structured error enums per crate
- Errors must be actionable - always include context

```rust
// Good
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file at {path}: {source}")]
    ReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

// Bad - no context
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error")]
    Io(#[from] std::io::Error),
}
```

## Async

- Use `tokio` as the async runtime
- Prefer `tokio::process` for spawning external commands
- Use async for I/O-bound operations (downloads, file reads)
- Keep CLI command handlers async-compatible

## Logging

- Use `tracing` for all logging
- Add spans for major operations
- Use appropriate log levels:
  - `error!` - unrecoverable failures
  - `warn!` - recoverable issues, deprecations
  - `info!` - high-level operation progress
  - `debug!` - detailed operation info
  - `trace!` - very verbose debugging

```rust
#[tracing::instrument(skip(config))]
pub async fn build(config: &Config) -> Result<()> {
    info!("starting build");
    // ...
}
```

## Dependencies

- Minimize dependencies
- Prefer well-maintained, widely-used crates
- Pin major versions in Cargo.toml
- Document why non-obvious dependencies are needed

## Documentation

- All public items need doc comments
- Include examples for complex APIs
- Document panic conditions if any
- Keep docs concise but complete
