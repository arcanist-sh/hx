//! Integration tests for `hx install` / `hx uninstall`.
//!
//! A mock `cabal` on PATH stands in for the real build: it "installs" a tiny
//! shell-script executable into the `--installdir` hx passes, so these tests
//! exercise hx's own work (argument construction, staging, collision checks,
//! moving into the bin directory, the manifest) without GHC or the network.
//! `HX_HOME` points the bin directory and manifest at a temp directory.

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
if [ "$1" = "--version" ]; then
  echo "cabal-install version 3.16.1.0"
  exit 0
fi
installdir=""
target=""
for a in "$@"; do
  case "$a" in
    --installdir=*) installdir="${a#--installdir=}" ;;
    -*|install) ;;
    *) target="$a" ;;
  esac
done
echo "Resolving dependencies..."
echo "Building     $target (exe:$MOCK_EXE)"
if [ -n "$MOCK_FAIL" ]; then
  echo "Error: [Cabal-7107] Could not resolve dependencies:" >&2
  exit 1
fi
printf '#!/bin/sh\necho "%s from %s"\n' "$MOCK_EXE" "$target" > "$installdir/$MOCK_EXE"
chmod +x "$installdir/$MOCK_EXE"
echo "Installing   $target"
"#;

const MOCK_GHC: &str = r#"#!/bin/sh
case "$1" in
  --numeric-version) echo "9.8.2" ;;
  *) echo "The Glorious Glasgow Haskell Compilation System, version 9.8.2" ;;
esac
"#;

/// A sandbox: mock toolchain on PATH, HX_HOME/HOME/caches in a temp dir.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let mocks = root.path().join("mocks");
        fs::create_dir_all(&mocks).unwrap();
        write_script(&mocks.join("cabal"), MOCK_CABAL);
        write_script(&mocks.join("ghc"), MOCK_GHC);
        for dir in ["home", "hx-home", "cache", "config", "tmp"] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        Self { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    fn bin(&self, exe: &str) -> PathBuf {
        self.path("hx-home/bin").join(exe)
    }

    fn manifest(&self) -> String {
        fs::read_to_string(self.path("hx-home/installed.toml")).unwrap_or_default()
    }

    fn cabal_log(&self) -> String {
        fs::read_to_string(self.path("cabal.log")).unwrap_or_default()
    }

    /// `hx` with a clean environment; `exe` is what the mock cabal builds.
    fn hx(&self, exe: &str) -> Command {
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
            .env("TMPDIR", self.path("tmp"))
            .env("NO_COLOR", "1")
            .env("MOCK_LOG", self.path("cabal.log"))
            .env("MOCK_EXE", exe)
            .current_dir(self.root.path());
        cmd
    }
}

fn write_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn install_moves_executable_into_bin_and_records_it() {
    let sb = Sandbox::new();

    sb.hx("hello")
        .args(["install", "hello@1.0.0.2"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Installed"));

    // The executable landed in the bin dir, runnable.
    let exe = sb.bin("hello");
    assert!(exe.exists(), "bin/hello missing");
    assert_ne!(fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0);
    assert!(
        fs::read_to_string(&exe)
            .unwrap()
            .contains("hello from hello-1.0.0.2")
    );

    // cabal was asked to copy the pinned version into a staging dir, with
    // the resolved compiler.
    let log = sb.cabal_log();
    let install_line = log.lines().find(|l| l.contains(" install ")).unwrap();
    assert!(
        install_line.contains("--install-method=copy"),
        "{install_line}"
    );
    assert!(
        install_line.contains("--overwrite-policy=always"),
        "{install_line}"
    );
    assert!(install_line.contains("--installdir="), "{install_line}");
    assert!(
        install_line.contains(&format!(
            "--with-compiler={}",
            sb.path("mocks/ghc").display()
        )),
        "{install_line}"
    );
    assert!(install_line.ends_with("hello-1.0.0.2"), "{install_line}");

    // Recorded in the manifest.
    let manifest = sb.manifest();
    assert!(manifest.contains("[packages.hello]"), "{manifest}");
    assert!(manifest.contains("version = \"1.0.0.2\""), "{manifest}");
    assert!(manifest.contains("executables = [\"hello\"]"), "{manifest}");
    assert!(manifest.contains("ghc = \"9.8.2\""), "{manifest}");
    assert!(manifest.contains("kind = \"hackage\""), "{manifest}");

    // The staging directory was cleaned up.
    let leftovers: Vec<_> = fs::read_dir(sb.path("hx-home"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".install-"))
        .collect();
    assert!(leftovers.is_empty(), "staging dir left behind");

    // Listed.
    sb.hx("hello")
        .args(["install", "--list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "hello 1.0.0.2 (hackage, ghc 9.8.2):",
        ))
        .stdout(predicate::str::contains("    hello"));
}

#[test]
fn reinstalling_same_version_is_skipped_unless_forced() {
    let sb = Sandbox::new();
    sb.hx("hello")
        .args(["install", "hello@1.0.0.2"])
        .assert()
        .success();
    let builds = || sb.cabal_log().matches(" install ").count();
    assert_eq!(builds(), 1);

    sb.hx("hello")
        .args(["install", "hello@1.0.0.2"])
        .assert()
        .success()
        .stderr(predicate::str::contains("already installed"));
    assert_eq!(builds(), 1, "an up-to-date install must not rebuild");

    sb.hx("hello")
        .args(["install", "hello@1.0.0.2", "--force"])
        .assert()
        .success();
    assert_eq!(builds(), 2);
}

#[test]
fn collision_with_another_package_is_refused_then_forced() {
    let sb = Sandbox::new();
    sb.hx("hello")
        .args(["install", "hello@1.0.0.2"])
        .assert()
        .success();
    let original = fs::read_to_string(sb.bin("hello")).unwrap();

    // `greet` also wants to install an executable called `hello`.
    sb.hx("hello")
        .args(["install", "greet@2.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("would overwrite"))
        .stderr(predicate::str::contains("hello (installed by hello)"))
        .stderr(predicate::str::contains("hx uninstall hello"));
    assert_eq!(fs::read_to_string(sb.bin("hello")).unwrap(), original);
    assert!(!sb.manifest().contains("[packages.greet]"));

    sb.hx("hello")
        .args(["install", "greet@2.0", "--force"])
        .assert()
        .success()
        .stderr(predicate::str::contains("previously installed by hello"));
    assert!(
        fs::read_to_string(sb.bin("hello"))
            .unwrap()
            .contains("greet-2.0")
    );
    let manifest = sb.manifest();
    assert!(manifest.contains("[packages.greet]"), "{manifest}");
    assert!(
        !manifest.contains("[packages.hello]"),
        "hello lost its only executable, so it is no longer installed: {manifest}"
    );
}

#[test]
fn collision_with_unmanaged_file_is_refused() {
    let sb = Sandbox::new();
    // e.g. the hx-managed GHC symlink lives in the same bin directory.
    fs::create_dir_all(sb.path("hx-home/bin")).unwrap();
    fs::write(sb.bin("ghc"), "not ours").unwrap();

    sb.hx("ghc")
        .args(["install", "ghc-wrapper@1.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not installed by `hx install`"));
    assert_eq!(fs::read_to_string(sb.bin("ghc")).unwrap(), "not ours");
}

#[test]
fn uninstall_removes_exactly_what_was_installed() {
    let sb = Sandbox::new();
    sb.hx("hello")
        .args(["install", "hello@1.0.0.2"])
        .assert()
        .success();
    fs::write(sb.bin("unrelated"), "keep me").unwrap();

    sb.hx("hello")
        .args(["uninstall", "hello"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Removed"));
    assert!(!sb.bin("hello").exists());
    assert!(sb.bin("unrelated").exists());
    assert!(!sb.manifest().contains("[packages.hello]"));

    sb.hx("hello")
        .args(["uninstall", "hello"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not installed"));
}

#[test]
fn install_from_local_path() {
    let sb = Sandbox::new();
    let project = sb.path("demo");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("demo.cabal"),
        "cabal-version: 3.0\nname: demo\nversion: 0.3.0\n\nexecutable demo\n  main-is: Main.hs\n",
    )
    .unwrap();

    sb.hx("demo")
        .args(["install", "--path"])
        .arg(&project)
        .assert()
        .success();

    assert!(sb.bin("demo").exists());
    let install_line = sb
        .cabal_log()
        .lines()
        .find(|l| l.contains(" install "))
        .unwrap()
        .to_string();
    assert!(install_line.ends_with(" demo"), "{install_line}");
    let manifest = sb.manifest();
    assert!(manifest.contains("[packages.demo]"), "{manifest}");
    assert!(manifest.contains("version = \"0.3.0\""), "{manifest}");
    assert!(manifest.contains("kind = \"path\""), "{manifest}");
}

#[test]
fn build_failure_exits_5_and_installs_nothing() {
    let sb = Sandbox::new();
    sb.hx("hello")
        .env("MOCK_FAIL", "1")
        .args(["install", "hello@1.0.0.2"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("Could not resolve dependencies"));
    assert!(!sb.bin("hello").exists());
    assert!(sb.manifest().is_empty());
}

#[test]
fn missing_cabal_is_a_toolchain_error() {
    let sb = Sandbox::new();
    fs::remove_file(sb.path("mocks/cabal")).unwrap();
    sb.hx("hello")
        .args(["install", "hello@1.0.0.2"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("hx toolchain install --cabal"));
}

#[test]
fn invalid_package_spec_is_a_usage_error() {
    let sb = Sandbox::new();
    sb.hx("hello")
        .args(["install", "hello@latest"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not a valid version"));
}

#[test]
fn list_with_nothing_installed() {
    let sb = Sandbox::new();
    sb.hx("hello")
        .args(["install", "--list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("No packages installed"));
}
