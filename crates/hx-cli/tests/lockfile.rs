//! Integration tests for reproducible lockfiles: `hx lock --check`,
//! `hx fetch` hash verification and `hx sync --offline` preflight.
//!
//! Everything runs against a throwaway HOME/CABAL_DIR and a local `file://`
//! Hackage mirror, so no test touches the network.
//!
//! Run with: cargo test -p hx-cli --test lockfile

#![cfg(not(windows))]

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn sha256(bytes: &[u8]) -> String {
    hx_core::hash::sha256_hex(bytes)
}

/// A project plus isolated home, cabal dir and Hackage mirror.
struct Env {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
    cabal_dir: PathBuf,
    mirror: PathBuf,
}

impl Env {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let env = Env {
            project: root.join("app"),
            home: root.join("home"),
            cabal_dir: root.join("cabal"),
            mirror: root.join("mirror"),
            _temp: temp,
        };
        fs::create_dir_all(&env.home).unwrap();
        // An (empty) cabal index, so nothing tries to run `cabal update`.
        let repo = env.cabal_repo();
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("01-index.tar"), [0u8; 1024]).unwrap();

        env.hx()
            .current_dir(&root)
            .args(["init", "--name", "app"])
            .arg(&env.project)
            .assert()
            .success();
        env
    }

    fn cabal_repo(&self) -> PathBuf {
        self.cabal_dir.join("packages").join("hackage.haskell.org")
    }

    fn hx(&self) -> Command {
        let mut cmd = Command::cargo_bin("hx").unwrap();
        cmd.current_dir(&self.project)
            .env("HOME", &self.home)
            .env("CABAL_DIR", &self.cabal_dir)
            .env(
                "HX_HACKAGE_URL",
                format!("file://{}", self.mirror.display()),
            )
            .env_remove("XDG_CACHE_HOME")
            .env_remove("HX_OFFLINE")
            .env("HX_NO_AUTO_INSTALL", "true");
        cmd
    }

    /// Publish `name-version` with `content` on the local mirror.
    fn publish(&self, name: &str, version: &str, content: &[u8]) {
        let id = format!("{name}-{version}");
        let dir = self.mirror.join("package").join(&id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{id}.tar.gz")), content).unwrap();
    }

    fn write_lock(&self, body: &str) {
        fs::write(self.project.join("hx.lock"), body).unwrap();
    }

    fn hx_cached(&self, id: &str) -> PathBuf {
        self.home
            .join(".cache")
            .join("hx")
            .join("packages")
            .join(format!("{id}.tar.gz"))
    }
}

fn v2_lock(packages: &[(&str, &str, &str)]) -> String {
    let mut s = String::from(
        "version = 2\ncreated_at = \"2026-10-01T12:00:00Z\"\n\n[toolchain]\n\n[plan]\n\
         platform = \"aarch64-macos\"\n",
    );
    for (name, version, sha) in packages {
        s.push_str(&format!(
            "\n[[packages]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"hackage\"\n\
             sha256 = \"{sha}\"\n"
        ));
    }
    s
}

fn exists(p: &Path) -> bool {
    p.is_file()
}

#[test]
fn test_fetch_downloads_verifies_and_seeds_cabal_cache() {
    let env = Env::new();
    env.publish("pkg", "1.0", b"pkg sdist");
    env.write_lock(&v2_lock(&[("pkg", "1.0", &sha256(b"pkg sdist"))]));

    env.hx()
        .arg("fetch")
        .assert()
        .success()
        .stderr(predicate::str::contains("Ready for offline builds"));

    assert!(exists(&env.hx_cached("pkg-1.0")));
    let seeded = env.cabal_repo().join("pkg/1.0/pkg-1.0.tar.gz");
    assert_eq!(fs::read(seeded).unwrap(), b"pkg sdist");
}

#[test]
fn test_fetch_rejects_tampered_tarball() {
    let env = Env::new();
    env.publish("pkg", "1.0", b"tampered sdist");
    env.write_lock(&v2_lock(&[("pkg", "1.0", &sha256(b"pkg sdist"))]));

    env.hx()
        .arg("fetch")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("hash mismatch for pkg-1.0"))
        .stderr(predicate::str::contains(sha256(b"pkg sdist")))
        .stderr(predicate::str::contains(sha256(b"tampered sdist")))
        .stderr(predicate::str::contains("tampered"));

    // The unverified download is not kept anywhere.
    assert!(!exists(&env.hx_cached("pkg-1.0")));
    assert!(!exists(&env.cabal_repo().join("pkg/1.0/pkg-1.0.tar.gz")));
}

#[test]
fn test_sync_offline_lists_missing_sources() {
    let env = Env::new();
    env.write_lock(&v2_lock(&[
        ("pkg", "1.0", &sha256(b"a")),
        ("other", "2.3", &sha256(b"b")),
        // Compiler-internal packages never need an sdist.
        ("base", "4.19.1.0", &sha256(b"c")),
    ]));

    env.hx()
        .args(["sync", "--offline"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "2 locked source packages are not available offline",
        ))
        .stderr(predicate::str::contains("other-2.3"))
        .stderr(predicate::str::contains("pkg-1.0"))
        .stderr(predicate::str::contains("base-4.19.1.0").not())
        .stderr(predicate::str::contains("hx fetch"));
}

#[test]
fn test_sync_offline_via_env_var() {
    let env = Env::new();
    env.write_lock(&v2_lock(&[("pkg", "1.0", &sha256(b"a"))]));
    env.hx()
        .arg("sync")
        .env("HX_OFFLINE", "1")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not available offline"));
}

#[test]
fn test_sync_offline_rejects_v1_lockfile() {
    let env = Env::new();
    env.write_lock(
        "version = 1\ncreated_at = \"2024-01-15T10:30:00Z\"\n\n[[packages]]\nname = \"pkg\"\n\
         version = \"1.0\"\n",
    );
    env.hx()
        .args(["sync", "--offline"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("format v1"))
        .stderr(predicate::str::contains("hx lock"));
}

#[test]
fn test_sync_rejects_mismatched_cabal_cache() {
    let env = Env::new();
    let cached = env.cabal_repo().join("pkg/1.0/pkg-1.0.tar.gz");
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    fs::write(&cached, b"evil").unwrap();
    env.write_lock(&v2_lock(&[("pkg", "1.0", &sha256(b"good"))]));

    // Even online, a known-bad cached tarball is never built.
    env.hx()
        .arg("sync")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("hash mismatch for pkg-1.0"));
}

#[test]
fn test_lock_check_passes_for_consistent_lock() {
    let env = Env::new();
    env.write_lock(&v2_lock(&[]));
    env.hx()
        .args(["lock", "--check"])
        .assert()
        .success()
        .stderr(predicate::str::contains("is consistent"));
}

#[test]
fn test_lock_check_fails_on_v1_lockfile() {
    let env = Env::new();
    env.write_lock("version = 1\ncreated_at = \"2024-01-15T10:30:00Z\"\n");
    env.hx()
        .args(["lock", "--check"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("format v1"));
}

#[test]
fn test_lock_check_fails_on_missing_dependency() {
    let env = Env::new();
    let cabal = env.project.join("app.cabal");
    let content = fs::read_to_string(&cabal).unwrap();
    let content = content.replacen("base", "text, base", 1);
    fs::write(&cabal, content).unwrap();
    env.write_lock(&v2_lock(&[]));

    env.hx()
        .args(["lock", "--check"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("dependency `text`"))
        .stderr(predicate::str::contains("hx lock"));
}

#[test]
fn test_lock_check_fails_on_cached_hash_mismatch() {
    let env = Env::new();
    let cached = env.hx_cached("pkg-1.0");
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    fs::write(&cached, b"corrupt").unwrap();
    env.write_lock(&v2_lock(&[("pkg", "1.0", &sha256(b"good"))]));

    env.hx()
        .args(["lock", "--check"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("hash mismatch for pkg-1.0"));
}

#[test]
fn test_lock_check_rejects_newer_format() {
    let env = Env::new();
    env.write_lock("version = 99\ncreated_at = \"2024-01-15T10:30:00Z\"\n");
    env.hx()
        .args(["lock", "--check"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("upgrade hx"));
}
