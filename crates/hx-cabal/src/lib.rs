//! Cabal wrapper and output parsing for hx.
//!
//! This crate handles:
//! - Invoking Cabal commands
//! - Parsing build output for errors
//! - Managing build directories
//! - Editing .cabal files
//! - Parallel package fetching
//! - Native GHC build orchestration
//! - GHC diagnostic parsing for LSP
//! - GHC compiler backend abstraction

pub mod backend;
pub mod build;
pub mod edit;
pub mod fetch;
pub mod freeze;
pub mod full_native;
pub mod ghc_diagnostics;
pub mod install;
pub mod native;
pub mod package_build;
pub mod package_db;
pub mod preprocessor;
pub mod server;
pub mod setup;

pub use backend::GhcBackend;
pub use build::{BuildOptions, BuildResult};
pub use edit::{CabalEditError, add_dependency, remove_dependency};
pub use fetch::{FetchOptions, FetchResult};
pub use full_native::{
    FullNativeBuildOptions, FullNativeBuildResult, FullNativeBuilder, is_pre_installed,
    pre_installed_packages,
};
pub use ghc_diagnostics::{
    diagnostic_flags, parse_ghc_json, parse_ghc_text, supports_json_diagnostics,
};
pub use install::InstallOptions;
pub use native::{
    BuildState, CCompileOptions, CCompileResult, CCompilerConfig, CCompilerType, GhcConfig,
    ModuleCompileResult, ModuleState, NativeBuildOptions, NativeBuildResult, NativeBuilder,
    PackageInfo, collect_extra_libraries, collect_hs_libraries, collect_library_paths,
    compile_c_sources, compute_flags_hash, detect_main_module, find_all_main_modules,
    packages_from_lockfile, query_all_packages, query_package_info, resolve_transitive_deps,
};
pub use package_build::{PackageBuildConfig, PackageBuildResult, build_package};
pub use package_db::{PackageDb, clear_package_db, default_cache_dir};
pub use preprocessor::{
    PreprocessResult, Preprocessor, PreprocessorConfig, PreprocessorSources, check_availability,
    detect_preprocessors, parse_build_tools, preprocess_all, run_alex, run_happy, run_hsc2hs,
};
pub use server::{
    CompilationServer, ReloadResult, ServerConfig, ServerStatus, TypeCheckResult,
    is_server_running, server_socket_path,
};
pub use setup::{
    BuildFlags, ConfigureFlags, CopyFlags, CustomBuildResult, CustomSetupOptions,
    SetupCompileResult, SetupRunResult, build_with_setup, compile_setup, find_setup_file,
    run_build as run_setup_build, run_configure, run_copy, run_register,
};
