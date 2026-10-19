//! Registry client errors.

use std::path::PathBuf;
use thiserror::Error;

/// Errors from resolving, authenticating against, or talking to a registry.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// `--registry <name>` did not match any configured registry.
    #[error("unknown registry `{name}`")]
    UnknownRegistry {
        /// The name that was asked for.
        name: String,
        /// Names that are configured.
        known: Vec<String>,
    },

    /// A registry URL is not http(s).
    #[error("invalid registry URL `{url}`: {reason}")]
    InvalidUrl {
        /// The offending URL.
        url: String,
        /// Why it was rejected.
        reason: String,
    },

    /// No token stored for a registry that needs one.
    #[error("not logged in to {url}")]
    NotLoggedIn {
        /// Registry URL.
        url: String,
    },

    /// The registry answered with a structured error.
    #[error("{message}")]
    Api {
        /// HTTP status.
        status: u16,
        /// Machine-readable error code (e.g. `version_exists`).
        code: String,
        /// Human-readable message from the registry.
        message: String,
        /// Optional hint from the registry.
        hint: Option<String>,
    },

    /// Network failure or an unexpected (non-JSON) response.
    #[error("request to {url} failed: {message}")]
    Http {
        /// URL that was requested.
        url: String,
        /// Description of the failure.
        message: String,
    },

    /// Reading or writing a local file failed.
    #[error("failed to {action} {}: {source}", path.display())]
    Io {
        /// What we were doing ("read", "write", ...).
        action: &'static str,
        /// The file involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A local file could not be parsed.
    #[error("failed to parse {}: {message}", path.display())]
    Parse {
        /// The file involved.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
}

impl RegistryError {
    /// A suggested next step for the user, if there is an obvious one.
    pub fn fix(&self) -> Option<String> {
        match self {
            RegistryError::UnknownRegistry { name, .. } => Some(format!(
                "add it to hx.toml or the global config:\n\n  [registries.{name}]\n  url = \"https://...\"\n\n     or pass a URL: --registry https://..."
            )),
            RegistryError::InvalidUrl { .. } => {
                Some("use an http(s) URL such as https://registry.arcanist.sh".to_string())
            }
            RegistryError::NotLoggedIn { url } => Some(format!(
                "run `hx registry login --registry {url}` (or set HX_REGISTRY_TOKEN)"
            )),
            RegistryError::Api { status: 401, .. } => {
                Some("run `hx registry login` with a valid token".to_string())
            }
            RegistryError::Api { hint: Some(h), .. } => Some(h.clone()),
            RegistryError::Http { .. } => {
                Some("check the registry URL and your network connection".to_string())
            }
            _ => None,
        }
    }

    /// The registry's machine-readable error code, if this is an API error.
    pub fn api_code(&self) -> Option<&str> {
        match self {
            RegistryError::Api { code, .. } => Some(code),
            _ => None,
        }
    }
}
