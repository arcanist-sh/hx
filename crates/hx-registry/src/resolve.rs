//! Resolving `--registry <name|url>` to a concrete registry.

use crate::error::RegistryError;
use hx_config::RegistryConfig;
use std::collections::BTreeMap;

/// Name of the built-in arcanist.sh registry entry.
pub const ARCANIST: &str = "arcanist";

/// Default URL of the arcanist.sh registry (override with `[registries.arcanist]`).
pub const DEFAULT_ARCANIST_URL: &str = "https://registry.arcanist.sh";

/// A resolved registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    /// Configured name, or the URL's host for ad-hoc URLs.
    pub name: String,
    /// Normalized base URL (no trailing slash).
    pub url: String,
}

impl Registry {
    /// Name to use for the cabal `repository` stanza: the URL's host (and
    /// port), which keeps cabal's per-repository cache directories distinct
    /// for, say, production and a local dev server.
    ///
    /// cabal requires `[A-Za-z_.-][A-Za-z0-9_.-]*`, so other characters
    /// become `-` and a leading digit (an IP address) gets a `registry-` prefix.
    pub fn cabal_repo_name(&self) -> String {
        let rest = self.url.split_once("://").map_or(&*self.url, |(_, r)| r);
        let host = rest.split('/').next().unwrap_or(rest);
        let name: String = host
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        if name.starts_with(|c: char| c.is_ascii_digit()) || name.is_empty() {
            format!("registry-{name}")
        } else {
            name
        }
    }
}

/// Normalize and validate a registry base URL.
pub fn normalize_url(url: &str) -> Result<String, RegistryError> {
    let trimmed = url.trim().trim_end_matches('/');
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| RegistryError::InvalidUrl {
            url: url.to_string(),
            reason: "must start with https:// or http://".to_string(),
        })?;
    if rest.is_empty() || rest.starts_with('/') {
        return Err(RegistryError::InvalidUrl {
            url: url.to_string(),
            reason: "missing host".to_string(),
        });
    }
    if trimmed.contains(['?', '#', ' ']) {
        return Err(RegistryError::InvalidUrl {
            url: url.to_string(),
            reason: "must not contain a query, fragment or spaces".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

/// Resolve a registry name or URL.
///
/// Precedence for names: project `hx.toml` `[registries]`, then the global
/// config, then the built-in `arcanist` entry.
pub fn resolve_registry(
    spec: &str,
    project: Option<&BTreeMap<String, RegistryConfig>>,
    global: Option<&BTreeMap<String, RegistryConfig>>,
) -> Result<Registry, RegistryError> {
    if spec.starts_with("http://") || spec.starts_with("https://") {
        let url = normalize_url(spec)?;
        let name = [project, global]
            .into_iter()
            .flatten()
            .flat_map(|m| m.iter())
            .find(|(_, r)| normalize_url(&r.url).ok().as_deref() == Some(url.as_str()))
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| {
                Registry {
                    name: String::new(),
                    url: url.clone(),
                }
                .cabal_repo_name()
            });
        return Ok(Registry { name, url });
    }
    for map in [project, global].into_iter().flatten() {
        if let Some(r) = map.get(spec) {
            return Ok(Registry {
                name: spec.to_string(),
                url: normalize_url(&r.url)?,
            });
        }
    }
    if spec == ARCANIST {
        return Ok(Registry {
            name: ARCANIST.to_string(),
            url: DEFAULT_ARCANIST_URL.to_string(),
        });
    }
    let mut known: Vec<String> = [project, global]
        .into_iter()
        .flatten()
        .flat_map(|m| m.keys().cloned())
        .collect();
    known.push(ARCANIST.to_string());
    known.sort();
    known.dedup();
    Err(RegistryError::UnknownRegistry {
        name: spec.to_string(),
        known,
    })
}

/// All registries known by name (built-in, global, project; later wins).
pub fn known_registries(
    project: Option<&BTreeMap<String, RegistryConfig>>,
    global: Option<&BTreeMap<String, RegistryConfig>>,
) -> Vec<Registry> {
    let mut all: BTreeMap<String, String> = BTreeMap::new();
    all.insert(ARCANIST.to_string(), DEFAULT_ARCANIST_URL.to_string());
    for map in [global, project].into_iter().flatten() {
        for (name, r) in map {
            if let Ok(url) = normalize_url(&r.url) {
                all.insert(name.clone(), url);
            }
        }
    }
    all.into_iter()
        .map(|(name, url)| Registry { name, url })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs(entries: &[(&str, &str)]) -> BTreeMap<String, RegistryConfig> {
        entries
            .iter()
            .map(|(n, u)| (n.to_string(), RegistryConfig { url: u.to_string() }))
            .collect()
    }

    #[test]
    fn test_resolve_builtin_arcanist() {
        let r = resolve_registry("arcanist", None, None).unwrap();
        assert_eq!(r.url, DEFAULT_ARCANIST_URL);
        assert_eq!(r.cabal_repo_name(), "registry.arcanist.sh");
    }

    #[test]
    fn test_resolve_project_overrides_global_and_builtin() {
        let project = regs(&[("arcanist", "http://localhost:8787/")]);
        let global = regs(&[
            ("arcanist", "https://staging.example"),
            ("corp", "https://corp.example"),
        ]);
        let r = resolve_registry("arcanist", Some(&project), Some(&global)).unwrap();
        assert_eq!(r.url, "http://localhost:8787");
        assert_eq!(r.cabal_repo_name(), "localhost-8787");
        let c = resolve_registry("corp", Some(&project), Some(&global)).unwrap();
        assert_eq!(c.url, "https://corp.example");
    }

    #[test]
    fn test_cabal_repo_name_is_valid_for_cabal() {
        let r = resolve_registry("http://127.0.0.1:8787", None, None).unwrap();
        assert_eq!(r.cabal_repo_name(), "registry-127.0.0.1-8787");
    }

    #[test]
    fn test_resolve_url_uses_configured_name_when_known() {
        let global = regs(&[("corp", "https://corp.example/")]);
        let r = resolve_registry("https://corp.example", None, Some(&global)).unwrap();
        assert_eq!(r.name, "corp");
        let adhoc = resolve_registry("https://other.example/", None, Some(&global)).unwrap();
        assert_eq!(adhoc.name, "other.example");
        assert_eq!(
            resolve_registry("http://127.0.0.1:1", None, None)
                .unwrap()
                .name,
            "registry-127.0.0.1-1"
        );
        assert_eq!(adhoc.url, "https://other.example");
    }

    #[test]
    fn test_resolve_unknown_lists_known_names() {
        let global = regs(&[("corp", "https://corp.example")]);
        match resolve_registry("nope", None, Some(&global)) {
            Err(RegistryError::UnknownRegistry { known, .. }) => {
                assert_eq!(known, vec!["arcanist".to_string(), "corp".to_string()])
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn test_normalize_url_rejects_bad_urls() {
        assert!(normalize_url("ftp://x").is_err());
        assert!(normalize_url("https://").is_err());
        assert!(normalize_url("https://x/?a=1").is_err());
        assert_eq!(normalize_url(" https://x.y/ ").unwrap(), "https://x.y");
    }

    #[test]
    fn test_known_registries_includes_builtin() {
        let global = regs(&[("corp", "https://corp.example")]);
        let all = known_registries(None, Some(&global));
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "arcanist");
    }
}
