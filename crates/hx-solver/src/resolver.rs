//! Dependency resolver using backtracking search.
//!
//! This implements a modular constraint satisfaction solver that resolves
//! package dependencies by exploring version choices and backtracking
//! when conflicts are found.

use crate::package::{Dependency, InstallPlan, PackageIndex, ResolvedPackage};
use crate::version::{Version, VersionConstraint};
use std::collections::{HashMap, HashSet};
use tracing::{debug, trace};

/// Error type for resolution failures.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ResolveError {
    #[error(
        "package not found: {name}\n\n  The package '{name}' was not found in the Hackage index.\n\n  Suggestions:\n    - Check the spelling of the package name\n    - Run `cabal update` to refresh the package index\n    - Verify the package exists on Hackage: https://hackage.haskell.org/package/{name}"
    )]
    PackageNotFound { name: String },

    #[error("no version of {package} satisfies {constraint}\n\n  Available versions: {}\n\n  Suggestions:\n    - Relax the version constraint in your .cabal file\n    - Check if a newer version exists that satisfies other constraints", .available.join(", "))]
    NoMatchingVersion {
        package: String,
        constraint: VersionConstraint,
        available: Vec<String>,
    },

    #[error("version conflict for {package}\n\n  Required: {required}\n  But already selected: {selected}\n  Required by: {}\n\n  Suggestions:\n    - Check for conflicting version bounds in your dependencies\n    - Try allowing a wider version range", .required_by.join(", "))]
    VersionConflict {
        package: String,
        required: VersionConstraint,
        selected: Version,
        required_by: Vec<String>,
    },

    #[error("dependency cycle detected\n\n  Cycle: {}\n\n  This usually indicates a problem with package metadata on Hackage.", .0.join(" -> "))]
    CycleDetected(Vec<String>),

    #[error(
        "resolution exhausted after {attempts} attempts\n\n  The solver tried {attempts} combinations without finding a valid solution.\n\n  Suggestions:\n    - Simplify your dependencies\n    - Try pinning specific versions\n    - Check for known incompatibilities between packages"
    )]
    Exhausted { attempts: usize },
}

/// Packages that ship with GHC and cannot be reinstalled from Hackage — the
/// runtime system and the wired-in / compiler-internal libraries. cabal never
/// solves these from Hackage; their versions are fixed by the toolchain.
///
/// The resolver treats a dependency on them as toolchain-provided: if a version
/// was seeded into `ResolverConfig.installed` (from `ghc-pkg`), the constraint
/// is checked against it; otherwise the requirement is skipped rather than
/// failing with "package not found" (e.g. `rts` has no Hackage entry) or
/// chasing false cycles through them (e.g. `ghc-prim -> deepseq -> ghc-prim`).
///
/// Deliberately excludes reinstallable boot packages (containers, text,
/// bytestring, mtl, transformers, …): those resolve from Hackage so projects
/// can use newer versions than GHC bundles.
pub fn is_ghc_builtin(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "ghc"
            | "ghc-bignum"
            | "ghc-boot"
            | "ghc-boot-th"
            | "ghc-experimental"
            | "ghc-heap"
            | "ghc-internal"
            | "ghc-prim"
            | "ghci"
            | "integer-gmp"
            | "libiserv"
            | "rts"
            | "system-cxx-std-lib"
            | "template-haskell"
    )
}

/// Configuration for the resolver.
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    /// Maximum number of backtracking attempts before giving up
    pub max_attempts: usize,
    /// Prefer newer versions over older ones
    pub prefer_newest: bool,
    /// Packages that are already installed (don't need resolution)
    pub installed: HashMap<String, Version>,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            max_attempts: 100_000,
            prefer_newest: true,
            installed: HashMap::new(),
        }
    }
}

/// The dependency resolver.
pub struct Resolver<'a> {
    /// Package index with available versions
    index: &'a PackageIndex,
    /// Resolver configuration
    config: ResolverConfig,
}

impl<'a> Resolver<'a> {
    /// Create a new resolver with the given package index.
    pub fn new(index: &'a PackageIndex) -> Self {
        Self {
            index,
            config: ResolverConfig::default(),
        }
    }

    /// Create a resolver with custom configuration.
    pub fn with_config(index: &'a PackageIndex, config: ResolverConfig) -> Self {
        Self { index, config }
    }

    /// Resolve dependencies for a root package.
    pub fn resolve(
        &self,
        root: &str,
        constraint: &VersionConstraint,
    ) -> Result<InstallPlan, ResolveError> {
        let mut state = ResolverState::new(self.config.max_attempts);

        // Add the root dependency
        state.add_requirement(root.to_string(), constraint.clone(), None);

        // Copy installed packages to selected
        for (name, version) in &self.config.installed {
            state.selected.insert(name.clone(), version.clone());
        }

        // Run the resolution
        self.resolve_loop(&mut state)?;

        // Build the install plan
        Ok(state.to_install_plan(self.index))
    }

    /// Resolve multiple root dependencies.
    pub fn resolve_all(&self, deps: &[Dependency]) -> Result<InstallPlan, ResolveError> {
        let mut state = ResolverState::new(self.config.max_attempts);

        // Add all root dependencies
        for dep in deps {
            state.add_requirement(dep.name.clone(), dep.constraint.clone(), None);
        }

        // Copy installed packages
        for (name, version) in &self.config.installed {
            state.selected.insert(name.clone(), version.clone());
        }

        // Run the resolution
        self.resolve_loop(&mut state)?;

        Ok(state.to_install_plan(self.index))
    }

    /// Main resolution loop with backtracking.
    fn resolve_loop(&self, state: &mut ResolverState) -> Result<(), ResolveError> {
        while let Some(req) = state.next_requirement() {
            state.attempts += 1;
            if state.attempts > state.max_attempts {
                return Err(ResolveError::Exhausted {
                    attempts: state.attempts,
                });
            }

            trace!("Resolving requirement: {} {}", req.package, req.constraint);

            // Check if already selected
            if let Some(selected_version) = state.selected.get(&req.package).cloned() {
                // Verify the selected version satisfies this constraint
                if !req.constraint.matches(&selected_version) {
                    debug!(
                        "Conflict: {} {} doesn't satisfy {}",
                        req.package, selected_version, req.constraint
                    );

                    // Try to backtrack
                    if !self.backtrack(state) {
                        let required_by = req
                            .parent
                            .clone()
                            .map(|p| vec![p])
                            .unwrap_or_else(|| vec!["(root)".to_string()]);
                        return Err(ResolveError::VersionConflict {
                            package: req.package,
                            required: req.constraint,
                            selected: selected_version,
                            required_by,
                        });
                    }
                    continue;
                }
                // Already selected and satisfies constraint, continue
                continue;
            }

            // GHC-bundled, non-reinstallable packages that weren't seeded with
            // a concrete version: treat as toolchain-provided and skip, rather
            // than failing to find them on Hackage or cycling through them.
            if is_ghc_builtin(&req.package) {
                trace!("Skipping GHC-bundled package: {}", req.package);
                continue;
            }

            // Find matching versions
            let package = match self.index.get_package(&req.package) {
                Some(p) => p,
                None => {
                    // A project's own (root) dependency that isn't on Hackage is
                    // a real error — most likely a typo.
                    if req.parent.is_none() {
                        return Err(ResolveError::PackageNotFound {
                            name: req.package.clone(),
                        });
                    }
                    // Transitive deps can come from `.cabal` conditionals/flags
                    // this parser doesn't evaluate — e.g. the `unbuildable`
                    // sentinel used in an `else` branch to disable a component.
                    // Skip rather than aborting the whole resolution.
                    debug!(
                        "Skipping unresolvable transitive dependency '{}' (from {:?})",
                        req.package, req.parent
                    );
                    continue;
                }
            };

            let mut candidates = package.matching_versions(&req.constraint);

            if candidates.is_empty() {
                // No matching version - try backtracking
                debug!("No matching version for {} {}", req.package, req.constraint);

                if !self.backtrack(state) {
                    let available = package
                        .versions_descending()
                        .iter()
                        .map(|v| v.to_string())
                        .collect();
                    return Err(ResolveError::NoMatchingVersion {
                        package: req.package,
                        constraint: req.constraint,
                        available,
                    });
                }
                continue;
            }

            // Sort by preference (newest first if prefer_newest)
            if !self.config.prefer_newest {
                candidates.reverse();
            }

            // Create a choice point and select the first candidate
            let version = candidates[0].clone();
            let alternatives: Vec<Version> = candidates[1..].iter().map(|v| (*v).clone()).collect();

            state.make_choice(&req.package, version.clone(), alternatives);

            debug!("Selected {} {}", req.package, version);

            // Add dependencies of the selected version. Cycles in package
            // metadata are absorbed naturally: a dependency that is already
            // selected hits the "already selected" branch above and is skipped,
            // so there is no infinite loop to guard against, and `max_attempts`
            // bounds pathological cases. (The previous explicit check compared
            // against a flat *selection log* as if it were a DFS path, so any
            // diamond — e.g. `deepseq` depending on both `array` and `text`,
            // with `text` also depending on `array` — was flagged as a false
            // cycle.)
            if let Some(pv) = package.get_version(&version) {
                for dep in &pv.dependencies {
                    state.add_requirement(
                        dep.name.clone(),
                        dep.constraint.clone(),
                        Some(&req.package),
                    );
                }
            }
        }

        Ok(())
    }

    /// Attempt to backtrack to a previous choice point.
    fn backtrack(&self, state: &mut ResolverState) -> bool {
        while let Some(choice) = state.choices.pop() {
            debug!("Backtracking: undoing choice for {}", choice.package);

            // Undo the selection
            state.selected.remove(&choice.package);

            // Remove any requirements added after this choice
            state.pending.retain(|r| r.depth <= choice.depth);

            // Try the next alternative
            if !choice.alternatives.is_empty() {
                let next_version = choice.alternatives[0].clone();
                let remaining: Vec<Version> = choice.alternatives[1..].to_vec();

                // Re-add the requirement
                state.add_requirement(
                    choice.package.clone(),
                    VersionConstraint::Exact(next_version),
                    choice.parent.as_deref(),
                );

                // Save new choice point if there are more alternatives
                if !remaining.is_empty() {
                    let pkg_name = choice.package.clone();
                    let version = state.selected.get(&pkg_name).cloned().unwrap_or_default();
                    state.choices.push(ChoicePoint {
                        package: choice.package,
                        version,
                        alternatives: remaining,
                        depth: choice.depth,
                        parent: choice.parent,
                    });
                }

                return true;
            }
        }

        false
    }
}

/// A pending requirement to resolve.
#[derive(Debug, Clone)]
struct Requirement {
    package: String,
    constraint: VersionConstraint,
    #[allow(dead_code)] // Useful for debugging resolution paths
    parent: Option<String>,
    depth: usize,
}

/// A choice point for backtracking.
#[derive(Debug, Clone)]
struct ChoicePoint {
    package: String,
    #[allow(dead_code)] // Stored for potential rollback debugging
    version: Version,
    alternatives: Vec<Version>,
    depth: usize,
    parent: Option<String>,
}

impl Default for Version {
    fn default() -> Self {
        Version::new(vec![0])
    }
}

/// Internal state of the resolver.
struct ResolverState {
    /// Pending requirements to process
    pending: Vec<Requirement>,
    /// Selected versions
    selected: HashMap<String, Version>,
    /// Choice points for backtracking
    choices: Vec<ChoicePoint>,
    /// Current resolution depth
    depth: usize,
    /// Number of attempts made
    attempts: usize,
    /// Maximum attempts allowed
    max_attempts: usize,
}

impl ResolverState {
    fn new(max_attempts: usize) -> Self {
        Self {
            pending: Vec::new(),
            selected: HashMap::new(),
            choices: Vec::new(),
            depth: 0,
            attempts: 0,
            max_attempts,
        }
    }

    fn add_requirement(
        &mut self,
        package: String,
        constraint: VersionConstraint,
        parent: Option<&str>,
    ) {
        self.pending.push(Requirement {
            package,
            constraint,
            parent: parent.map(|s| s.to_string()),
            depth: self.depth,
        });
    }

    fn next_requirement(&mut self) -> Option<Requirement> {
        self.pending.pop()
    }

    fn make_choice(&mut self, package: &str, version: Version, alternatives: Vec<Version>) {
        self.selected.insert(package.to_string(), version.clone());
        self.depth += 1;

        if !alternatives.is_empty() {
            self.choices.push(ChoicePoint {
                package: package.to_string(),
                version,
                alternatives,
                depth: self.depth,
                parent: None,
            });
        }
    }

    fn to_install_plan(&self, index: &PackageIndex) -> InstallPlan {
        let mut plan = InstallPlan::new();
        let mut visited = HashSet::new();

        // Build plan in dependency order using DFS
        for (name, version) in &self.selected {
            self.add_to_plan_recursive(name, version, index, &mut plan, &mut visited);
        }

        plan
    }

    fn add_to_plan_recursive(
        &self,
        name: &str,
        version: &Version,
        index: &PackageIndex,
        plan: &mut InstallPlan,
        visited: &mut HashSet<String>,
    ) {
        if visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());

        // Get dependencies and add them first
        let mut dep_names = Vec::new();
        if let Some(pv) = index.get_package_version(name, version) {
            for dep in &pv.dependencies {
                if let Some(dep_version) = self.selected.get(&dep.name) {
                    self.add_to_plan_recursive(&dep.name, dep_version, index, plan, visited);
                    dep_names.push(dep.name.clone());
                }
            }
        }

        plan.add(ResolvedPackage {
            name: name.to_string(),
            version: version.clone(),
            dependencies: dep_names,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageVersion;

    fn create_test_index() -> PackageIndex {
        let mut index = PackageIndex::new();

        // base package (simulating GHC base)
        let base_4_18 = PackageVersion::new("base", "4.18.0".parse().unwrap());
        index.add_package_version(base_4_18);

        // text package
        let mut text_2_0 = PackageVersion::new("text", "2.0.0".parse().unwrap());
        text_2_0.add_dependency(Dependency::with_constraint(
            "base",
            VersionConstraint::GreaterThanOrEqual("4.16".parse().unwrap()),
        ));
        index.add_package_version(text_2_0);

        let mut text_2_1 = PackageVersion::new("text", "2.1.0".parse().unwrap());
        text_2_1.add_dependency(Dependency::with_constraint(
            "base",
            VersionConstraint::GreaterThanOrEqual("4.17".parse().unwrap()),
        ));
        index.add_package_version(text_2_1);

        // aeson depends on text
        let mut aeson_2_2 = PackageVersion::new("aeson", "2.2.0".parse().unwrap());
        aeson_2_2.add_dependency(Dependency::with_constraint(
            "base",
            VersionConstraint::GreaterThanOrEqual("4.17".parse().unwrap()),
        ));
        aeson_2_2.add_dependency(Dependency::with_constraint(
            "text",
            VersionConstraint::GreaterThanOrEqual("2.0".parse().unwrap()),
        ));
        index.add_package_version(aeson_2_2);

        index
    }

    #[test]
    fn test_resolve_simple() {
        let index = create_test_index();
        let resolver = Resolver::new(&index);

        let plan = resolver.resolve("text", &VersionConstraint::Any).unwrap();

        assert!(plan.get_version("text").is_some());
        // `base` is a GHC-bundled package: provided by the toolchain, not
        // resolved into the install plan.
        assert!(plan.get_version("base").is_none());
    }

    #[test]
    fn test_resolve_with_constraint() {
        let index = create_test_index();
        let resolver = Resolver::new(&index);

        let plan = resolver
            .resolve(
                "text",
                &VersionConstraint::GreaterThanOrEqual("2.1".parse().unwrap()),
            )
            .unwrap();

        let text_version = plan.get_version("text").unwrap();
        assert_eq!(text_version.to_string(), "2.1.0");
    }

    #[test]
    fn test_resolve_transitive() {
        let index = create_test_index();
        let resolver = Resolver::new(&index);

        let plan = resolver.resolve("aeson", &VersionConstraint::Any).unwrap();

        assert!(plan.get_version("aeson").is_some());
        assert!(plan.get_version("text").is_some());
        // `base` is toolchain-provided and intentionally absent from the plan.
        assert!(plan.get_version("base").is_none());
    }

    #[test]
    fn test_resolve_not_found() {
        let index = create_test_index();
        let resolver = Resolver::new(&index);

        let result = resolver.resolve("nonexistent", &VersionConstraint::Any);
        assert!(matches!(result, Err(ResolveError::PackageNotFound { .. })));
    }

    #[test]
    fn test_resolve_diamond_is_not_a_false_cycle() {
        // A diamond (top -> {left, right}; left -> shared; right -> shared)
        // must resolve, not be misreported as a cycle. Regression for the old
        // detector that treated the flat selection log as a DFS path.
        let mut index = PackageIndex::new();
        index.add_package_version(PackageVersion::new("shared", "1.0".parse().unwrap()));
        let mut left = PackageVersion::new("left", "1.0".parse().unwrap());
        left.add_dependency(Dependency::new("shared"));
        index.add_package_version(left);
        let mut right = PackageVersion::new("right", "1.0".parse().unwrap());
        right.add_dependency(Dependency::new("shared"));
        index.add_package_version(right);
        let mut top = PackageVersion::new("top", "1.0".parse().unwrap());
        top.add_dependency(Dependency::new("left"));
        top.add_dependency(Dependency::new("right"));
        index.add_package_version(top);

        let resolver = Resolver::new(&index);
        let plan = resolver.resolve("top", &VersionConstraint::Any).unwrap();
        assert!(plan.get_version("shared").is_some());
        assert!(plan.get_version("left").is_some());
        assert!(plan.get_version("right").is_some());
    }

    #[test]
    fn test_resolve_no_matching_version() {
        let index = create_test_index();
        let resolver = Resolver::new(&index);

        let result = resolver.resolve(
            "text",
            &VersionConstraint::GreaterThanOrEqual("99.0".parse().unwrap()),
        );
        assert!(matches!(
            result,
            Err(ResolveError::NoMatchingVersion { .. })
        ));
    }
}
