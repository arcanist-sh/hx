//! Integration tests for building with the plan pinned in hx.lock: `hx build`,
//! `hx test`, `hx run` and `--locked` / `HX_LOCKED`.
//!
//! A mock `cabal` and `ghc` on PATH stand in for the real toolchain; the mock
//! cabal logs its arguments, so the tests check exactly what hx asks cabal to
//! build with. Nothing touches the network.
//!
//! Run with: cargo test -p hx-cli --test locked

#![cfg(unix)]
#![allow(deprecated)] // cargo_bin is deprecated but the replacement requires macros

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const MOCK_CABAL: &str = r#"#!/bin/sh
echo "$@" >> "$MOCK_LOG"
case "$1" in
  --version|--numeric-version) echo "cabal-install version 3.12.1.0"; exit 0 ;;
esac
echo "Resolving dependencies..."
exit 0
"#;

const MOCK_GHC: &str = r#"#!/bin/sh
case "$1" in
  --numeric-version) echo "9.8.2" ;;
  *) echo "The Glorious Glasgow Haskell Compilation System, version 9.8.2" ;;
esac
"#;

struct Env {
    root: TempDir,
}

impl Env {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let mocks = root.path().join("mocks");
        fs::create_dir_all(&mocks).unwrap();
        write_script(&mocks.join("cabal"), MOCK_CABAL);
        write_script(&mocks.join("ghc"), MOCK_GHC);
        for dir in ["home", "hx-home", "cache", "config", "tmp", "cabal"] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        let env = Env { root };
        // cabal's index, newer than the locks below (2026-10-01).
        env.write_cabal_index(1_800_000_000);
        env.hx()
            .current_dir(env.root.path())
            .args(["init", "--name", "app", "app"])
            .assert()
            .success();
        env
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    fn hx(&self) -> Command {
        let mut cmd = Command::cargo_bin("hx").unwrap();
        cmd.env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("mocks").display()),
            )
            .env("HOME", self.path("home"))
            .env("HX_HOME", self.path("hx-home"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("CABAL_DIR", self.path("cabal"))
            .env("TMPDIR", self.path("tmp"))
            .env("NO_COLOR", "1")
            .env("HX_NO_AUTO_INSTALL", "true")
            .env("MOCK_LOG", self.path("cabal.log"))
            .current_dir(self.path("app"));
        cmd
    }

    /// The cabal invocations for one subcommand (`build`, `test`, `run`).
    fn cabal_calls(&self, sub: &str) -> Vec<String> {
        fs::read_to_string(self.path("cabal.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.split_whitespace().any(|w| w == sub))
            .map(String::from)
            .collect()
    }

    /// cabal's `01-index.tar` with one entry dated `mtime` (what cabal's index state is read from).
    fn write_cabal_index(&self, mtime: u64) {
        let repo = self.path("cabal/packages/hackage.haskell.org");
        fs::create_dir_all(&repo).unwrap();
        let mut header = [0u8; 512];
        header[..13].copy_from_slice(b"pkg/1.0/a.tar");
        header[100..108].copy_from_slice(b"0000644\0");
        header[124..136].copy_from_slice(b"00000000000\0");
        header[136..148].copy_from_slice(format!("{mtime:011o}\0").as_bytes());
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].fill(b' ');
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut tar = header.to_vec();
        tar.extend_from_slice(&[0u8; 1024]);
        fs::write(repo.join("01-index.tar"), tar).unwrap();
    }

    fn write_lock(&self, body: &str) {
        fs::write(self.path("app/hx.lock"), body).unwrap();
    }

    fn add_dependency(&self, dep: &str) {
        let cabal = self.path("app/app.cabal");
        let content = fs::read_to_string(&cabal).unwrap();
        fs::write(&cabal, content.replacen("base", &format!("{dep}, base"), 1)).unwrap();
    }
}

fn write_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A current-format lock pinning `pkg-1.0`, resolved for GHC 9.8.2.
fn lock(compiler: &str) -> String {
    format!(
        "version = 2\ncreated_at = \"2026-10-01T12:00:00Z\"\n\n[toolchain]\nghc = \"9.8.2\"\n\n[plan]\n\
         compiler_id = \"{compiler}\"\nplatform = \"aarch64-macos\"\nindex_state = \"2026-10-01T00:00:00Z\"\n\n\
         [[packages]]\nname = \"pkg\"\nversion = \"1.0\"\nsource = \"hackage\"\n\
         sha256 = \"{}\"\n",
        hx_core::hash::sha256_hex(b"good")
    )
}

#[test]
fn build_uses_the_locked_plan() {
    let env = Env::new();
    env.write_lock(&lock("ghc-9.8.2"));
    env.hx().arg("build").assert().success();
    let calls = env.cabal_calls("build");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        calls[0].contains("--index-state=2026-10-01T00:00:00Z"),
        "{}",
        calls[0]
    );
    assert!(
        calls[0].contains("--constraint=any.pkg ==1.0"),
        "{}",
        calls[0]
    );
}

#[test]
fn test_and_run_use_the_locked_plan() {
    let env = Env::new();
    env.write_lock(&lock("ghc-9.8.2"));
    env.hx().arg("run").assert().success();
    let run = env.cabal_calls("run");
    assert!(
        run.iter().any(|c| c.contains("--constraint=any.pkg ==1.0")),
        "{run:?}"
    );
    // The arguments come before `--`, so they reach cabal, not the program.
    let line = &run[0];
    assert!(!line.contains("-- --constraint"), "{line}");
}

#[test]
fn build_without_a_lockfile_lets_cabal_plan() {
    let env = Env::new();
    env.hx().arg("build").assert().success();
    let calls = env.cabal_calls("build");
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].contains("--constraint"), "{}", calls[0]);
}

#[test]
fn stale_lockfile_warns_and_builds_without_its_pins() {
    let env = Env::new();
    env.write_lock(&lock("ghc-9.8.2"));
    env.add_dependency("text");
    env.hx()
        .arg("build")
        .assert()
        .success()
        .stderr(predicate::str::contains("hx.lock is out of date"))
        .stderr(predicate::str::contains("dependency `text`"))
        .stderr(predicate::str::contains("hx lock"));
    let calls = env.cabal_calls("build");
    assert!(!calls[0].contains("--constraint"), "{}", calls[0]);
}

#[test]
fn lock_for_another_ghc_is_stale() {
    let env = Env::new();
    env.write_lock(&lock("ghc-9.6.4"));
    env.hx()
        .arg("build")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "resolved for GHC 9.6.4, but GHC 9.8.2 is active",
        ));
    assert!(!env.cabal_calls("build")[0].contains("--constraint"));
}

#[test]
fn locked_requires_a_lockfile() {
    let env = Env::new();
    env.hx()
        .args(["build", "--locked"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--locked requires it"));
    assert!(env.cabal_calls("build").is_empty(), "cabal must not run");
}

#[test]
fn locked_refuses_a_stale_lockfile() {
    let env = Env::new();
    env.write_lock(&lock("ghc-9.8.2"));
    env.add_dependency("text");
    env.hx()
        .args(["test", "--locked"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("--locked requires a current one"));
    assert!(env.cabal_calls("test").is_empty(), "cabal must not run");
}

#[test]
fn hx_locked_env_var_is_the_same_as_the_flag() {
    let env = Env::new();
    env.hx().arg("build").env("HX_LOCKED", "1").assert().code(3);
}

#[test]
fn locked_refuses_a_cached_tarball_that_does_not_match() {
    let env = Env::new();
    env.write_lock(&lock("ghc-9.8.2"));
    let cached = env.path("cabal/packages/hackage.haskell.org/pkg/1.0/pkg-1.0.tar.gz");
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    fs::write(&cached, b"evil").unwrap();
    env.hx()
        .args(["build", "--locked"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("hash mismatch for pkg-1.0"));
    assert!(env.cabal_calls("build").is_empty(), "cabal must not run");
}

#[test]
fn locked_build_with_a_current_lockfile_succeeds() {
    let env = Env::new();
    env.write_lock(&lock("ghc-9.8.2"));
    env.hx().args(["build", "--locked"]).assert().success();
    assert!(env.cabal_calls("build")[0].contains("--constraint=any.pkg ==1.0"));
}

#[test]
fn an_outdated_cabal_index_is_updated_before_building() {
    let env = Env::new();
    env.write_cabal_index(1_700_000_000); // 2023: older than the lock's index state
    env.write_lock(&lock("ghc-9.8.2"));
    // The mock cabal "updates" but leaves the index as it is, so the build keeps
    // the versions and drops the index state it cannot honour.
    env.hx()
        .arg("build")
        .assert()
        .success()
        .stderr(predicate::str::contains("cabal's Hackage index is older"));
    assert_eq!(
        env.cabal_calls("update").len(),
        1,
        "cabal update must run once"
    );
    let build = &env.cabal_calls("build")[0];
    assert!(build.contains("--constraint=any.pkg ==1.0"), "{build}");
    assert!(!build.contains("--index-state"), "{build}");
}

#[test]
fn locked_fails_when_cabal_index_cannot_reach_the_lock() {
    let env = Env::new();
    env.write_cabal_index(1_700_000_000);
    env.write_lock(&lock("ghc-9.8.2"));
    env.hx()
        .args(["build", "--locked"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cabal update"));
    assert!(env.cabal_calls("build").is_empty(), "cabal must not build");
}
