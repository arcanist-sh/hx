//! Integration tests for hx CLI.
//!
//! These tests are skipped on Windows due to stack overflow in the CLI binary.
//! See: https://github.com/arcanist-sh/hx/issues/xxx (TODO: file issue)

#![cfg(not(windows))]
#![allow(deprecated)] // cargo_bin is deprecated but the replacement requires macros

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn hx() -> Command {
    Command::cargo_bin("hx").unwrap()
}

#[test]
fn test_help() {
    hx().arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Haskell toolchain CLI"))
        .stdout(predicate::str::contains("Commands:"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("build"))
        .stdout(predicate::str::contains("test"));
}

#[test]
fn test_version() {
    hx().arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("hx"))
        .stdout(predicate::str::is_match(r"\d+\.\d+\.\d+").unwrap());
}

#[test]
fn test_no_command_shows_help() {
    hx().assert()
        .success()
        .stdout(predicate::str::contains("Haskell toolchain CLI"));
}

#[test]
fn test_init_bin_project() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("myapp");

    // --bin is no longer needed (executable is default)
    // Path is now positional instead of --dir
    hx().args(["init", "--name", "myapp"])
        .arg(&project_dir)
        .assert()
        .success();

    // Verify files were created
    assert!(project_dir.join("hx.toml").exists());
    assert!(project_dir.join("myapp.cabal").exists());
    assert!(project_dir.join("src/Main.hs").exists());
    assert!(project_dir.join(".gitignore").exists());
    assert!(project_dir.join(".editorconfig").exists());

    // Verify hx.toml content
    let hx_toml = fs::read_to_string(project_dir.join("hx.toml")).unwrap();
    assert!(hx_toml.contains("name = \"myapp\""));
    assert!(hx_toml.contains("kind = \"bin\""));

    // Verify .cabal content
    let cabal = fs::read_to_string(project_dir.join("myapp.cabal")).unwrap();
    assert!(cabal.contains("name:"));
    assert!(cabal.contains("executable"));
    assert!(cabal.contains("Main.hs"));

    // Verify Main.hs content
    let main_hs = fs::read_to_string(project_dir.join("src/Main.hs")).unwrap();
    assert!(main_hs.contains("module Main"));
    assert!(main_hs.contains("main :: IO ()"));
}

#[test]
fn test_init_lib_project() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("mylib");

    // Path is now positional instead of --dir
    hx().args(["init", "--lib", "--name", "mylib"])
        .arg(&project_dir)
        .assert()
        .success();

    // Verify library-specific files
    assert!(project_dir.join("hx.toml").exists());
    assert!(project_dir.join("mylib.cabal").exists());
    assert!(project_dir.join("src/Lib.hs").exists());

    // Verify hx.toml content
    let hx_toml = fs::read_to_string(project_dir.join("hx.toml")).unwrap();
    assert!(hx_toml.contains("kind = \"lib\""));

    // Verify .cabal content has library section
    let cabal = fs::read_to_string(project_dir.join("mylib.cabal")).unwrap();
    assert!(cabal.contains("library"));
    assert!(cabal.contains("Lib"));
}

#[test]
fn test_init_default_bin() {
    let temp = TempDir::new().unwrap();
    let project_dir = temp.path().join("defaultapp");

    // Without --lib, should default to bin
    // Path is now positional instead of --dir
    hx().args(["init", "--name", "defaultapp"])
        .arg(&project_dir)
        .assert()
        .success();

    let hx_toml = fs::read_to_string(project_dir.join("hx.toml")).unwrap();
    assert!(hx_toml.contains("kind = \"bin\""));
}

#[test]
fn test_verbose_flag() {
    hx().args(["--verbose", "--help"]).assert().success();
}

#[test]
fn test_quiet_flag() {
    hx().args(["--quiet", "--help"]).assert().success();
}

#[test]
fn test_subcommand_help() {
    hx().args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialize"));

    hx().args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Build"));

    hx().args(["doctor", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("doctor"));
}

// ========== Stackage Commands ==========

#[test]
fn test_stackage_help() {
    hx().args(["stackage", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Stackage"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("info"))
        .stdout(predicate::str::contains("set"));
}

#[test]
fn test_stackage_list_help() {
    hx().args(["stackage", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--lts"))
        .stdout(predicate::str::contains("--nightly"))
        .stdout(predicate::str::contains("--limit"));
}

#[test]
fn test_stackage_info_help() {
    hx().args(["stackage", "info", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("snapshot"))
        .stdout(predicate::str::contains("--packages"));
}

#[test]
fn test_stackage_set_help() {
    hx().args(["stackage", "set", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("snapshot"));
}

// ========== Cross-Compilation Flags ==========

#[test]
fn test_build_target_flag() {
    hx().args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target"))
        .stdout(predicate::str::contains("triple"));
}

#[test]
fn test_test_target_flag() {
    hx().args(["test", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target"))
        .stdout(predicate::str::contains("triple"));
}

#[test]
fn test_run_target_flag() {
    hx().args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target"))
        .stdout(predicate::str::contains("triple"));
}

// ========== Lock --snapshot ==========

#[test]
fn test_lock_snapshot_flag() {
    hx().args(["lock", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--snapshot"));
}

// ========== Server Commands ==========

#[test]
fn test_server_help() {
    hx().args(["server", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("start"))
        .stdout(predicate::str::contains("stop"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("restart"));
}

// ========== Coverage Commands ==========

#[test]
fn test_coverage_help() {
    hx().args(["coverage", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--html"))
        .stdout(predicate::str::contains("--threshold"))
        .stdout(predicate::str::contains("--json"));
}
