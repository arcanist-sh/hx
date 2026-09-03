# Building

`hx-cabal` wraps Cabal commands for building, testing, and running Haskell projects.

## Build Command

```rust
use hx_cabal::build::{build, BuildOptions};

let options = BuildOptions {
    release: false,
    jobs: Some(4),
    target: None,
    package: None,
    verbose: false,
    fingerprint: None,
    ghc_version: Some("9.8.2".into()),
    package_count: Some(1),
    project_name: Some("my-project".into()),
    toolchain_bin_dirs: vec![],
};

let result = build(&project_root, &build_dir, &options, &output).await?;
```

## BuildOptions

```rust
pub struct BuildOptions {
    /// Build with optimizations (-O2)
    pub release: bool,

    /// Number of parallel jobs
    pub jobs: Option<usize>,

    /// Specific target (lib, exe, test)
    pub target: Option<String>,

    /// Specific package in workspace
    pub package: Option<String>,

    /// Enable verbose output
    pub verbose: bool,

    /// Lockfile fingerprint for caching
    pub fingerprint: Option<String>,

    /// GHC version for display
    pub ghc_version: Option<String>,

    /// Total packages in workspace
    pub package_count: Option<usize>,

    /// Project name for display
    pub project_name: Option<String>,

    /// Toolchain bin directories to add to PATH
    pub toolchain_bin_dirs: Vec<PathBuf>,
}
```

## Cabal Invocation

The build function constructs and runs:

```bash
cabal --store-dir=<cache>/cabal/store build \
    --builddir=.hx/build \
    --jobs=4 \
    -O2  # if release
```

### Global Options

These must come **before** the subcommand:
- `--store-dir` - Package store location

### Build Options

These come **after** the subcommand:
- `--builddir` - Build artifact directory
- `--jobs` - Parallelism
- `-O2` - Optimization (release mode)

## Test Command

```rust
use hx_cabal::build::test;

let exit_code = test(
    &project_root,
    &build_dir,
    Some("MyTest"),      // Test pattern
    Some("my-package"),  // Package filter
    &toolchain_bin_dirs,
    &output,
).await?;
```

Runs:
```bash
cabal --store-dir=<cache>/cabal/store test \
    --builddir=.hx/build \
    --test-show-details=streaming \
    my-package:test:MyTest
```

## Run Command

```rust
use hx_cabal::build::run;

let exit_code = run(
    &project_root,
    &build_dir,
    &["--port", "8080"],  // Args to executable
    Some("my-package"),
    &toolchain_bin_dirs,
    &output,
).await?;
```

Runs:
```bash
cabal --store-dir=<cache>/cabal/store run \
    --builddir=.hx/build \
    my-package -- --port 8080
```

## REPL Command

```rust
use hx_cabal::build::repl;

let exit_code = repl(
    &project_root,
    &build_dir,
    &toolchain_bin_dirs,
).await?;
```

Runs:
```bash
cabal --store-dir=<cache>/cabal/store repl --builddir=.hx/build
```

## Build Results

```rust
pub struct BuildResult {
    /// Build succeeded
    pub success: bool,

    /// Build duration
    pub duration: Duration,

    /// Raw output
    pub output: String,
}
```

## Environment Setup

Toolchain bin directories are prepended to PATH:

```rust
let mut cmd = CommandRunner::new()
    .working_dir(&project_root);

for bin_dir in &toolchain_bin_dirs {
    cmd = cmd.with_ghc_bin(bin_dir);
}

cmd.run("cabal", &args).await
```

This ensures hx-managed GHC/Cabal are used.

## Store Directory

hx uses a shared store for built packages, under the platform cache
directory (`hx_cache::cabal_store_dir()`, i.e. `<global cache>/cabal/store` --
see `crates/hx-cache/docs/directories.md` for the per-platform locations):

```
<cache>/cabal/store/
├── ghc-9.8.2/
│   ├── aeson-2.2.1.0-abc123/
│   ├── text-2.1-def456/
│   └── ...
```

This enables sharing compiled packages across projects.

## Build Directory

Project-local build artifacts:

```
.hx/build/
├── build/
│   └── my-project/
├── cache/
└── packagedb/
```

## Error Handling

```rust
match build(&root, &build_dir, &options, &output).await {
    Ok(result) if result.success => {
        println!("Build succeeded in {:?}", result.duration);
    }
    Ok(result) => {
        eprintln!("Build failed");
        // Parse output for errors
    }
    Err(e) => {
        output.print_error(&e);
    }
}
```
