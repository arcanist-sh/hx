//! Toolchain detection and management for hx.
//!
//! This crate handles:
//! - Detecting GHC, Cabal, GHCup, HLS, and BHC
//! - Parsing tool versions
//! - Installing toolchains via ghcup or direct download
//! - Checking toolchain requirements and auto-installing
//! - Direct GHC and Cabal binary downloads (no ghcup dependency required)
//! - Per-project toolchain version management
//! - BHC (Basel Haskell Compiler) detection and management

pub mod bhc;
pub mod cabal;
pub mod check;
pub mod detect;
pub mod ghc;
pub mod install;
pub mod verify;

pub use bhc::{
    BhcError, BhcInstallOptions, BhcInstallResult, BhcVersion, InstalledBhc, KNOWN_BHC_VERSIONS,
    RECOMMENDED_BHC_VERSION, bhc_download_url, bhc_install_dir,
    current_platform as bhc_current_platform, detect as detect_bhc, install_bhc,
    is_known_version as is_known_bhc_version, is_valid_version as is_valid_bhc_version,
    known_versions as known_bhc_versions, list_installed as list_installed_bhc, remove_bhc,
    set_active as set_active_bhc, version_install_path as bhc_version_install_path,
};
pub use cabal::{
    CabalDownloadOptions, CabalInstallResult, CabalVersion, KNOWN_CABAL_VERSIONS,
    RECOMMENDED_CABAL_VERSION, cabal_download_url, download_and_install_cabal, get_active_cabal,
    is_known_version as is_known_cabal_version, is_valid_version as is_valid_cabal_version,
    known_versions as known_cabal_versions, list_installed as list_installed_cabal, remove_cabal,
    set_active_cabal,
};
pub use check::{AutoInstallPolicy, ToolchainCheck, check_requirements, ensure_toolchain};
pub use detect::{DetectedTool, ToolStatus, Toolchain};
pub use ghc::{
    DownloadOptions, GhcSource, GhcVersion, InstallResult, InstallSource, InstalledCabal,
    InstalledGhc, Platform, RECOMMENDED_GHC_VERSION, ResolutionConfig, ResolvedGhc,
    ToolchainManifest, create_symlinks, download_and_install_ghc, get_active, get_path_with_ghc,
    ghc_download_url, is_known_version, is_valid_version, known_versions, list_installed,
    remove_ghc, remove_symlinks, resolve_ghc, set_active,
};
pub use install::{
    InstallStrategy, SmartCabalInstallOptions, SmartInstallOptions, install_cabal_smart,
    install_ghc_smart,
};
