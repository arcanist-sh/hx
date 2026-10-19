//! Fetching and caching ghcup metadata.
//!
//! The metadata file is cached in hx's global cache directory and refreshed
//! once it is older than [`METADATA_TTL`]. A failed refresh falls back to the
//! stale cache, and with no cache at all [`load`] returns `None` so callers use
//! their built-in tables. Nothing here ever turns into an error for the user.

use super::GhcupMetadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::OnceCell;
use tracing::debug;

/// Where ghcup publishes the metadata hx reads.
/// ghcup's default channel, so hx's "recommended" matches what ghcup users get.
/// For upstream-only downloads, point `HX_GHCUP_METADATA_URL` at the vanilla
/// channel (`ghcup-vanilla-0.1.0.yaml`); its tags track the newest releases.
pub const DEFAULT_METADATA_URL: &str =
    "https://raw.githubusercontent.com/haskell/ghcup-metadata/master/ghcup-0.1.0.yaml";

/// Override the metadata URL (for mirrors and testing). Accepts `https://`,
/// `http://` and `file://` URLs.
pub const METADATA_URL_ENV: &str = "HX_GHCUP_METADATA_URL";

/// Never fetch metadata over the network; use the cache if there is one.
pub const OFFLINE_ENV: &str = "HX_OFFLINE";

/// How long a cached copy is used before it is refreshed.
pub const METADATA_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Network timeout for a refresh. Kept short: a slow refresh delays commands
/// like `hx doctor` and the stale cache or built-in tables are always usable.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// The metadata URL in effect (`HX_GHCUP_METADATA_URL` or the default).
pub fn metadata_url() -> String {
    std::env::var(METADATA_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_METADATA_URL.to_string())
}

/// Whether `HX_OFFLINE` is set to a truthy value.
pub fn is_offline() -> bool {
    std::env::var(OFFLINE_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Cache file for metadata fetched from `url`:
/// `<global cache>/ghcup/ghcup-metadata-<hash of url>.yaml`.
///
/// Keyed by URL so switching to a mirror never serves the other source's copy.
pub fn cache_file(url: &str) -> Option<PathBuf> {
    let digest = hx_core::hash::sha256_hex(url.as_bytes());
    let dir = hx_cache::global_cache_dir().ok()?.join("ghcup");
    Some(dir.join(format!("ghcup-metadata-{}.yaml", &digest[..16])))
}

/// How [`load_with`] finds the metadata.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Source URL.
    pub url: String,
    /// Cache file; `None` disables caching.
    pub cache_file: Option<PathBuf>,
    /// Use only the cache, never the network.
    pub offline: bool,
    /// Maximum age of a cache that is used without refreshing.
    pub ttl: Duration,
    /// Network timeout for a refresh.
    pub timeout: Duration,
}

impl LoadOptions {
    /// Options from the environment (`HX_GHCUP_METADATA_URL`, `HX_OFFLINE`).
    pub fn from_env() -> Self {
        let url = metadata_url();
        Self {
            cache_file: cache_file(&url),
            url,
            offline: is_offline(),
            ttl: METADATA_TTL,
            timeout: FETCH_TIMEOUT,
        }
    }
}

/// Load ghcup metadata as configured by the environment, at most once per
/// process. `None` means "use the built-in tables".
pub async fn load() -> Option<Arc<GhcupMetadata>> {
    static METADATA: OnceCell<Option<Arc<GhcupMetadata>>> = OnceCell::const_new();
    METADATA
        .get_or_init(|| async { load_with(&LoadOptions::from_env()).await.map(Arc::new) })
        .await
        .clone()
}

/// Load ghcup metadata: a fresh cache, else a refresh, else a stale cache.
pub async fn load_with(options: &LoadOptions) -> Option<GhcupMetadata> {
    let cached = options.cache_file.as_deref().and_then(read_cache);

    if let Some((contents, age)) = &cached
        && (options.offline || *age < options.ttl)
        && let Some(metadata) = parse_logged(contents, "cached")
    {
        return Some(metadata);
    }

    if options.offline {
        debug!("{} is set and no usable metadata cache exists", OFFLINE_ENV);
        return None;
    }

    match fetch(&options.url, options.timeout).await {
        Ok(contents) => {
            if let Some(metadata) = parse_logged(&contents, &options.url) {
                if let Some(path) = &options.cache_file {
                    write_cache(path, &contents);
                }
                return Some(metadata);
            }
        }
        Err(e) => debug!("Could not fetch ghcup metadata from {}: {}", options.url, e),
    }

    let (contents, _) = cached?;
    debug!("Using stale ghcup metadata cache");
    parse_logged(&contents, "stale cache")
}

fn parse_logged(contents: &str, source: &str) -> Option<GhcupMetadata> {
    match GhcupMetadata::parse(contents) {
        Ok(metadata) => Some(metadata),
        Err(e) => {
            debug!("Ignoring ghcup metadata from {}: {}", source, e);
            None
        }
    }
}

fn read_cache(path: &Path) -> Option<(String, Duration)> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    let contents = std::fs::read_to_string(path).ok()?;
    Some((contents, age))
}

/// Write the cache atomically (temp file + rename); failures only log.
fn write_cache(path: &Path, contents: &str) {
    let result = (|| -> std::io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        std::io::Write::write_all(&mut tmp, contents.as_bytes())?;
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    })();
    if let Err(e) = result {
        debug!(
            "Could not write ghcup metadata cache {}: {}",
            path.display(),
            e
        );
    }
}

async fn fetch(url: &str, timeout: Duration) -> Result<String, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return tokio::fs::read_to_string(path)
            .await
            .map_err(|e| e.to_string());
    }

    debug!("Fetching ghcup metadata from {}", url);
    let client = hx_core::http::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response.text().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Tool;

    const FIXTURE: &str = include_str!("../../fixtures/ghcup-0.1.0-excerpt.yaml");

    /// Nothing listens on port 1; the connection is refused immediately.
    const UNREACHABLE: &str = "http://127.0.0.1:1/ghcup-0.1.0.yaml";

    fn options(url: &str, cache: &Path) -> LoadOptions {
        LoadOptions {
            url: url.to_string(),
            cache_file: Some(cache.to_path_buf()),
            offline: false,
            ttl: METADATA_TTL,
            timeout: Duration::from_secs(5),
        }
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn make_stale(path: &Path) {
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(old).unwrap();
    }

    #[tokio::test]
    async fn test_fetch_populates_cache() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.yaml");
        std::fs::write(&source, FIXTURE).unwrap();
        let cache = dir.path().join("cache").join("meta.yaml");

        let m = load_with(&options(&file_url(&source), &cache))
            .await
            .unwrap();
        assert_eq!(m.recommended(Tool::Ghc), Some("9.10.3"));
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), FIXTURE);
    }

    #[tokio::test]
    async fn test_fresh_cache_skips_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("meta.yaml");
        std::fs::write(&cache, FIXTURE).unwrap();

        let m = load_with(&options(UNREACHABLE, &cache)).await;
        assert!(m.is_some());
    }

    #[tokio::test]
    async fn test_stale_cache_used_when_refresh_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("meta.yaml");
        std::fs::write(&cache, FIXTURE).unwrap();
        make_stale(&cache);

        let m = load_with(&options(UNREACHABLE, &cache)).await;
        assert_eq!(m.unwrap().latest(Tool::Ghc), Some("9.14.1"));
    }

    #[tokio::test]
    async fn test_stale_cache_refreshed_when_source_available() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("meta.yaml");
        std::fs::write(&cache, FIXTURE).unwrap();
        make_stale(&cache);
        let source = dir.path().join("source.yaml");
        let newer = FIXTURE.replacen("      9.14.1:", "      9.14.2:", 1);
        std::fs::write(&source, &newer).unwrap();

        let m = load_with(&options(&file_url(&source), &cache))
            .await
            .unwrap();
        assert!(m.release(Tool::Ghc, "9.14.2").is_some());
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), newer);
    }

    #[tokio::test]
    async fn test_no_cache_and_no_network() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("meta.yaml");
        assert!(load_with(&options(UNREACHABLE, &cache)).await.is_none());
        assert!(!cache.exists());
    }

    #[tokio::test]
    async fn test_offline_uses_stale_cache_and_never_fetches() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.yaml");
        std::fs::write(&source, FIXTURE).unwrap();
        let cache = dir.path().join("meta.yaml");

        let mut opts = options(&file_url(&source), &cache);
        opts.offline = true;
        assert!(load_with(&opts).await.is_none(), "offline must not fetch");

        std::fs::write(&cache, FIXTURE).unwrap();
        make_stale(&cache);
        assert!(load_with(&opts).await.is_some());
    }

    #[tokio::test]
    async fn test_invalid_source_is_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.yaml");
        std::fs::write(&source, "<html>rate limited</html>").unwrap();
        let cache = dir.path().join("meta.yaml");

        assert!(
            load_with(&options(&file_url(&source), &cache))
                .await
                .is_none()
        );
        assert!(!cache.exists());

        // A corrupt cache is ignored and replaced by a good refresh.
        std::fs::write(&cache, "not: [yaml").unwrap();
        std::fs::write(&source, FIXTURE).unwrap();
        assert!(
            load_with(&options(&file_url(&source), &cache))
                .await
                .is_some()
        );
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), FIXTURE);
    }

    #[test]
    fn test_cache_file_is_keyed_by_url() {
        let a = cache_file(DEFAULT_METADATA_URL).unwrap();
        let b = cache_file("https://mirror.example/ghcup-0.1.0.yaml").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.parent().unwrap().file_name().unwrap(), "ghcup");
    }

    #[tokio::test]
    #[ignore = "fetches the live ghcup metadata"]
    async fn test_live_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("meta.yaml");
        let m = load_with(&options(DEFAULT_METADATA_URL, &cache))
            .await
            .expect("live metadata loads");
        assert!(m.release(Tool::Ghc, "9.10.3").is_some());
        assert!(m.recommended(Tool::Ghc).is_some());
        assert!(m.recommended(Tool::Cabal).is_some());
        let host = super::super::Host::detect(crate::ghc::Platform::current().unwrap());
        assert!(m.download(Tool::Ghc, "9.10.3", &host).is_some());
    }
}
