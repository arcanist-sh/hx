//! Dependency solver for Haskell packages.
//!
//! This crate provides:
//! - Version parsing and constraints (PVP-compliant)
//! - Package metadata types
//! - Backtracking dependency resolver
//! - Hackage index parsing
//! - .cabal file parsing
//! - Native package fetching
//! - Build plan generation
//!
//! # Example
//!
//! ```ignore
//! use hx_solver::{load_index, IndexOptions, Resolver, VersionConstraint, default_index_path};
//!
//! // Load the Hackage index
//! let path = default_index_path().expect("Hackage index not found");
//! let index = load_index(&path, &IndexOptions::default())?;
//!
//! // Resolve dependencies
//! let resolver = Resolver::new(&index);
//! let plan = resolver.resolve("aeson", &VersionConstraint::Any)?;
//!
//! for pkg in &plan.packages {
//!     println!("{} {}", pkg.name, pkg.version);
//! }
//! ```

pub mod bhc_platform;
pub mod cabal;
pub mod cache;
pub mod condition;
pub mod extract;
pub mod fetch;
pub mod index;
pub mod mirror;
pub mod modules;
pub mod package;
pub mod plan;
pub mod resolver;
pub mod snapshot;
pub mod version;

pub use cabal::{
    BuildType, CabalFile, CustomSetupConfig, ExecutableConfig, LibraryConfig, PackageBuildInfo,
    parse_cabal, parse_cabal_ctx, parse_cabal_full,
};
pub use cache::{
    CacheError, clear_index_cache, clear_resolution_cache, compute_deps_fingerprint,
    load_cached_index, load_cached_resolution, save_index_cache, save_resolution_cache,
};
pub use condition::{CabalContext, Condition, parse_condition};
pub use extract::{
    ExtractError, ExtractOptions, ExtractedPackage, clear_sources, extract_all_packages,
    extract_package, is_extracted, sources_dir,
};
pub use fetch::{
    FetchError, FetchOptions, FetchResult, FetchSummary, default_package_cache_dir, fetch_packages,
};
pub use index::{IndexError, IndexOptions, default_index_path, load_index};
pub use mirror::{
    IndexState, IndexStatus, MirrorError, MirrorOptions, UpdateResult, best_index_path,
    clear_index, index_dir, index_is_current, index_path, index_state_path, index_status,
    load_index_state, update_index,
};
pub use modules::{
    ModuleError, ModuleGraph, ModuleInfo, build_module_graph, find_haskell_files, parse_imports,
    path_to_module_name,
};
pub use package::{
    Dependency, InstallPlan, Package, PackageIndex, PackageVersion, ResolvedPackage,
};
pub use plan::{
    BuildPlan, BuildStyle, BuildUnit, PlanError, PlanOptions, PlanSummary, generate_build_plan,
};
pub use resolver::{ResolveError, Resolver, ResolverConfig};
pub use snapshot::{
    Snapshot, SnapshotError, SnapshotId, SnapshotMetadata, SnapshotPackage, SnapshotType,
    fetch_snapshot, get_latest_lts, get_latest_nightly, load_snapshot, snapshot_cache_dir,
};
pub use version::{Version, VersionConstraint, VersionParseError, parse_constraint};
