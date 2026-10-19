# hx-core

Core types, error handling, and command execution utilities for the hx toolchain.

## Overview

`hx-core` provides the foundational types and utilities shared across all hx crates. It defines:

- **Structured error handling** with actionable fixes
- **Command execution** with environment management
- **Version parsing** and comparison
- **Exit codes** following CLI conventions
- **Diagnostic types** for GHC output

## Modules

| Module | Description |
|--------|-------------|
| `error` | Error types with fix suggestions |
| `command` | Process spawning and output capture |
| `version` | Semantic version parsing |
| `diagnostic` | GHC diagnostic structures |
| `env` | Environment variable utilities |
| `http` | The shared HTTP client builder (rustls + ring, trust roots, User-Agent, timeouts); every crate builds its clients here |

## Quick Start

```rust
use hx_core::{Error, Result, CommandRunner, Fix};

// Run a command
let runner = CommandRunner::new()
    .working_dir("/path/to/project")
    .with_ghc_bin("/path/to/ghc/bin");

let output = runner.run("ghc", &["--version"]).await?;
println!("GHC version: {}", output.stdout);

// Create errors with fixes
let error = Error::toolchain_missing("ghc")
    .with_fix(Fix::command("Install GHC", "hx toolchain install"));
```

## Error Handling Philosophy

All errors in hx should be **actionable**. Every error includes:

1. **What failed** - Clear description of the problem
2. **Why it failed** - Context about the operation
3. **How to fix it** - One or more suggested fixes

```rust
// Good: actionable error
Error::Config {
    message: "invalid GHC version format".into(),
    path: Some(path),
    source: None,
    fixes: vec![
        Fix::new("Use semantic version format (e.g., 9.8.2)"),
    ],
}

// Bad: unhelpful error
Error::Other(anyhow::anyhow!("config error"))
```

## Documentation

- [Error Types](./errors.md) - Complete error type reference
- [Command Runner](./commands.md) - Process execution guide
- [Exit Codes](./exit-codes.md) - CLI exit code conventions
