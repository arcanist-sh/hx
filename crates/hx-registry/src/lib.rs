//! Client for hx package registries.
//!
//! The arcanist.sh registry (`https://registry.arcanist.sh` by default) is a
//! Haskell package repository that plain cabal consumes through a
//! `repository` stanza (hackage-security / TUF), plus a JSON API that hx uses
//! for search, publishing, yanking, ownership and API tokens.
//!
//! This crate handles:
//! - Resolving `--registry <name|url>` against `[registries]` in `hx.toml`
//!   and the global config, with a built-in `arcanist` entry
//! - Storing API tokens in `<hx config dir>/credentials.toml` (mode 0600)
//! - The JSON API client
//! - Writing the cabal `repository` stanza into `cabal.project`
//!
//! Hackage stays the default target for `hx publish` / `hx search`; a
//! registry is only used when asked for explicitly.

pub mod cabal_project;
pub mod client;
pub mod credentials;
pub mod error;
#[cfg(any(test, feature = "test-util"))]
pub mod mock;
pub mod resolve;

pub use cabal_project::{RepositoryStanza, upsert_repository_stanza};
pub use client::{
    ApiErrorBody, CreatedToken, Me, PackageInfo, PublishResult, RegistryClient, RegistryConfigInfo,
    SearchHit, TokenInfo, TufInfo, VersionDetail, VersionSummary,
};
pub use credentials::{Credentials, TOKEN_ENV};
pub use error::RegistryError;
pub use resolve::{ARCANIST, DEFAULT_ARCANIST_URL, Registry, known_registries, resolve_registry};
