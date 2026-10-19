//! Integration tests for the solver.
//!
//! These tests require a Hackage index to be present.
//! Run `cabal update` first to download the index.
//!
//! Run with: cargo test -p hx-solver --test integration

use hx_solver::{
    IndexOptions, PlanOptions, Resolver, ResolverConfig, VersionConstraint, default_index_path,
    generate_build_plan, load_index,
};

/// Check if the Hackage index is available.
fn hackage_index_available() -> bool {
    default_index_path().is_some()
}

/// Skip test if Hackage index is not available.
macro_rules! require_hackage_index {
    () => {
        if !hackage_index_available() {
            eprintln!("Skipping test: Hackage index not found. Run `cabal update` first.");
            return;
        }
    };
}

#[test]
fn test_load_hackage_index() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    // Only load `base` for speed. (A `max_packages` limit would load the
    // oldest tar entries, which need not include any particular package.)
    let options = IndexOptions {
        filter_packages: vec!["base".to_string()],
        ..Default::default()
    };

    let index = load_index(&index_path, &options).expect("Failed to load index");

    assert!(index.package_count() > 0);
    assert!(index.version_count() > 0);

    // Check for well-known packages
    assert!(
        index.get_package("base").is_some(),
        "base package should exist"
    );
}

#[test]
fn test_resolve_real_package_text() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions::default();
    let index = load_index(&index_path, &options).expect("Failed to load index");

    let resolver = Resolver::new(&index);
    let plan = resolver
        .resolve("text", &VersionConstraint::Any)
        .expect("Failed to resolve text");

    assert!(!plan.packages.is_empty());

    // text should be in the plan
    assert!(
        plan.get_version("text").is_some(),
        "text should be in the plan"
    );

    // Reinstallable dependencies are resolved from Hackage. (`base` is
    // toolchain-provided and only appears when seeded as installed.)
    assert!(
        plan.get_version("bytestring").is_some(),
        "bytestring should be in the plan"
    );
}

#[test]
fn test_resolve_real_package_aeson() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions::default();
    let index = load_index(&index_path, &options).expect("Failed to load index");

    let resolver = Resolver::new(&index);
    let plan = resolver
        .resolve("aeson", &VersionConstraint::Any)
        .expect("Failed to resolve aeson");

    // aeson has many transitive dependencies
    assert!(
        plan.packages.len() > 5,
        "aeson should have multiple dependencies"
    );

    // Check for known aeson dependencies
    assert!(plan.get_version("aeson").is_some());
    assert!(plan.get_version("text").is_some());
}

#[test]
fn test_resolve_with_version_constraint() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions::default();
    let index = load_index(&index_path, &options).expect("Failed to load index");

    let resolver = Resolver::new(&index);

    // Resolve text with a specific version range (>= 2.0 && < 2.2)
    let constraint = VersionConstraint::GreaterThanOrEqual("2.0".parse().unwrap())
        .and(VersionConstraint::LessThan("2.2".parse().unwrap()));

    let plan = resolver
        .resolve("text", &constraint)
        .expect("Failed to resolve text with constraint");

    let text_version = plan.get_version("text").unwrap();

    // Version should be >= 2.0 and < 2.2
    assert!(
        text_version >= &"2.0".parse().unwrap(),
        "text version should be >= 2.0"
    );
    assert!(
        text_version < &"2.2".parse().unwrap(),
        "text version should be < 2.2"
    );
}

#[test]
fn test_resolve_prefer_oldest() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions::default();
    let index = load_index(&index_path, &options).expect("Failed to load index");

    // Resolve with prefer_newest = false
    let config = ResolverConfig {
        prefer_newest: false,
        ..Default::default()
    };
    let resolver = Resolver::with_config(&index, config);

    let plan = resolver
        .resolve("text", &VersionConstraint::Any)
        .expect("Failed to resolve text");

    // Should get an older version (exact version depends on index state)
    let text_version = plan.get_version("text").unwrap();
    println!("Resolved text version (prefer oldest): {}", text_version);
}

#[test]
fn test_generate_build_plan_real() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions::default();
    let index = load_index(&index_path, &options).expect("Failed to load index");

    let resolver = Resolver::new(&index);
    let install_plan = resolver
        .resolve("text", &VersionConstraint::Any)
        .expect("Failed to resolve text");

    let plan_options = PlanOptions::default();
    let build_plan =
        generate_build_plan(&install_plan, &plan_options).expect("Failed to generate build plan");

    assert!(!build_plan.packages.is_empty());
    assert!(!build_plan.fingerprint.is_empty());
    assert!(!build_plan.compiler_id.is_empty());
    assert!(!build_plan.platform.is_empty());

    // base should be pre-installed
    let base = build_plan.packages.iter().find(|p| p.name == "base");
    if let Some(base) = base {
        assert!(base.pre_installed);
    }
}

#[test]
fn test_index_loading_with_progress() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions {
        max_packages: 50, // Small limit for testing
        show_progress: true,
        ..Default::default()
    };

    let index = load_index(&index_path, &options).expect("Failed to load index with progress");
    assert!(index.package_count() > 0);
}

#[test]
fn test_resolve_nonexistent_package() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions::default();
    let index = load_index(&index_path, &options).expect("Failed to load index");

    let resolver = Resolver::new(&index);
    let result = resolver.resolve(
        "this-package-definitely-does-not-exist-12345",
        &VersionConstraint::Any,
    );

    assert!(result.is_err());
}

#[test]
fn test_resolve_impossible_constraint() {
    require_hackage_index!();

    let index_path = default_index_path().unwrap();
    let options = IndexOptions::default();
    let index = load_index(&index_path, &options).expect("Failed to load index");

    let resolver = Resolver::new(&index);

    // Try to resolve with an impossible version constraint
    let constraint = VersionConstraint::GreaterThanOrEqual("999.0.0".parse().unwrap());
    let result = resolver.resolve("text", &constraint);

    assert!(result.is_err());
}
