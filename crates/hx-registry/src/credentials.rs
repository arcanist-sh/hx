//! API token storage.
//!
//! Tokens live in `<hx config dir>/credentials.toml`, keyed by registry URL,
//! written atomically with mode 0600 on Unix:
//!
//! ```toml
//! [registries."https://registry.arcanist.sh"]
//! token = "hxr_..."
//! ```
//!
//! `HX_REGISTRY_TOKEN` overrides the file (handy in CI).

use crate::error::RegistryError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Environment variable that overrides the stored token.
pub const TOKEN_ENV: &str = "HX_REGISTRY_TOKEN";

/// File name inside the hx config directory.
pub const CREDENTIALS_FILE: &str = "credentials.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Entry {
    token: String,
}

/// Stored registry credentials.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default)]
    registries: BTreeMap<String, Entry>,
    #[serde(skip)]
    path: PathBuf,
}

impl Credentials {
    /// Default location: `<hx config dir>/credentials.toml`.
    pub fn default_path() -> Result<PathBuf, RegistryError> {
        hx_cache::global_config_dir()
            .map(|d| d.join(CREDENTIALS_FILE))
            .map_err(|e| RegistryError::Io {
                action: "locate",
                path: PathBuf::from(CREDENTIALS_FILE),
                source: std::io::Error::other(e.to_string()),
            })
    }

    /// Load from the default location (empty if the file does not exist).
    pub fn load() -> Result<Self, RegistryError> {
        Self::load_from(&Self::default_path()?)
    }

    /// Load from an explicit path (empty if the file does not exist).
    pub fn load_from(path: &Path) -> Result<Self, RegistryError> {
        let mut creds = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Credentials>(&text).map_err(|e| RegistryError::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Credentials::default(),
            Err(source) => {
                return Err(RegistryError::Io {
                    action: "read",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        creds.path = path.to_path_buf();
        Ok(creds)
    }

    /// Where this file is (or will be) stored.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Token for a registry URL: `HX_REGISTRY_TOKEN` first, then the file.
    pub fn token_for(&self, url: &str) -> Option<String> {
        if let Ok(t) = std::env::var(TOKEN_ENV)
            && !t.trim().is_empty()
        {
            return Some(t.trim().to_string());
        }
        self.stored_token(url)
    }

    /// Token stored in the file (ignores the environment).
    pub fn stored_token(&self, url: &str) -> Option<String> {
        self.registries.get(url).map(|e| e.token.clone())
    }

    /// Store a token for a registry URL (call [`save`](Self::save) afterwards).
    pub fn set_token(&mut self, url: &str, token: &str) {
        self.registries.insert(
            url.to_string(),
            Entry {
                token: token.to_string(),
            },
        );
    }

    /// Remove a registry's token. Returns whether one was stored.
    pub fn remove(&mut self, url: &str) -> bool {
        self.registries.remove(url).is_some()
    }

    /// URLs with a stored token.
    pub fn urls(&self) -> impl Iterator<Item = &str> {
        self.registries.keys().map(String::as_str)
    }

    /// Write the file atomically with owner-only permissions.
    pub fn save(&self) -> Result<(), RegistryError> {
        let io = |action, source| RegistryError::Io {
            action,
            path: self.path.clone(),
            source,
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io("create the directory for", e))?;
        }
        let text = format!(
            "# hx registry API tokens. Keep this file private.\n{}",
            toml::to_string_pretty(self).map_err(|e| io("serialize", std::io::Error::other(e)))?
        );
        // atomic_write uses a NamedTempFile, which is created 0600 on Unix;
        // set it explicitly anyway in case the file pre-existed with wider bits.
        hx_core::atomic_write(&self.path, text).map_err(|e| io("write", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io("set permissions on", e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_roundtrip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(CREDENTIALS_FILE);
        let mut c = Credentials::load_from(&path).unwrap();
        assert!(c.stored_token("https://r.example").is_none());
        c.set_token("https://r.example", "hxr_secret");
        c.save().unwrap();

        let loaded = Credentials::load_from(&path).unwrap();
        assert_eq!(
            loaded.stored_token("https://r.example").as_deref(),
            Some("hxr_secret")
        );
        assert_eq!(loaded.urls().collect::<Vec<_>>(), vec!["https://r.example"]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let mut c = loaded;
        assert!(c.remove("https://r.example"));
        assert!(!c.remove("https://r.example"));
        c.save().unwrap();
        assert!(
            Credentials::load_from(&path)
                .unwrap()
                .stored_token("https://r.example")
                .is_none()
        );
    }

    #[test]
    fn test_credentials_parse_error_names_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CREDENTIALS_FILE);
        std::fs::write(&path, "not = [valid").unwrap();
        let err = Credentials::load_from(&path).unwrap_err();
        assert!(err.to_string().contains("credentials.toml"));
    }
}
