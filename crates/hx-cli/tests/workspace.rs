//! Integration tests for multi-package workspaces.
//!
//! Builds run against a mock `cabal` (and `ghc`) placed first on PATH that
//! records every invocation, so the tests assert exactly which targets hx asks
//! Cabal to build or test without needing a real toolchain or the network.

#![cfg(unix)]
#![allow(deprecated)] // cargo_bin is deprecated but the replacement requires macros

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A 3-member workspace (lib-a; lib-b depends on lib-a; app depends on
/// lib-b) plus a sandboxed HOME and a bin directory with mock tools.
struct Fixture {
    _tmp: TempDir,
    home: PathBuf,
    bin: PathBuf,
    root: PathBuf,
    log: PathBuf,
}

const MOCK_CABAL: &str = r#"#!/bin/sh
case "$1" in
  --version|--numeric-version) echo "cabal-install version 3.12.1.0"; exit 0 ;;
esac
# Drop the global --store-dir and the --builddir so the log shows the command
# and its targets.
args=""
for a in "$@"; do
  case "$a" in
    --store-dir=*|--builddir=*) ;;
    *) args="$args $a" ;;
  esac
done
echo "$PWD |$args" >> "$MOCK_CABAL_LOG"
exit 0
"#;

const MOCK_GHC: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "The Glorious Glasgow Haskell Compilation System, version 9.8.2" ;;
  --numeric-version) echo "9.8.2" ;;
esac
exit 0
"#;

fn write_exe(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn write_member(root: &Path, name: &str, body: &str) {
    let dir = root.join(name);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join(format!("{name}.cabal")),
        format!("cabal-version: 3.0\nname: {name}\nversion: 0.1.0\n\n{body}"),
    )
    .unwrap();
    fs::write(dir.join("src").join("Lib.hs"), "module Lib where\n").unwrap();
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        // Canonicalize so paths match what the hx process sees as its cwd
        // (macOS temp dirs live behind a /var -> /private/var symlink).
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let bin = base.join("bin");
        let root = base.join("ws");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&root).unwrap();
        write_exe(&bin.join("cabal"), MOCK_CABAL);
        write_exe(&bin.join("ghc"), MOCK_GHC);

        write_member(&root, "lib-a", "library\n  build-depends: base\n");
        write_member(
            &root,
            "lib-b",
            "library\n  build-depends: base, lib-a\n\ntest-suite lib-b-test\n  type: exitcode-stdio-1.0\n  main-is: Spec.hs\n  build-depends: base, lib-b\n",
        );
        write_member(
            &root,
            "app",
            "executable app\n  main-is: Main.hs\n  build-depends: base, lib-b\n",
        );

        let log = base.join("cabal.log");
        Fixture {
            _tmp: tmp,
            home,
            bin,
            root,
            log,
        }
    }

    /// An `hx` command sandboxed to this fixture, run from `dir`.
    fn hx_in(&self, dir: &Path) -> Command {
        let mut cmd = Command::cargo_bin("hx").unwrap();
        cmd.current_dir(dir)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("MOCK_CABAL_LOG", &self.log)
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .arg("--no-auto-install");
        cmd
    }

    fn hx(&self) -> Command {
        self.hx_in(&self.root)
    }

    /// `hx workspace init` + add all three members.
    fn init(&self) {
        self.hx().args(["workspace", "init"]).assert().success();
    }

    /// The recorded cabal invocations (`<cwd> | <command> <targets…>`).
    fn cabal_calls(&self) -> Vec<String> {
        let calls = fs::read_to_string(&self.log).unwrap_or_default();
        let _ = fs::remove_file(&self.log);
        calls
            .lines()
            .map(|l| {
                let (cwd, args) = l.split_once(" | ").unwrap_or(("", l));
                let cwd = Path::new(cwd)
                    .strip_prefix(&self.root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| cwd.to_string());
                format!("[{cwd}] {}", args.trim())
            })
            .collect()
    }

    fn git(&self, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=hx", "-c", "user.email=hx@example.com"])
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    fn commit_all(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "snapshot"]);
    }
}

#[test]
fn workspace_init_adopts_existing_members() {
    let fx = Fixture::new();
    fx.init();

    let cabal_project = fs::read_to_string(fx.root.join("cabal.project")).unwrap();
    assert!(cabal_project.contains("packages:\n  ./app\n  ./lib-a\n  ./lib-b\n"));
    let manifest = fs::read_to_string(fx.root.join("hx.toml")).unwrap();
    assert!(manifest.contains("name = \"ws\""));

    // Re-running keeps both files.
    fs::write(fx.root.join("cabal.project"), "packages: ./lib-a\n").unwrap();
    fx.init();
    assert_eq!(
        fs::read_to_string(fx.root.join("cabal.project")).unwrap(),
        "packages: ./lib-a\n"
    );
}

#[test]
fn workspace_add_rm_and_list() {
    let fx = Fixture::new();
    fs::write(fx.root.join("hx.toml"), "[project]\nname = \"ws\"\n").unwrap();
    fs::write(
        fx.root.join("cabal.project"),
        "-- members\npackages: ./lib-a\n\nwith-compiler: ghc-9.8.2\n",
    )
    .unwrap();

    fx.hx()
        .args(["workspace", "add", "lib-b", "./app/"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(fx.root.join("cabal.project")).unwrap(),
        "-- members\npackages:\n  ./lib-a\n  ./lib-b\n  ./app\n\nwith-compiler: ghc-9.8.2\n"
    );

    // Adding again is a no-op.
    fx.hx()
        .args(["workspace", "add", "lib-b"])
        .assert()
        .success()
        .stderr(predicate::str::contains("already a workspace member"));

    fx.hx()
        .args(["workspace", "list"])
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"NAME\s+VERSION\s+PATH\s+DEPENDS ON").unwrap())
        .stdout(predicate::str::is_match(r"lib-b\s+0\.1\.0\s+lib-b\s+lib-a").unwrap())
        .stdout(predicate::str::is_match(r"app\s+0\.1\.0\s+app\s+lib-b").unwrap());

    let out = fx
        .hx()
        .args(["workspace", "list", "--json"])
        .output()
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let members = doc["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[2]["name"], "app");
    assert_eq!(members[2]["local_deps"], serde_json::json!(["lib-b"]));
    assert_eq!(members[2]["executables"], serde_json::json!(["app"]));

    // Remove by name; a dependent member triggers a warning.
    fx.hx()
        .args(["workspace", "rm", "lib-b"])
        .assert()
        .success()
        .stderr(predicate::str::contains("app depend(s) on lib-b"));
    assert_eq!(
        fs::read_to_string(fx.root.join("cabal.project")).unwrap(),
        "-- members\npackages:\n  ./lib-a\n  ./app\n\nwith-compiler: ghc-9.8.2\n"
    );
}

#[test]
fn workspace_add_errors() {
    let fx = Fixture::new();
    fs::write(fx.root.join("hx.toml"), "[project]\nname = \"ws\"\n").unwrap();

    // No cabal.project yet: point at `hx workspace init`.
    fx.hx()
        .args(["workspace", "add", "lib-a"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("hx workspace init"));

    fs::write(fx.root.join("cabal.project"), "packages:\n").unwrap();
    fs::create_dir_all(fx.root.join("empty")).unwrap();
    fx.hx()
        .args(["workspace", "add", "empty"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no .cabal file"))
        .stderr(predicate::str::contains("hx init --lib empty"));
    fx.hx()
        .args(["workspace", "add", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn build_at_root_builds_all_members() {
    let fx = Fixture::new();
    fx.init();
    fx.hx().arg("build").assert().success();
    assert_eq!(fx.cabal_calls(), vec!["[] build all"]);
}

#[test]
fn build_package_flags_select_members() {
    let fx = Fixture::new();
    fx.init();
    fx.hx()
        .args(["build", "-p", "app", "-p", "lib-a"])
        .assert()
        .success();
    // Workspace order, not flag order.
    assert_eq!(fx.cabal_calls(), vec!["[] build app lib-a"]);

    fx.hx()
        .args(["build", "-p", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Package 'nope' not found in workspace",
        ));
    assert!(fx.cabal_calls().is_empty());
}

#[test]
fn commands_inside_a_member_target_that_member_from_the_root() {
    let fx = Fixture::new();
    fx.init();
    let member_src = fx.root.join("lib-b").join("src");

    fx.hx_in(&member_src).arg("build").assert().success();
    fx.hx_in(&member_src).arg("check").assert().success();
    fx.hx_in(&member_src).arg("test").assert().success();
    // cabal runs at the workspace root (shared build dir, lockfile and
    // toolchain), targeting only the member.
    assert_eq!(
        fx.cabal_calls(),
        vec!["[] build lib-b", "[] build lib-b", "[] test lib-b"]
    );

    // Same when the member carries its own hx.toml.
    fs::write(
        fx.root.join("lib-b").join("hx.toml"),
        "[project]\nname = \"lib-b\"\n",
    )
    .unwrap();
    fx.hx_in(&member_src).arg("build").assert().success();
    assert_eq!(fx.cabal_calls(), vec!["[] build lib-b"]);
}

#[test]
fn test_skips_members_without_test_suites() {
    let fx = Fixture::new();
    fx.init();
    fx.hx().arg("test").assert().success();
    assert_eq!(fx.cabal_calls(), vec!["[] test lib-b"]);

    fx.hx()
        .args(["test", "--package", "lib-a"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No test suites"));
    assert!(fx.cabal_calls().is_empty());
}

#[test]
fn run_picks_the_only_executable_member() {
    let fx = Fixture::new();
    fx.init();
    fx.hx().arg("run").assert().success();
    assert_eq!(fx.cabal_calls(), vec!["[] run app"]);
}

#[test]
fn changed_selects_affected_members_and_reverse_deps() {
    let fx = Fixture::new();
    fx.init();
    fx.git(&["init", "-q"]);
    fx.commit_all();

    // Nothing changed: nothing to build.
    fx.hx()
        .args(["build", "--changed"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No workspace members affected"));
    assert!(fx.cabal_calls().is_empty());

    // A change in lib-b affects lib-b and app (which depends on it), not lib-a.
    fs::write(
        fx.root.join("lib-b/src/Lib.hs"),
        "module Lib where\nx = 1\n",
    )
    .unwrap();
    fx.hx().args(["build", "--changed"]).assert().success();
    assert_eq!(fx.cabal_calls(), vec!["[] build app lib-b"]);
    fx.hx()
        .args(["workspace", "affected"])
        .assert()
        .success()
        .stdout("app\nlib-b\n");

    // Only lib-b of the affected members has a test suite.
    fx.hx().args(["test", "--changed"]).assert().success();
    assert_eq!(fx.cabal_calls(), vec!["[] test lib-b"]);

    // Against a branch base: committed changes since the fork point count,
    // and an untracked file in lib-a pulls in everything.
    fx.git(&["branch", "base"]);
    fx.commit_all();
    fs::write(fx.root.join("lib-a/src/New.hs"), "module New where\n").unwrap();
    fx.hx()
        .args(["build", "--changed", "base"])
        .assert()
        .success();
    assert_eq!(fx.cabal_calls(), vec!["[] build all"]);

    // A workspace-wide file affects every member.
    fx.commit_all();
    fs::write(
        fx.root.join("cabal.project"),
        fs::read_to_string(fx.root.join("cabal.project")).unwrap() + "\ntests: True\n",
    )
    .unwrap();
    fx.hx().args(["build", "--changed"]).assert().success();
    assert_eq!(fx.cabal_calls(), vec!["[] build all"]);
}

#[test]
fn changed_outside_git_explains_the_fix() {
    let fx = Fixture::new();
    fx.init();
    fx.hx()
        .args(["build", "--changed"])
        .env("GIT_CEILING_DIRECTORIES", fx.root.parent().unwrap())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--changed"));
    assert!(fx.cabal_calls().is_empty());
}
