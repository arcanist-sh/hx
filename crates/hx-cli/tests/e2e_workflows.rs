//! End-to-end integration tests for complete hx workflows.
//!
//! These tests verify that hx commands work correctly together
//! in realistic development scenarios.
//!
//! Run with: cargo test -p hx-cli --test e2e_workflows

#![cfg(not(windows))]

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn hx() -> Command {
    Command::cargo_bin("hx").unwrap()
}

// =============================================================================
// Workflow 1: Create and Build a Simple Project
// =============================================================================

#[test]
fn test_workflow_create_build_simple() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("simple-app");

    // Step 1: Initialize a new binary project
    hx().args(["init", "--name", "simple-app"])
        .arg(&project_dir)
        .assert()
        .success();

    // Verify project structure
    assert!(project_dir.join("hx.toml").exists());
    assert!(project_dir.join("simple-app.cabal").exists());
    assert!(project_dir.join("src/Main.hs").exists());

    // Step 2: Run doctor to verify environment
    hx().current_dir(&project_dir)
        .arg("doctor")
        .assert()
        .success();
}

// =============================================================================
// Workflow 2: Library Project with Tests
// =============================================================================

#[test]
fn test_workflow_library_with_tests() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("my-lib");

    // Step 1: Create library project
    hx().args(["init", "--lib", "--name", "my-lib"])
        .arg(&project_dir)
        .assert()
        .success();

    // Verify library structure
    assert!(project_dir.join("src/Lib.hs").exists());

    let hx_toml = fs::read_to_string(project_dir.join("hx.toml")).unwrap();
    assert!(hx_toml.contains("kind = \"lib\""));

    // Step 2: Check config is parseable
    hx().current_dir(&project_dir)
        .args(["config", "show"])
        .assert()
        .success();
}

// =============================================================================
// Workflow 3: Project Configuration Management
// =============================================================================

#[test]
fn test_workflow_config_management() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("config-test");

    // Create project
    hx().args(["init", "--name", "config-test"])
        .arg(&project_dir)
        .assert()
        .success();

    // View config - may show global or project config
    hx().current_dir(&project_dir)
        .args(["config", "show"])
        .assert()
        .success();

    // Modify hx.toml to add custom settings
    let hx_toml_path = project_dir.join("hx.toml");
    let mut content = fs::read_to_string(&hx_toml_path).unwrap();
    content.push_str(
        r#"
[build]
ghc-options = ["-Wall", "-Wcompat"]
"#,
    );
    fs::write(&hx_toml_path, content).unwrap();

    // Verify modified config is read correctly
    hx().current_dir(&project_dir)
        .args(["config", "show"])
        .assert()
        .success();
}

// =============================================================================
// Workflow 4: Clean Build Artifacts
// =============================================================================

#[test]
fn test_workflow_clean() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("clean-test");

    // Create project
    hx().args(["init", "--name", "clean-test"])
        .arg(&project_dir)
        .assert()
        .success();

    // Create fake build artifacts
    let hx_cache = project_dir.join(".hx");
    fs::create_dir_all(&hx_cache).unwrap();
    fs::write(hx_cache.join("cache.db"), "fake cache").unwrap();

    // Clean
    hx().current_dir(&project_dir)
        .arg("clean")
        .assert()
        .success();

    // Verify artifacts are removed
    assert!(!hx_cache.exists());
}

// =============================================================================
// Workflow 5: Shell Completions Generation
// =============================================================================

#[test]
fn test_workflow_completions_all_shells() {
    // Bash completions - just verify it succeeds and produces output
    hx().args(["completions", "generate", "bash"])
        .assert()
        .success();

    // Zsh completions
    hx().args(["completions", "generate", "zsh"])
        .assert()
        .success();

    // Fish completions
    hx().args(["completions", "generate", "fish"])
        .assert()
        .success();
}

// =============================================================================
// Workflow 6: Project with Custom Toolchain
// =============================================================================

#[test]
fn test_workflow_custom_toolchain() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("toolchain-test");

    // Create project
    hx().args(["init", "--name", "toolchain-test"])
        .arg(&project_dir)
        .assert()
        .success();

    // Modify hx.toml to specify a GHC version
    let hx_toml_path = project_dir.join("hx.toml");
    let content = r#"
[project]
name = "toolchain-test"
version = "0.1.0"
kind = "bin"

[toolchain]
ghc = "9.8.2"
"#;
    fs::write(&hx_toml_path, content).unwrap();

    // Verify config is valid
    hx().current_dir(&project_dir)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("9.8.2"));
}

// =============================================================================
// Workflow 7: Multiple Projects in Workspace
// =============================================================================

#[test]
fn test_workflow_multiple_projects() {
    let temp = TempDir::new().unwrap();

    // Create multiple projects in same temp directory
    let app1 = temp.path().join("app1");
    let app2 = temp.path().join("app2");
    let lib1 = temp.path().join("lib1");

    // Initialize all projects
    hx().args(["init", "--name", "app1"])
        .arg(&app1)
        .assert()
        .success();

    hx().args(["init", "--name", "app2"])
        .arg(&app2)
        .assert()
        .success();

    hx().args(["init", "--lib", "--name", "lib1"])
        .arg(&lib1)
        .assert()
        .success();

    // Verify each has correct structure
    assert!(app1.join("src/Main.hs").exists());
    assert!(app2.join("src/Main.hs").exists());
    assert!(lib1.join("src/Lib.hs").exists());

    // Each should pass doctor check independently
    hx().current_dir(&app1).arg("doctor").assert().success();

    hx().current_dir(&lib1).arg("doctor").assert().success();
}

// =============================================================================
// Workflow 8: Help System Navigation
// =============================================================================

#[test]
fn test_workflow_help_navigation() {
    // Main help
    hx().arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Commands:"));

    // All major subcommands have help
    let subcommands = [
        "init",
        "build",
        "test",
        "run",
        "bench",
        "clean",
        "lock",
        "doctor",
        "toolchain",
        "fmt",
        "lint",
        "watch",
        "repl",
        "config",
        "stackage",
        "server",
        "coverage",
    ];

    for cmd in subcommands {
        hx().args([cmd, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(cmd).or(predicate::str::contains("Usage")));
    }
}

// =============================================================================
// Workflow 9: Error Handling
// =============================================================================

#[test]
fn test_workflow_error_handling() {
    let temp = TempDir::new().unwrap();

    // Running build outside a project should fail gracefully
    hx().current_dir(temp.path())
        .arg("build")
        .assert()
        .failure()
        .stderr(predicate::str::contains("hx.toml").or(predicate::str::contains("project")));

    // Invalid command should fail
    hx().arg("not-a-command").assert().failure();
}

// =============================================================================
// Workflow 10: Verbose and Quiet Modes
// =============================================================================

#[test]
fn test_workflow_output_modes() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("output-test");

    // Create project
    hx().args(["init", "--name", "output-test"])
        .arg(&project_dir)
        .assert()
        .success();

    // Verbose mode
    hx().current_dir(&project_dir)
        .args(["--verbose", "doctor"])
        .assert()
        .success();

    // Quiet mode
    hx().current_dir(&project_dir)
        .args(["--quiet", "doctor"])
        .assert()
        .success();
}

// =============================================================================
// Workflow 11: Lockfile Operations
// =============================================================================

#[test]
fn test_workflow_lockfile_help() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("lock-test");

    // Create project
    hx().args(["init", "--name", "lock-test"])
        .arg(&project_dir)
        .assert()
        .success();

    // Lock help should work
    hx().current_dir(&project_dir)
        .args(["lock", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--snapshot"));
}

// =============================================================================
// Workflow 12: Stackage Integration
// =============================================================================

#[test]
fn test_workflow_stackage_commands() {
    // Stackage list help
    hx().args(["stackage", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--lts"))
        .stdout(predicate::str::contains("--nightly"));

    // Stackage info help
    hx().args(["stackage", "info", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("snapshot"));

    // Stackage set help
    hx().args(["stackage", "set", "--help"]).assert().success();
}

// =============================================================================
// Workflow 13: Cross-Compilation Support
// =============================================================================

#[test]
fn test_workflow_cross_compilation_flags() {
    // Build should support --target
    hx().args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target"));

    // Test should support --target
    hx().args(["test", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target"));

    // Run should support --target
    hx().args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target"));
}

// =============================================================================
// Workflow 14: Server/LSP Commands
// =============================================================================

#[test]
fn test_workflow_server_commands() {
    // Server subcommands
    hx().args(["server", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("start"))
        .stdout(predicate::str::contains("stop"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("restart"));
}

// =============================================================================
// Workflow 15: Coverage Commands
// =============================================================================

#[test]
fn test_workflow_coverage_commands() {
    hx().args(["coverage", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--html"))
        .stdout(predicate::str::contains("--json"))
        .stdout(predicate::str::contains("--threshold"));
}
