//! Environment variable constants for hx.
//!
//! This module defines all environment variables that hx recognizes,
//! providing a single source of truth for environment configuration.

/// Environment variable names used by hx.
pub struct EnvVars;

impl EnvVars {
    // ─── Global Settings ─────────────────────────────────────────────────────

    /// Path to the hx configuration file.
    pub const HX_CONFIG_FILE: &'static str = "HX_CONFIG_FILE";

    /// Directory for hx cache storage.
    pub const HX_CACHE_DIR: &'static str = "HX_CACHE_DIR";

    /// Enable verbose output.
    pub const HX_VERBOSE: &'static str = "HX_VERBOSE";

    /// Suppress output.
    pub const HX_QUIET: &'static str = "HX_QUIET";

    /// Disable colored output.
    pub const HX_NO_COLOR: &'static str = "HX_NO_COLOR";

    /// Force colored output.
    pub const HX_COLOR: &'static str = "HX_COLOR";

    /// Enable JSON log output.
    pub const HX_LOG_JSON: &'static str = "HX_LOG_JSON";

    // ─── Toolchain Settings ──────────────────────────────────────────────────

    /// Path to GHC executable.
    pub const HX_GHC: &'static str = "HX_GHC";

    /// Path to Cabal executable.
    pub const HX_CABAL: &'static str = "HX_CABAL";

    /// Path to GHCup executable.
    pub const HX_GHCUP: &'static str = "HX_GHCUP";

    /// Path to HLS executable.
    pub const HX_HLS: &'static str = "HX_HLS";

    /// Default GHC version to use.
    pub const HX_GHC_VERSION: &'static str = "HX_GHC_VERSION";

    /// Allow toolchain downloads to proceed without a published checksum.
    pub const HX_ALLOW_UNVERIFIED_DOWNLOADS: &'static str = "HX_ALLOW_UNVERIFIED_DOWNLOADS";

    /// Pinned Ed25519 public key (hex) for BHC Platform snapshot verification.
    pub const HX_BHC_PLATFORM_PUBKEY: &'static str = "HX_BHC_PLATFORM_PUBKEY";

    // ─── Build Settings ──────────────────────────────────────────────────────

    /// Number of parallel jobs for builds.
    pub const HX_JOBS: &'static str = "HX_JOBS";

    /// Enable release mode builds.
    pub const HX_RELEASE: &'static str = "HX_RELEASE";

    // ─── Format/Lint Settings ────────────────────────────────────────────────

    /// Formatter to use (fourmolu or ormolu).
    pub const HX_FORMATTER: &'static str = "HX_FORMATTER";

    // ─── Standard Environment Variables ──────────────────────────────────────

    /// Standard NO_COLOR environment variable.
    pub const NO_COLOR: &'static str = "NO_COLOR";

    /// Standard CLICOLOR environment variable.
    pub const CLICOLOR: &'static str = "CLICOLOR";

    /// CI environment indicator.
    pub const CI: &'static str = "CI";

    /// Jupyter session name (for detecting notebook environments).
    pub const JPY_SESSION_NAME: &'static str = "JPY_SESSION_NAME";

    /// Standard HOME environment variable.
    pub const HOME: &'static str = "HOME";
}

/// Check if running in a CI environment.
pub fn is_ci() -> bool {
    std::env::var(EnvVars::CI).is_ok()
}

/// Check if running in a Jupyter notebook environment.
pub fn is_jupyter() -> bool {
    std::env::var(EnvVars::JPY_SESSION_NAME).is_ok()
}

/// Check if colors should be disabled based on environment.
pub fn no_color() -> bool {
    std::env::var(EnvVars::NO_COLOR).is_ok()
        || std::env::var(EnvVars::HX_NO_COLOR).is_ok()
        || std::env::var(EnvVars::CLICOLOR)
            .map(|v| v == "0")
            .unwrap_or(false)
}
