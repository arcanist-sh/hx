//! CLI command benchmarks for hx.
//!
//! These benchmarks measure the performance of hx CLI commands
//! compared to baseline implementations (cabal, stack).
//!
//! Run with: cargo bench -p hx-cli
//! Run specific: cargo bench -p hx-cli -- init
//!
//! Results are saved to target/criterion/ with HTML reports.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

/// Helper to create a temporary directory for benchmarks
fn create_temp_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp directory")
}

/// Helper to check if cabal is available
fn has_cabal() -> bool {
    Command::new("cabal")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Helper to check if stack is available
fn has_stack() -> bool {
    Command::new("stack")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// =============================================================================
// Project Initialization Benchmarks
// =============================================================================

/// Benchmark hx init command
fn bench_init(c: &mut Criterion) {
    let mut group = c.benchmark_group("init");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(10));

    // Benchmark hx init (binary project)
    group.bench_function("hx_init_bin", |b| {
        b.iter_with_setup(create_temp_dir, |temp| {
            let project_dir = temp.path().join("bench-project");
            Command::new(env!("CARGO_BIN_EXE_hx"))
                .args(["init", "--name", "bench-project"])
                .arg(&project_dir)
                .output()
                .expect("hx init failed");
        })
    });

    // Benchmark hx init (library project)
    group.bench_function("hx_init_lib", |b| {
        b.iter_with_setup(create_temp_dir, |temp| {
            let project_dir = temp.path().join("bench-lib");
            Command::new(env!("CARGO_BIN_EXE_hx"))
                .args(["init", "--lib", "--name", "bench-lib"])
                .arg(&project_dir)
                .output()
                .expect("hx init failed");
        })
    });

    // Compare with cabal init if available
    if has_cabal() {
        group.bench_function("cabal_init", |b| {
            b.iter_with_setup(create_temp_dir, |temp| {
                let project_dir = temp.path().join("cabal-project");
                std::fs::create_dir_all(&project_dir).unwrap();
                Command::new("cabal")
                    .current_dir(&project_dir)
                    .args([
                        "init",
                        "--non-interactive",
                        "--package-name=cabal-project",
                        "--no-comments",
                    ])
                    .output()
                    .expect("cabal init failed");
            })
        });
    }

    // Compare with stack new if available
    if has_stack() {
        group.bench_function("stack_new", |b| {
            b.iter_with_setup(create_temp_dir, |temp| {
                Command::new("stack")
                    .current_dir(temp.path())
                    .args(["new", "stack-project", "simple"])
                    .output()
                    .expect("stack new failed");
            })
        });
    }

    group.finish();
}

// =============================================================================
// Help/Version Benchmarks (CLI Startup Time)
// =============================================================================

/// Benchmark CLI startup time (help command)
fn bench_startup(c: &mut Criterion) {
    let mut group = c.benchmark_group("startup");
    group.sample_size(50);

    // hx --help
    group.bench_function("hx_help", |b| {
        b.iter(|| {
            Command::new(env!("CARGO_BIN_EXE_hx"))
                .arg("--help")
                .output()
                .expect("hx --help failed")
        })
    });

    // hx --version
    group.bench_function("hx_version", |b| {
        b.iter(|| {
            Command::new(env!("CARGO_BIN_EXE_hx"))
                .arg("--version")
                .output()
                .expect("hx --version failed")
        })
    });

    // Compare with cabal
    if has_cabal() {
        group.bench_function("cabal_help", |b| {
            b.iter(|| {
                Command::new("cabal")
                    .arg("--help")
                    .output()
                    .expect("cabal --help failed")
            })
        });

        group.bench_function("cabal_version", |b| {
            b.iter(|| {
                Command::new("cabal")
                    .arg("--version")
                    .output()
                    .expect("cabal --version failed")
            })
        });
    }

    // Compare with stack
    if has_stack() {
        group.bench_function("stack_help", |b| {
            b.iter(|| {
                Command::new("stack")
                    .arg("--help")
                    .output()
                    .expect("stack --help failed")
            })
        });

        group.bench_function("stack_version", |b| {
            b.iter(|| {
                Command::new("stack")
                    .arg("--version")
                    .output()
                    .expect("stack --version failed")
            })
        });
    }

    group.finish();
}

// =============================================================================
// Doctor Command Benchmarks
// =============================================================================

/// Benchmark hx doctor (environment diagnostics)
fn bench_doctor(c: &mut Criterion) {
    let mut group = c.benchmark_group("doctor");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));

    group.bench_function("hx_doctor", |b| {
        b.iter(|| {
            Command::new(env!("CARGO_BIN_EXE_hx"))
                .arg("doctor")
                .output()
                .expect("hx doctor failed")
        })
    });

    group.finish();
}

// =============================================================================
// Config Parsing Benchmarks
// =============================================================================

/// Create a sample hx.toml for config parsing benchmarks
fn create_sample_project(temp: &TempDir) -> PathBuf {
    let project_dir = temp.path().join("config-bench");
    std::fs::create_dir_all(&project_dir).unwrap();

    let hx_toml = r#"
[project]
name = "config-bench"
version = "0.1.0"
kind = "bin"

[toolchain]
ghc = "9.8.2"

[dependencies]
base = "^>=4.18"
text = "^>=2.0"
containers = "^>=0.6"
aeson = "^>=2.2"

[build]
ghc-options = ["-Wall", "-Wcompat", "-O2"]
split-sections = true

[dev-dependencies]
hspec = "^>=2.11"
QuickCheck = "^>=2.14"
"#;

    std::fs::write(project_dir.join("hx.toml"), hx_toml).unwrap();

    // Create minimal cabal file
    let cabal = r#"
cabal-version: 3.0
name: config-bench
version: 0.1.0.0
build-type: Simple

executable config-bench
    main-is: Main.hs
    hs-source-dirs: src
    default-language: GHC2021
    build-depends: base ^>=4.18
"#;
    std::fs::write(project_dir.join("config-bench.cabal"), cabal).unwrap();

    // Create src directory and Main.hs
    std::fs::create_dir_all(project_dir.join("src")).unwrap();
    std::fs::write(
        project_dir.join("src/Main.hs"),
        "module Main where\nmain :: IO ()\nmain = putStrLn \"Hello\"",
    )
    .unwrap();

    project_dir
}

/// Benchmark config parsing and project detection
fn bench_config(c: &mut Criterion) {
    let mut group = c.benchmark_group("config");
    group.sample_size(30);

    let temp = create_temp_dir();
    let project_dir = create_sample_project(&temp);

    // Benchmark hx config show (reads and displays config)
    group.bench_function("hx_config_show", |b| {
        b.iter(|| {
            Command::new(env!("CARGO_BIN_EXE_hx"))
                .current_dir(&project_dir)
                .args(["config", "show"])
                .output()
                .expect("hx config show failed")
        })
    });

    group.finish();
}

// =============================================================================
// Clean Command Benchmarks
// =============================================================================

/// Benchmark hx clean command
fn bench_clean(c: &mut Criterion) {
    let mut group = c.benchmark_group("clean");
    group.sample_size(20);

    // Setup: create a project with build artifacts
    let temp = create_temp_dir();
    let project_dir = temp.path().join("clean-bench");

    // Initialize project
    Command::new(env!("CARGO_BIN_EXE_hx"))
        .args(["init", "--name", "clean-bench"])
        .arg(&project_dir)
        .output()
        .expect("hx init failed");

    // Create fake build artifacts
    let hx_cache = project_dir.join(".hx");
    let dist_newstyle = project_dir.join("dist-newstyle");
    std::fs::create_dir_all(&hx_cache).unwrap();
    std::fs::create_dir_all(&dist_newstyle).unwrap();

    // Create some fake files to clean
    for i in 0..10 {
        std::fs::write(hx_cache.join(format!("artifact-{}.o", i)), "fake").unwrap();
        std::fs::create_dir_all(dist_newstyle.join(format!("build-{}", i))).unwrap();
    }

    group.bench_function("hx_clean", |b| {
        b.iter_with_setup(
            || {
                // Recreate artifacts before each iteration
                std::fs::create_dir_all(&hx_cache).ok();
                std::fs::create_dir_all(&dist_newstyle).ok();
                for i in 0..10 {
                    std::fs::write(hx_cache.join(format!("artifact-{}.o", i)), "fake").ok();
                }
            },
            |_| {
                Command::new(env!("CARGO_BIN_EXE_hx"))
                    .current_dir(&project_dir)
                    .arg("clean")
                    .output()
                    .expect("hx clean failed")
            },
        )
    });

    // Compare with cabal clean
    if has_cabal() {
        group.bench_function("cabal_clean", |b| {
            b.iter_with_setup(
                || {
                    std::fs::create_dir_all(&dist_newstyle).ok();
                    for i in 0..10 {
                        std::fs::create_dir_all(dist_newstyle.join(format!("build-{}", i))).ok();
                    }
                },
                |_| {
                    Command::new("cabal")
                        .current_dir(&project_dir)
                        .arg("clean")
                        .output()
                        .expect("cabal clean failed")
                },
            )
        });
    }

    group.finish();
}

// =============================================================================
// Completions Generation Benchmarks
// =============================================================================

/// Benchmark shell completions generation
fn bench_completions(c: &mut Criterion) {
    let mut group = c.benchmark_group("completions");
    group.sample_size(30);

    for shell in ["bash", "zsh", "fish"] {
        group.bench_with_input(BenchmarkId::new("hx", shell), shell, |b, shell| {
            b.iter(|| {
                Command::new(env!("CARGO_BIN_EXE_hx"))
                    .args(["completions", "generate", shell])
                    .output()
                    .expect("hx completions failed")
            })
        });
    }

    group.finish();
}

// =============================================================================
// Lockfile Operations Benchmarks
// =============================================================================

/// Create a project with dependencies for lock benchmarks
fn create_deps_project(temp: &TempDir) -> PathBuf {
    let project_dir = temp.path().join("lock-bench");
    std::fs::create_dir_all(&project_dir).unwrap();

    let hx_toml = r#"
[project]
name = "lock-bench"
version = "0.1.0"
kind = "bin"

[toolchain]
ghc = "9.8.2"

[dependencies]
base = "^>=4.18"
text = "^>=2.0"
bytestring = "^>=0.11"
containers = "^>=0.6"
"#;
    std::fs::write(project_dir.join("hx.toml"), hx_toml).unwrap();

    let cabal = r#"
cabal-version: 3.0
name: lock-bench
version: 0.1.0.0
build-type: Simple

executable lock-bench
    main-is: Main.hs
    hs-source-dirs: src
    default-language: GHC2021
    build-depends:
        base ^>=4.18,
        text ^>=2.0,
        bytestring ^>=0.11,
        containers ^>=0.6
"#;
    std::fs::write(project_dir.join("lock-bench.cabal"), cabal).unwrap();

    std::fs::create_dir_all(project_dir.join("src")).unwrap();
    std::fs::write(
        project_dir.join("src/Main.hs"),
        "module Main where\nmain :: IO ()\nmain = pure ()",
    )
    .unwrap();

    project_dir
}

/// Benchmark lockfile check (--check flag)
fn bench_lock_check(c: &mut Criterion) {
    let mut group = c.benchmark_group("lock_check");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(15));

    let temp = create_temp_dir();
    let project_dir = create_deps_project(&temp);

    // Create a simple lockfile
    let lock_content = r#"
# This file is auto-generated by hx lock
# Do not edit manually

version = 1

[[package]]
name = "base"
version = "4.18.2.1"
flags = []

[[package]]
name = "text"
version = "2.0.2"
flags = []
"#;
    std::fs::write(project_dir.join("hx.lock"), lock_content).unwrap();

    group.bench_function("hx_lock_check", |b| {
        b.iter(|| {
            Command::new(env!("CARGO_BIN_EXE_hx"))
                .current_dir(&project_dir)
                .args(["lock", "--check"])
                .output()
        })
    });

    group.finish();
}

// =============================================================================
// Main Criterion Configuration
// =============================================================================

criterion_group! {
    name = cli_benches;
    config = Criterion::default()
        .with_output_color(true)
        .warm_up_time(Duration::from_secs(2))
        .significance_level(0.05)
        .noise_threshold(0.02);
    targets =
        bench_init,
        bench_startup,
        bench_doctor,
        bench_config,
        bench_clean,
        bench_completions,
        bench_lock_check,
}

criterion_main!(cli_benches);
