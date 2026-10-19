//! Integration tests for `hx registry`, `hx search --registry` and
//! `hx publish --registry`, against a local mock HTTP server. hx writes its
//! human-readable output (`Output`) to stderr.

#![cfg(not(windows))]
#![allow(deprecated)] // cargo_bin is deprecated but the replacement requires macros

use assert_cmd::Command;
use hx_registry::mock::{MockResponse, MockServer};
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

const CONFIG: &str = r#"{"name":"registry.test","url":"http://x","api_version":1,
  "tuf":{"enabled":true,"root_keys":["aaaa","bbbb","cccc"],"key_threshold":2}}"#;

/// `hx` with config isolated in `cfg` (credentials land in `$XDG_CONFIG_HOME/hx`).
fn hx(cfg: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hx").unwrap();
    cmd.env("XDG_CONFIG_HOME", cfg.path())
        .env("XDG_CACHE_HOME", cfg.path().join("cache"))
        .env("NO_COLOR", "1")
        .env_remove("HX_REGISTRY_TOKEN");
    cmd
}

#[test]
fn test_registry_login_verifies_and_stores_token_privately() {
    let cfg = TempDir::new().unwrap();
    let server = MockServer::start(vec![MockResponse::json(
        "GET",
        "/api/v1/me",
        200,
        r#"{"login":"zara","admin":false,"token":{"id":"tok_1","scopes":["publish"]}}"#,
    )]);
    hx(&cfg)
        .args([
            "registry",
            "login",
            "--registry",
            &server.url(),
            "--token-stdin",
        ])
        .write_stdin("hxr_secret\n")
        .assert()
        .success();
    let me = &server.requests()[0];
    assert_eq!(
        me.header("authorization").as_deref(),
        Some("Bearer hxr_secret")
    );

    let creds = cfg.path().join("hx").join("credentials.toml");
    let text = fs::read_to_string(&creds).unwrap();
    assert!(text.contains("hxr_secret"));
    assert!(text.contains(&server.url()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&creds).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    hx(&cfg)
        .args(["registry", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains(server.url()).and(predicate::str::contains("logged in")));

    hx(&cfg)
        .args(["registry", "logout", "--registry", &server.url()])
        .assert()
        .success();
    assert!(!fs::read_to_string(&creds).unwrap().contains("hxr_secret"));
}

#[test]
fn test_registry_login_rejects_bad_token_and_stores_nothing() {
    let cfg = TempDir::new().unwrap();
    let server = MockServer::start(vec![MockResponse::json(
        "GET",
        "/api/v1/me",
        401,
        r#"{"error":{"code":"invalid_token","message":"API token is invalid, expired or revoked"}}"#,
    )]);
    hx(&cfg)
        .args([
            "registry",
            "login",
            "--registry",
            &server.url(),
            "--token-stdin",
        ])
        .write_stdin("hxr_bad\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid, expired or revoked"));
    assert!(!cfg.path().join("hx").join("credentials.toml").exists());
}

#[test]
fn test_search_with_registry_uses_json_api() {
    let cfg = TempDir::new().unwrap();
    let server = MockServer::start(vec![MockResponse::json(
        "GET",
        "/api/v1/search?q=hello&limit=10",
        200,
        r#"{"query":"hello","results":[{"name":"arcanist-hello","synopsis":"Says hello","latest":"0.1.0"}]}"#,
    )]);
    hx(&cfg)
        .args(["search", "hello", "--registry", &server.url()])
        .assert()
        .success()
        .stderr(predicate::str::contains("arcanist-hello").and(predicate::str::contains("0.1.0")));
}

#[test]
fn test_registry_by_name_from_global_config() {
    let cfg = TempDir::new().unwrap();
    let server = MockServer::start(vec![MockResponse::json(
        "GET",
        "/api/v1/packages/foo",
        200,
        r#"{"name":"foo","synopsis":"","latest":"1.0","owners":["zara"],
            "versions":[{"version":"1.0","yanked":true,"published_at":1,"sha256":"x","size":1,"bhc_verified":true}]}"#,
    )]);
    fs::create_dir_all(cfg.path().join("hx")).unwrap();
    fs::write(
        cfg.path().join("hx").join("config.toml"),
        format!("[registries.local]\nurl = \"{}\"\n", server.url()),
    )
    .unwrap();
    hx(&cfg)
        .args(["registry", "info", "foo", "--registry", "local"])
        .assert()
        .success()
        .stderr(predicate::str::contains("yanked, BHC-verified"));
}

#[test]
fn test_registry_add_writes_cabal_repository_stanza() {
    let cfg = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::write(project.path().join("cabal.project"), "packages: .\n").unwrap();
    let server = MockServer::start(vec![MockResponse::json(
        "GET",
        "/api/v1/config",
        200,
        CONFIG,
    )]);
    for _ in 0..2 {
        hx(&cfg)
            .args(["registry", "add", "--registry", &server.url(), "--dir"])
            .arg(project.path())
            .assert()
            .success();
    }
    let text = fs::read_to_string(project.path().join("cabal.project")).unwrap();
    let name = format!(
        "registry-127.0.0.1-{}",
        server.url().rsplit(':').next().unwrap()
    );
    assert_eq!(text.matches("repository ").count(), 1, "{text}");
    assert!(text.starts_with("packages: .\n\nrepository "));
    assert!(text.contains(&format!(
        "repository {name}\n  url: {}/\n  secure: True\n",
        server.url()
    )));
    assert!(
        text.contains(
            "  root-keys: aaaa\n             bbbb\n             cccc\n  key-threshold: 2\n"
        )
    );
}

#[test]
fn test_unknown_registry_is_a_config_error() {
    let cfg = TempDir::new().unwrap();
    hx(&cfg)
        .args(["search", "x", "--registry", "nope"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("unknown registry `nope`"))
        .stderr(predicate::str::contains("[registries.nope]"));
}

#[test]
fn test_publish_to_registry_requires_login_before_checks() {
    let cfg = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("hx.toml"),
        "[project]\nname = \"demo\"\n",
    )
    .unwrap();
    fs::write(
        project.path().join("demo.cabal"),
        "name: demo\nversion: 0.1\n",
    )
    .unwrap();
    hx(&cfg)
        .current_dir(project.path())
        .args(["publish", "--registry", "http://127.0.0.1:9"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "not logged in to http://127.0.0.1:9",
        ))
        .stderr(predicate::str::contains("hx registry login"));
}
