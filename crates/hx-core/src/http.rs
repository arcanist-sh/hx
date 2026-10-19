//! The one place hx builds HTTP clients.
//!
//! Every network request hx makes (toolchain downloads, the Hackage index,
//! Stackage and BHC Platform snapshots, self-upgrade) goes through a client
//! from [`builder`] or [`client`], so they all share:
//!
//! - **Pure-Rust TLS.** reqwest is built with `rustls-no-provider`, and the
//!   `ring` crypto provider is installed here before any client exists. The
//!   default `rustls` feature would pull in aws-lc-rs, a C library the
//!   cross-compiled release targets cannot easily build. A client built
//!   without a provider installed panics, which is why nothing outside this
//!   module may call `reqwest::Client::new`/`builder` (enforced by
//!   `clippy.toml`).
//! - **Trust roots.** Certificates are verified against the operating
//!   system's trust store (Keychain on macOS, the system store on Windows,
//!   `/etc/ssl`, or `SSL_CERT_FILE`/`SSL_CERT_DIR` when set, on Linux), so corporate TLS-inspecting
//!   proxies whose CA is installed system-wide work. The Mozilla root bundle
//!   is merged in as well, so a minimal container with no CA store still
//!   reaches Hackage and GitHub, as it did before.
//! - **Identification and limits.** A `hx/<version>` User-Agent and a connect
//!   timeout. `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` and the system proxy
//!   settings are honoured.

use std::sync::Once;
use std::time::Duration;

/// The User-Agent sent with every request: `hx/<version>`.
pub const USER_AGENT: &str = concat!("hx/", env!("CARGO_PKG_VERSION"));

/// How long to wait for a TCP + TLS connection before giving up.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Install rustls's `ring` provider as the process default, once.
///
/// Called by [`builder`]; exposed for code that builds a rustls config
/// directly. If another provider was installed first, that one is kept.
pub fn install_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        // Err means a provider is already installed, which is all we need.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A client builder with hx's TLS, User-Agent and timeout defaults applied.
///
/// Callers add request-specific settings (an overall `timeout`, a redirect
/// policy) and call `build()`.
#[allow(clippy::disallowed_methods)] // the one sanctioned call site
pub fn builder() -> reqwest::ClientBuilder {
    install_crypto_provider();
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .tls_certs_merge(bundled_roots())
}

/// A client with hx's defaults and no overall request timeout.
pub fn client() -> reqwest::Result<reqwest::Client> {
    builder().build()
}

/// The Mozilla root bundle, as reqwest certificates.
fn bundled_roots() -> Vec<reqwest::Certificate> {
    webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|der| reqwest::Certificate::from_der(der).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_hx_and_its_version() {
        assert_eq!(USER_AGENT, format!("hx/{}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn bundled_roots_are_loaded() {
        assert!(bundled_roots().len() > 100);
    }

    #[test]
    fn builds_a_client_without_a_preinstalled_provider() {
        // Panics inside reqwest if no rustls crypto provider is installed.
        client().expect("client builds");
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    /// Real TLS handshake against Hackage and GitHub.
    /// Run with `cargo test -p hx-core -- --ignored`.
    #[tokio::test]
    #[ignore = "network"]
    async fn https_requests_succeed() {
        let client = client().unwrap();
        for url in [
            "https://hackage.haskell.org/package/aeson/preferred",
            "https://api.github.com/repos/arcanist-sh/hx",
        ] {
            let response = client.get(url).send().await.unwrap();
            assert!(
                response.status().is_success(),
                "{url}: {}",
                response.status()
            );
        }
    }
}
