//! JSON API client (`/api/v1`).

use crate::error::RegistryError;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

/// Error envelope returned by the registry: `{"error": {...}}`.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorBody {
    /// The error details.
    pub error: ApiErrorDetail,
}

/// Error details.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorDetail {
    /// Machine-readable code.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Optional hint.
    #[serde(default)]
    pub hint: Option<String>,
}

/// hackage-security settings advertised by the registry.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TufInfo {
    /// Whether signed TUF metadata is served.
    pub enabled: bool,
    /// Root key ids for the cabal `root-keys:` field.
    #[serde(default)]
    pub root_keys: Vec<String>,
    /// Signature threshold for the cabal `key-threshold:` field.
    #[serde(default)]
    pub key_threshold: Option<u32>,
}

/// `GET /api/v1/config`.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryConfigInfo {
    /// Registry display name.
    pub name: String,
    /// Public URL.
    pub url: String,
    /// TUF settings.
    #[serde(default)]
    pub tuf: TufInfo,
}

/// One search result.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchHit {
    /// Package name.
    pub name: String,
    /// Synopsis from the latest `.cabal`.
    #[serde(default)]
    pub synopsis: String,
    /// Latest non-yanked version.
    pub latest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchHit>,
}

/// A version in `GET /api/v1/packages/:name`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionSummary {
    /// Version number.
    pub version: String,
    /// Whether it is yanked.
    pub yanked: bool,
    /// Publish time (unix seconds).
    pub published_at: i64,
    /// sha256 of the tarball.
    pub sha256: String,
    /// Whether a BHC verifier attested a passing build.
    #[serde(default)]
    pub bhc_verified: bool,
}

/// `GET /api/v1/packages/:name`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackageInfo {
    /// Package name.
    pub name: String,
    /// Synopsis.
    #[serde(default)]
    pub synopsis: String,
    /// Latest non-yanked version.
    pub latest: Option<String>,
    /// Owner logins.
    #[serde(default)]
    pub owners: Vec<String>,
    /// Versions, newest first.
    pub versions: Vec<VersionSummary>,
}

/// A dependency in a version detail.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dependency {
    /// Package name.
    pub name: String,
    /// Version range text.
    pub range: String,
}

/// `GET /api/v1/packages/:name/:version`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionDetail {
    /// Package name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Whether it is yanked.
    pub yanked: bool,
    /// Build dependencies (union over components).
    #[serde(default)]
    pub deps: Vec<Dependency>,
    /// Whether a BHC verifier attested a passing build.
    #[serde(default)]
    pub bhc_verified: bool,
    /// Tarball download URL.
    pub tarball_url: String,
}

/// `PUT /api/v1/packages/upload` response.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishResult {
    /// Package name.
    pub name: String,
    /// Version.
    pub version: String,
    /// sha256 the registry computed.
    pub sha256: String,
}

/// A token as listed by `GET /api/v1/tokens` (no secret).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenInfo {
    /// Public id (`tok_...`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Scopes.
    pub scopes: Vec<String>,
    /// Package allow-list, if restricted.
    pub packages: Option<Vec<String>>,
    /// Expiry (unix seconds).
    pub expires_at: Option<i64>,
    /// Last use (unix seconds).
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TokenList {
    tokens: Vec<TokenInfo>,
}

/// `POST /api/v1/tokens` response: contains the secret, shown once.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatedToken {
    /// Public id.
    pub id: String,
    /// The secret token.
    pub token: String,
    /// Scopes.
    pub scopes: Vec<String>,
}

/// `GET /api/v1/me`.
#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    /// The user's login.
    pub login: String,
    /// The token used for the request.
    pub token: MeToken,
}

/// The calling token, as reported by `GET /api/v1/me`.
#[derive(Debug, Clone, Deserialize)]
pub struct MeToken {
    /// Public token id (`tok_...`).
    pub id: String,
    /// Scopes.
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OwnersResponse {
    owners: Vec<Owner>,
}

#[derive(Debug, Deserialize)]
struct Owner {
    login: String,
}

/// Client for one registry.
#[derive(Debug, Clone)]
pub struct RegistryClient {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl RegistryClient {
    /// Create a client for `base_url` (normalized, without trailing slash).
    pub fn new(base_url: &str, token: Option<String>) -> Result<Self, RegistryError> {
        let http = reqwest::Client::builder()
            .user_agent(format!("hx/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| RegistryError::Http {
                url: base_url.to_string(),
                message: e.to_string(),
            })?;
        Ok(Self {
            base: base_url.trim_end_matches('/').to_string(),
            token,
            http,
        })
    }

    /// Base URL.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Body>,
        auth: bool,
    ) -> Result<T, RegistryError> {
        let url = format!("{}{}", self.base, path);
        debug!(%method, %url, "registry request");
        let mut req = self.http.request(method, &url);
        if auth {
            let token = self
                .token
                .as_ref()
                .ok_or_else(|| RegistryError::NotLoggedIn {
                    url: self.base.clone(),
                })?;
            req = req.bearer_auth(token);
        }
        req = match body {
            Some(Body::Json(v)) => req.json(&v),
            Some(Body::Gzip(bytes)) => req.header("Content-Type", "application/gzip").body(bytes),
            None => req,
        };
        let resp = req.send().await.map_err(|e| RegistryError::Http {
            url: url.clone(),
            message: e.to_string(),
        })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| RegistryError::Http {
            url: url.clone(),
            message: e.to_string(),
        })?;
        if !status.is_success() {
            return Err(api_error(&url, status, &text));
        }
        serde_json::from_str(&text).map_err(|e| RegistryError::Http {
            url,
            message: format!("unexpected response ({e})"),
        })
    }

    /// `GET /api/v1/config`.
    pub async fn config(&self) -> Result<RegistryConfigInfo, RegistryError> {
        self.request(Method::GET, "/api/v1/config", None, false)
            .await
    }

    /// `GET /api/v1/search`.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, RegistryError> {
        let path = format!("/api/v1/search?q={}&limit={limit}", encode(query));
        let r: SearchResponse = self.request(Method::GET, &path, None, false).await?;
        Ok(r.results)
    }

    /// `GET /api/v1/packages/:name`.
    pub async fn package(&self, name: &str) -> Result<PackageInfo, RegistryError> {
        self.request(
            Method::GET,
            &format!("/api/v1/packages/{}", encode(name)),
            None,
            false,
        )
        .await
    }

    /// `GET /api/v1/packages/:name/:version`.
    pub async fn version(&self, name: &str, version: &str) -> Result<VersionDetail, RegistryError> {
        let path = format!("/api/v1/packages/{}/{}", encode(name), encode(version));
        self.request(Method::GET, &path, None, false).await
    }

    /// `GET /api/v1/me` (validates the token).
    pub async fn me(&self) -> Result<Me, RegistryError> {
        self.request(Method::GET, "/api/v1/me", None, true).await
    }

    /// Upload an sdist tarball.
    pub async fn publish(&self, tarball: Vec<u8>) -> Result<PublishResult, RegistryError> {
        self.request(
            Method::PUT,
            "/api/v1/packages/upload",
            Some(Body::Gzip(tarball)),
            true,
        )
        .await
    }

    /// Yank (`true`) or unyank (`false`) a version.
    pub async fn set_yanked(
        &self,
        name: &str,
        version: &str,
        yanked: bool,
    ) -> Result<(), RegistryError> {
        let path = format!("/api/v1/packages/{}/{}/yank", encode(name), encode(version));
        let method = if yanked { Method::PUT } else { Method::DELETE };
        let _: serde_json::Value = self.request(method, &path, None, true).await?;
        Ok(())
    }

    /// List owners of a package.
    pub async fn owners(&self, name: &str) -> Result<Vec<String>, RegistryError> {
        let path = format!("/api/v1/packages/{}/owners", encode(name));
        let r: OwnersResponse = self.request(Method::GET, &path, None, false).await?;
        Ok(r.owners.into_iter().map(|o| o.login).collect())
    }

    /// Add (`true`) or remove (`false`) an owner; returns the new owner list.
    pub async fn change_owner(
        &self,
        name: &str,
        login: &str,
        add: bool,
    ) -> Result<Vec<String>, RegistryError> {
        let path = format!("/api/v1/packages/{}/owners/{}", encode(name), encode(login));
        let method = if add { Method::PUT } else { Method::DELETE };
        let r: OwnersResponse = self.request(method, &path, None, true).await?;
        Ok(r.owners.into_iter().map(|o| o.login).collect())
    }

    /// List the caller's active tokens.
    pub async fn tokens(&self) -> Result<Vec<TokenInfo>, RegistryError> {
        let r: TokenList = self
            .request(Method::GET, "/api/v1/tokens", None, true)
            .await?;
        Ok(r.tokens)
    }

    /// Create a token.
    pub async fn create_token(
        &self,
        name: &str,
        scopes: &[String],
        packages: &[String],
        expires_in_days: Option<u32>,
    ) -> Result<CreatedToken, RegistryError> {
        let mut body = serde_json::json!({ "name": name });
        if !scopes.is_empty() {
            body["scopes"] = serde_json::json!(scopes);
        }
        if !packages.is_empty() {
            body["packages"] = serde_json::json!(packages);
        }
        if let Some(d) = expires_in_days {
            body["expires_in_days"] = serde_json::json!(d);
        }
        self.request(Method::POST, "/api/v1/tokens", Some(Body::Json(body)), true)
            .await
    }

    /// Revoke a token by id.
    pub async fn revoke_token(&self, id: &str) -> Result<(), RegistryError> {
        let path = format!("/api/v1/tokens/{}", encode(id));
        let _: serde_json::Value = self.request(Method::DELETE, &path, None, true).await?;
        Ok(())
    }
}

enum Body {
    Json(serde_json::Value),
    Gzip(Vec<u8>),
}

fn api_error(url: &str, status: StatusCode, text: &str) -> RegistryError {
    match serde_json::from_str::<ApiErrorBody>(text) {
        Ok(b) => RegistryError::Api {
            status: status.as_u16(),
            code: b.error.code,
            message: b.error.message,
            hint: b.error.hint,
        },
        Err(_) => RegistryError::Http {
            url: url.to_string(),
            message: format!("HTTP {status}"),
        },
    }
}

/// Percent-encode a path or query component.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockResponse, MockServer};

    #[tokio::test]
    async fn test_search_and_package_info() {
        let server = MockServer::start(vec![
            MockResponse::json(
                "GET",
                "/api/v1/search?q=hello%20world&limit=5",
                200,
                r#"{"query":"hello world","results":[{"name":"hello","synopsis":"Hi","latest":"1.0"}]}"#,
            ),
            MockResponse::json(
                "GET",
                "/api/v1/packages/hello",
                200,
                r#"{"name":"hello","synopsis":"Hi","latest":"1.0","owners":["zara"],"versions":[{"version":"1.0","yanked":false,"published_at":1,"sha256":"ab","size":3,"bhc_verified":true}]}"#,
            ),
        ]);
        let c = RegistryClient::new(&server.url(), None).unwrap();
        let hits = c.search("hello world", 5).await.unwrap();
        assert_eq!(hits[0].name, "hello");
        let info = c.package("hello").await.unwrap();
        assert!(info.versions[0].bhc_verified);
        assert_eq!(info.owners, vec!["zara"]);
    }

    #[tokio::test]
    async fn test_publish_sends_token_and_gzip_body() {
        let server = MockServer::start(vec![MockResponse::json(
            "PUT",
            "/api/v1/packages/upload",
            201,
            r#"{"name":"hello","version":"1.0","sha256":"ab","size":3}"#,
        )]);
        let c = RegistryClient::new(&server.url(), Some("hxr_t".into())).unwrap();
        let r = c.publish(vec![1, 2, 3]).await.unwrap();
        assert_eq!(r.version, "1.0");
        let req = &server.requests()[0];
        assert_eq!(req.header("authorization").as_deref(), Some("Bearer hxr_t"));
        assert_eq!(
            req.header("content-type").as_deref(),
            Some("application/gzip")
        );
        assert_eq!(req.body, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_api_errors_are_structured() {
        let server = MockServer::start(vec![MockResponse::json(
            "PUT",
            "/api/v1/packages/upload",
            409,
            r#"{"error":{"code":"version_exists","message":"hello-1.0 already exists","hint":"bump the version"}}"#,
        )]);
        let c = RegistryClient::new(&server.url(), Some("t".into())).unwrap();
        let err = c.publish(vec![]).await.unwrap_err();
        assert_eq!(err.api_code(), Some("version_exists"));
        assert_eq!(err.to_string(), "hello-1.0 already exists");
        assert_eq!(err.fix().as_deref(), Some("bump the version"));
    }

    #[tokio::test]
    async fn test_authenticated_call_without_token_fails_locally() {
        let c = RegistryClient::new("http://127.0.0.1:9", None).unwrap();
        assert!(matches!(
            c.me().await,
            Err(RegistryError::NotLoggedIn { .. })
        ));
    }

    #[tokio::test]
    async fn test_non_json_error_is_http_error() {
        let server = MockServer::start(vec![MockResponse::text(
            "GET",
            "/api/v1/config",
            502,
            "bad gateway",
        )]);
        let c = RegistryClient::new(&server.url(), None).unwrap();
        assert!(matches!(c.config().await, Err(RegistryError::Http { .. })));
    }

    #[test]
    fn test_encode() {
        assert_eq!(encode("a b/c"), "a%20b%2Fc");
        assert_eq!(encode("foo-1.0"), "foo-1.0");
    }
}
