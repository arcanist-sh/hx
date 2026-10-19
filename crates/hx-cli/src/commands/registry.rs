//! `hx registry` and the registry paths of `hx publish` / `hx search`.

use crate::cli::{RegistryCommands, RegistryOwnerCommands, RegistryTokenCommands};
use anyhow::Result;
use hx_config::{Manifest, find_project_root, load_global_config};
use hx_registry::{
    Credentials, Registry, RegistryClient, RegistryError, RepositoryStanza, TOKEN_ENV,
    known_registries, resolve_registry, upsert_repository_stanza,
};
use hx_ui::{Output, Spinner};
use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal};
use std::path::Path;

type RegistryMap = BTreeMap<String, hx_config::RegistryConfig>;

/// `[registries]` from the current project's hx.toml (if any) and the global config.
fn configured_registries() -> (Option<RegistryMap>, Option<RegistryMap>) {
    let project = find_project_root(".")
        .ok()
        .map(|root| root.join(hx_config::MANIFEST_FILENAME))
        .filter(|p| p.exists())
        .and_then(|p| Manifest::from_file(p).ok())
        .map(|m| m.registries);
    let global = load_global_config().ok().flatten().map(|g| g.registries);
    (project, global)
}

/// Resolve `--registry <name|url>`.
pub fn resolve(spec: &str) -> Result<Registry, RegistryError> {
    let (project, global) = configured_registries();
    resolve_registry(spec, project.as_ref(), global.as_ref())
}

/// A client for `registry`, with its stored token (if any).
pub fn client(registry: &Registry) -> Result<RegistryClient, RegistryError> {
    let token = Credentials::load()?.token_for(&registry.url);
    RegistryClient::new(&registry.url, token)
}

/// Print a registry error in hx's error format; returns the exit code.
pub fn report(err: &RegistryError, output: &Output) -> i32 {
    output.error(&err.to_string());
    if let RegistryError::UnknownRegistry { known, .. } = err {
        output.info(&format!("  known registries: {}", known.join(", ")));
    }
    if let Some(fix) = err.fix() {
        eprintln!();
        eprintln!("fix: {fix}");
    }
    match err {
        RegistryError::UnknownRegistry { .. }
        | RegistryError::InvalidUrl { .. }
        | RegistryError::NotLoggedIn { .. }
        | RegistryError::Parse { .. } => 3,
        _ => 1,
    }
}

/// Turn a registry result into an exit code, reporting errors.
fn finish(r: Result<(), RegistryError>, output: &Output) -> Result<i32> {
    Ok(match r {
        Ok(()) => 0,
        Err(e) => report(&e, output),
    })
}

/// Run `hx registry <command>`.
pub async fn run(command: RegistryCommands, output: &Output) -> Result<i32> {
    let r = match command {
        RegistryCommands::Login {
            registry,
            token_stdin,
        } => login(&registry.registry, token_stdin, output).await,
        RegistryCommands::Logout { registry, revoke } => {
            logout(&registry.registry, revoke, output).await
        }
        RegistryCommands::List => list(output),
        RegistryCommands::Add { registry, dir } => add(&registry.registry, &dir, output).await,
        RegistryCommands::Info { package, registry } => {
            info(&registry.registry, &package, output).await
        }
        RegistryCommands::Yank {
            package,
            pkg_version,
            undo,
            registry,
        } => yank(&registry.registry, &package, &pkg_version, !undo, output).await,
        RegistryCommands::Owner { command } => owner(command, output).await,
        RegistryCommands::Token { command } => token(command, output).await,
    };
    finish(r, output)
}

fn read_token(token_stdin: bool) -> Result<String, RegistryError> {
    let io = |source| RegistryError::Io {
        action: "read the token from",
        path: "stdin".into(),
        source,
    };
    let stdin = std::io::stdin();
    let token = if token_stdin || !stdin.is_terminal() {
        let mut line = String::new();
        stdin.lock().read_line(&mut line).map_err(io)?;
        line
    } else {
        eprint!("API token (input hidden): ");
        console::Term::stderr().read_secure_line().map_err(io)?
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(io(std::io::Error::other("no token given")));
    }
    Ok(token)
}

async fn login(spec: &str, token_stdin: bool, output: &Output) -> Result<(), RegistryError> {
    let registry = resolve(spec)?;
    let token = read_token(token_stdin)?;
    let spinner = Spinner::new(format!("Verifying token with {}...", registry.url));
    let me = match RegistryClient::new(&registry.url, Some(token.clone()))?
        .me()
        .await
    {
        Ok(me) => me,
        Err(e) => {
            spinner.finish_error("Token rejected");
            return Err(e);
        }
    };
    spinner.finish_success(format!("Logged in as {}", me.login));
    let mut creds = Credentials::load()?;
    creds.set_token(&registry.url, &token);
    creds.save()?;
    output.status("Saved", &format!("token to {}", creds.path().display()));
    Ok(())
}

async fn logout(spec: &str, revoke: bool, output: &Output) -> Result<(), RegistryError> {
    let registry = resolve(spec)?;
    let mut creds = Credentials::load()?;
    let Some(token) = creds.stored_token(&registry.url) else {
        output.info(&format!("Not logged in to {}", registry.url));
        return Ok(());
    };
    if revoke {
        let client = RegistryClient::new(&registry.url, Some(token))?;
        let id = client.me().await?.token.id;
        client.revoke_token(&id).await?;
        output.status("Revoked", &format!("token {id} on {}", registry.url));
    }
    creds.remove(&registry.url);
    creds.save()?;
    output.status("Logged out", &registry.url);
    Ok(())
}

fn list(output: &Output) -> Result<(), RegistryError> {
    let (project, global) = configured_registries();
    let creds = Credentials::load()?;
    output.header("Registries");
    let known = known_registries(project.as_ref(), global.as_ref());
    for r in &known {
        let state = if creds.stored_token(&r.url).is_some() {
            "logged in"
        } else {
            "not logged in"
        };
        output.list_item(&r.name, &format!("{}  ({state})", r.url));
    }
    // Registries logged in to by URL only (not configured by name).
    for url in creds.urls().filter(|u| known.iter().all(|r| r.url != *u)) {
        output.list_item("(url)", &format!("{url}  (logged in)"));
    }
    output.list_item(
        "hackage",
        "https://hackage.haskell.org  (default for publish/search)",
    );
    if std::env::var(TOKEN_ENV).is_ok() {
        output.info(&format!("{TOKEN_ENV} is set and overrides stored tokens"));
    }
    Ok(())
}

async fn add(spec: &str, dir: &Path, output: &Output) -> Result<(), RegistryError> {
    let registry = resolve(spec)?;
    let cfg = RegistryClient::new(&registry.url, None)?.config().await?;
    let stanza = RepositoryStanza {
        name: registry.cabal_repo_name(),
        url: registry.url.clone(),
        tuf: Some(cfg.tuf.clone()),
    };
    if !cfg.tuf.enabled {
        output.warn(&format!(
            "{} does not serve signed (hackage-security) metadata; writing `secure: False`",
            registry.url
        ));
    }
    let path = dir.join("cabal.project");
    let existing = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(RegistryError::Io {
                action: "read",
                path,
                source,
            });
        }
    };
    let created = existing.is_none();
    let updated = upsert_repository_stanza(existing.as_deref(), &stanza);
    if existing.as_deref() == Some(updated.as_str()) {
        output.info(&format!("{} already lists {}", path.display(), stanza.name));
        return Ok(());
    }
    hx_core::atomic_write(&path, updated).map_err(|source| RegistryError::Io {
        action: "write",
        path: path.clone(),
        source,
    })?;
    output.status(
        if created { "Created" } else { "Updated" },
        &format!("{} with repository {}", path.display(), stanza.name),
    );
    output.info("Run `cabal update` to fetch the registry index.");
    Ok(())
}

async fn info(spec: &str, package: &str, output: &Output) -> Result<(), RegistryError> {
    let registry = resolve(spec)?;
    let p = client(&registry)?.package(package).await?;
    output.header(&format!(
        "{} {}",
        p.name,
        p.latest.as_deref().unwrap_or("(no versions)")
    ));
    if !p.synopsis.is_empty() {
        output.info(&p.synopsis);
    }
    output.list_item("registry", &registry.url);
    output.list_item("owners", &p.owners.join(", "));
    for v in &p.versions {
        let mut flags = Vec::new();
        if v.yanked {
            flags.push("yanked");
        }
        if v.bhc_verified {
            flags.push("BHC-verified");
        }
        output.list_item(&v.version, &flags.join(", "));
    }
    Ok(())
}

async fn yank(
    spec: &str,
    package: &str,
    version: &str,
    yanked: bool,
    output: &Output,
) -> Result<(), RegistryError> {
    let registry = resolve(spec)?;
    client(&registry)?
        .set_yanked(package, version, yanked)
        .await?;
    output.status(
        if yanked { "Yanked" } else { "Unyanked" },
        &format!("{package}-{version} on {}", registry.name),
    );
    Ok(())
}

async fn owner(command: RegistryOwnerCommands, output: &Output) -> Result<(), RegistryError> {
    let (spec, package, change) = match &command {
        RegistryOwnerCommands::List { package, registry } => (&registry.registry, package, None),
        RegistryOwnerCommands::Add {
            package,
            login,
            registry,
        } => (&registry.registry, package, Some((login, true))),
        RegistryOwnerCommands::Remove {
            package,
            login,
            registry,
        } => (&registry.registry, package, Some((login, false))),
    };
    let registry = resolve(spec)?;
    let c = client(&registry)?;
    let owners = match change {
        None => c.owners(package).await?,
        Some((login, add)) => c.change_owner(package, login, add).await?,
    };
    output.header(&format!("Owners of {package}"));
    for o in owners {
        output.list_item(&o, "");
    }
    Ok(())
}

async fn token(command: RegistryTokenCommands, output: &Output) -> Result<(), RegistryError> {
    match command {
        RegistryTokenCommands::Create {
            name,
            scopes,
            packages,
            expires_in_days,
            registry,
        } => {
            let registry = resolve(&registry.registry)?;
            let t = client(&registry)?
                .create_token(&name, &scopes, &packages, expires_in_days)
                .await?;
            output.status(
                "Created",
                &format!("token {} ({})", t.id, t.scopes.join(" ")),
            );
            output.info("This is the only time the token is shown:");
            println!("{}", t.token);
        }
        RegistryTokenCommands::List { registry } => {
            let registry = resolve(&registry.registry)?;
            let tokens = client(&registry)?.tokens().await?;
            output.header(&format!("Tokens on {}", registry.url));
            for t in tokens {
                let mut detail = t.scopes.join(" ");
                if let Some(p) = t.packages {
                    detail.push_str(&format!("  packages: {}", p.join(", ")));
                }
                output.list_item(&format!("{} {}", t.id, t.name), &detail);
            }
        }
        RegistryTokenCommands::Revoke { id, registry } => {
            let registry = resolve(&registry.registry)?;
            client(&registry)?.revoke_token(&id).await?;
            output.status("Revoked", &id);
        }
    }
    Ok(())
}

/// `hx search --registry`.
pub async fn search(
    spec: &str,
    query: &str,
    limit: usize,
    detailed: bool,
    output: &Output,
) -> Result<i32> {
    let r = async {
        let registry = resolve(spec)?;
        output.status("Searching", &format!("{} for '{query}'", registry.url));
        let hits = client(&registry)?.search(query, limit).await?;
        if hits.is_empty() {
            output.warn(&format!("No packages found matching '{query}'"));
            return Ok(());
        }
        output.header(&format!("Found {} package(s)", hits.len()));
        for h in &hits {
            let version = h.latest.as_deref().unwrap_or("-");
            if detailed {
                println!();
                output.header(&format!("{} {version}", h.name));
                if !h.synopsis.is_empty() {
                    output.info(&h.synopsis);
                }
                output.info(&format!(
                    "Registry: {}/api/v1/packages/{}",
                    registry.url, h.name
                ));
                output.info(&format!(
                    "Info: hx registry info {} --registry {spec}",
                    h.name
                ));
            } else {
                output.list_item(&h.name, &format!("{version}  {}", h.synopsis));
            }
        }
        Ok(())
    }
    .await;
    finish(r, output)
}

/// Upload an sdist tarball (used by `hx publish --registry`).
pub async fn upload(registry: &Registry, tarball: &Path, output: &Output) -> Result<i32> {
    let r = async {
        let client = client(registry)?;
        let bytes = std::fs::read(tarball).map_err(|source| RegistryError::Io {
            action: "read",
            path: tarball.to_path_buf(),
            source,
        })?;
        let spinner = Spinner::new(format!("Uploading to {}...", registry.url));
        match client.publish(bytes).await {
            Ok(p) => {
                spinner.finish_success(format!("Uploaded {}-{}", p.name, p.version));
                output.list_item("sha256", &p.sha256);
                output.status(
                    "Published",
                    &format!("{}-{} to {}", p.name, p.version, registry.name),
                );
                Ok(())
            }
            Err(e) => {
                spinner.finish_error("Upload failed");
                Err(e)
            }
        }
    }
    .await;
    finish(r, output)
}
