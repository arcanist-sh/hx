//! Build plan generation for Haskell packages.
//!
//! This module generates build plans from resolved dependencies,
//! determining the order packages should be built and their configurations.

use crate::package::InstallPlan;
use crate::version::Version;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// Error type for build plan operations.
#[derive(Debug, Error)]
pub enum PlanError {
    #[error("cycle detected in dependencies: {}", .0.join(" -> "))]
    CycleDetected(Vec<String>),

    #[error("missing dependency: {0} requires {1}")]
    MissingDependency(String, String),
}

/// A build plan with packages in topological order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildPlan {
    /// Compiler ID (e.g., "ghc-9.8.2")
    pub compiler_id: String,
    /// Platform (e.g., "x86_64-darwin")
    pub platform: String,
    /// Packages in build order (dependencies first)
    pub packages: Vec<BuildUnit>,
    /// Plan fingerprint for caching
    pub fingerprint: String,
}

/// A single unit to build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildUnit {
    /// Package name
    pub name: String,
    /// Package version
    pub version: Version,
    /// Package hash (for content-addressable store)
    pub hash: Option<String>,
    /// Build flags
    pub flags: HashMap<String, bool>,
    /// Whether this is a pre-installed package
    pub pre_installed: bool,
    /// Build style
    pub style: BuildStyle,
    /// Dependencies (package names)
    pub depends: Vec<String>,
}

/// How a package should be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildStyle {
    /// Build from source
    Source,
    /// Already installed (e.g., base packages)
    PreInstalled,
    /// Use cached build from store
    Cached,
}

/// Options for build plan generation.
#[derive(Debug, Clone)]
pub struct PlanOptions {
    /// Compiler ID
    pub compiler_id: String,
    /// Target platform
    pub platform: String,
    /// Pre-installed packages (come with GHC)
    pub pre_installed: HashSet<String>,
    /// Cached package hashes (from store)
    pub cached_hashes: HashMap<String, String>,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            compiler_id: detect_compiler_id(),
            platform: detect_platform(),
            pre_installed: default_pre_installed(),
            cached_hashes: HashMap::new(),
        }
    }
}

/// Generate a build plan from an install plan.
pub fn generate_build_plan(
    install_plan: &InstallPlan,
    options: &PlanOptions,
) -> Result<BuildPlan, PlanError> {
    // Build dependency graph
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut versions: HashMap<String, Version> = HashMap::new();

    for pkg in &install_plan.packages {
        graph.insert(pkg.name.clone(), pkg.dependencies.clone());
        versions.insert(pkg.name.clone(), pkg.version.clone());
    }

    // Topological sort
    let sorted = topological_sort(&graph)?;

    // Generate build units
    let mut packages = Vec::new();
    for name in sorted {
        let version = versions.get(&name).cloned().unwrap_or_default();
        let depends = graph.get(&name).cloned().unwrap_or_default();

        let (style, pre_installed) = if options.pre_installed.contains(&name) {
            (BuildStyle::PreInstalled, true)
        } else if options
            .cached_hashes
            .contains_key(&format!("{}-{}", name, version))
        {
            (BuildStyle::Cached, false)
        } else {
            (BuildStyle::Source, false)
        };

        let hash = options
            .cached_hashes
            .get(&format!("{}-{}", name, version))
            .cloned();

        packages.push(BuildUnit {
            name,
            version,
            hash,
            flags: HashMap::new(),
            pre_installed,
            style,
            depends,
        });
    }

    // Compute fingerprint
    let fingerprint = compute_plan_fingerprint(&packages, &options.compiler_id, &options.platform);

    Ok(BuildPlan {
        compiler_id: options.compiler_id.clone(),
        platform: options.platform.clone(),
        packages,
        fingerprint,
    })
}

/// Topological sort of packages.
fn topological_sort(graph: &HashMap<String, Vec<String>>) -> Result<Vec<String>, PlanError> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();

    fn visit(
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        in_progress: &mut HashSet<String>,
        result: &mut Vec<String>,
        path: &mut Vec<String>,
    ) -> Result<(), PlanError> {
        if visited.contains(node) {
            return Ok(());
        }

        if in_progress.contains(node) {
            path.push(node.to_string());
            return Err(PlanError::CycleDetected(path.clone()));
        }

        in_progress.insert(node.to_string());
        path.push(node.to_string());

        if let Some(deps) = graph.get(node) {
            for dep in deps {
                visit(dep, graph, visited, in_progress, result, path)?;
            }
        }

        path.pop();
        in_progress.remove(node);
        visited.insert(node.to_string());
        result.push(node.to_string());

        Ok(())
    }

    for node in graph.keys() {
        let mut path = Vec::new();
        visit(
            node,
            graph,
            &mut visited,
            &mut in_progress,
            &mut result,
            &mut path,
        )?;
    }

    Ok(result)
}

/// Compute a fingerprint for the build plan.
fn compute_plan_fingerprint(packages: &[BuildUnit], compiler_id: &str, platform: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(format!("compiler:{}", compiler_id));
    hasher.update(format!("platform:{}", platform));

    for pkg in packages {
        hasher.update(format!("pkg:{}@{}", pkg.name, pkg.version));
        if let Some(hash) = &pkg.hash {
            hasher.update(format!("hash:{}", hash));
        }
    }

    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Detect the compiler ID.
fn detect_compiler_id() -> String {
    // Try to detect from environment or default
    std::env::var("GHC_VERSION")
        .map(|v| format!("ghc-{}", v))
        .unwrap_or_else(|_| "ghc-9.8.2".to_string())
}

/// Detect the platform.
fn detect_platform() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    format!("{}-{}", arch, os)
}

/// Default pre-installed packages (come with GHC).
fn default_pre_installed() -> HashSet<String> {
    [
        "base",
        "ghc-prim",
        "ghc-bignum",
        "integer-gmp",
        "integer-simple",
        "template-haskell",
        "ghc-boot",
        "ghc-boot-th",
        "ghc-heap",
        "ghci",
        "hpc",
        "transformers",
        "array",
        "binary",
        "bytestring",
        "containers",
        "deepseq",
        "directory",
        "exceptions",
        "filepath",
        "mtl",
        "parsec",
        "pretty",
        "process",
        "stm",
        "text",
        "time",
        "unix",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Summary of a build plan.
#[derive(Debug, Default)]
pub struct PlanSummary {
    /// Total packages
    pub total: usize,
    /// Pre-installed packages (skipped)
    pub pre_installed: usize,
    /// Cached packages (from store)
    pub cached: usize,
    /// Packages to build from source
    pub to_build: usize,
}

impl PlanSummary {
    /// Create summary from build plan.
    pub fn from_plan(plan: &BuildPlan) -> Self {
        let mut summary = PlanSummary {
            total: plan.packages.len(),
            ..Default::default()
        };

        for pkg in &plan.packages {
            match pkg.style {
                BuildStyle::PreInstalled => summary.pre_installed += 1,
                BuildStyle::Cached => summary.cached += 1,
                BuildStyle::Source => summary.to_build += 1,
            }
        }

        summary
    }
}

#[cfg(test)]
mod hash_golden_tests {
    //! Plan fingerprints are cache keys. Pinned to exact output so a hashing-library upgrade cannot
    //! silently change them.
    use super::*;

    #[test]
    fn plan_fingerprint_is_stable() {
        let unit = |name: &str, version: &str, hash: Option<&str>| BuildUnit {
            name: name.to_string(),
            version: version.parse().unwrap(),
            hash: hash.map(str::to_string),
            flags: HashMap::new(),
            pre_installed: false,
            style: BuildStyle::Source,
            depends: vec![],
        };
        let packages = vec![
            unit("text", "2.1.1", Some("abc")),
            unit("aeson", "2.2.3.0", None),
        ];
        assert_eq!(
            compute_plan_fingerprint(&packages, "ghc-9.8.2", "aarch64-osx"),
            "ea5a9320cc56971bdd60efb45c889b1010a9db17b4430692a51c302282f1fe7f"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::ResolvedPackage;

    #[test]
    fn test_topological_sort() {
        let mut graph = HashMap::new();
        graph.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
        graph.insert("b".to_string(), vec!["c".to_string()]);
        graph.insert("c".to_string(), vec![]);

        let sorted = topological_sort(&graph).unwrap();

        // c must come before b, b must come before a
        let c_pos = sorted.iter().position(|x| x == "c").unwrap();
        let b_pos = sorted.iter().position(|x| x == "b").unwrap();
        let a_pos = sorted.iter().position(|x| x == "a").unwrap();

        assert!(c_pos < b_pos);
        assert!(b_pos < a_pos);
    }

    #[test]
    fn test_topological_sort_single_node() {
        let mut graph = HashMap::new();
        graph.insert("a".to_string(), vec![]);

        let sorted = topological_sort(&graph).unwrap();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0], "a");
    }

    #[test]
    fn test_topological_sort_diamond() {
        // Diamond dependency: a -> b, a -> c, b -> d, c -> d
        let mut graph = HashMap::new();
        graph.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
        graph.insert("b".to_string(), vec!["d".to_string()]);
        graph.insert("c".to_string(), vec!["d".to_string()]);
        graph.insert("d".to_string(), vec![]);

        let sorted = topological_sort(&graph).unwrap();

        // d must come before b and c, which must come before a
        let d_pos = sorted.iter().position(|x| x == "d").unwrap();
        let b_pos = sorted.iter().position(|x| x == "b").unwrap();
        let c_pos = sorted.iter().position(|x| x == "c").unwrap();
        let a_pos = sorted.iter().position(|x| x == "a").unwrap();

        assert!(d_pos < b_pos);
        assert!(d_pos < c_pos);
        assert!(b_pos < a_pos);
        assert!(c_pos < a_pos);
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = HashMap::new();
        graph.insert("a".to_string(), vec!["b".to_string()]);
        graph.insert("b".to_string(), vec!["c".to_string()]);
        graph.insert("c".to_string(), vec!["a".to_string()]);

        let result = topological_sort(&graph);
        assert!(matches!(result, Err(PlanError::CycleDetected(_))));
    }

    #[test]
    fn test_self_cycle_detection() {
        let mut graph = HashMap::new();
        graph.insert("a".to_string(), vec!["a".to_string()]);

        let result = topological_sort(&graph);
        assert!(matches!(result, Err(PlanError::CycleDetected(_))));
    }

    #[test]
    fn test_plan_summary() {
        let plan = BuildPlan {
            compiler_id: "ghc-9.8.2".to_string(),
            platform: "x86_64-linux".to_string(),
            fingerprint: "test".to_string(),
            packages: vec![
                BuildUnit {
                    name: "base".to_string(),
                    version: "4.18.0".parse().unwrap(),
                    hash: None,
                    flags: HashMap::new(),
                    pre_installed: true,
                    style: BuildStyle::PreInstalled,
                    depends: vec![],
                },
                BuildUnit {
                    name: "text".to_string(),
                    version: "2.1.0".parse().unwrap(),
                    hash: Some("abc123".to_string()),
                    flags: HashMap::new(),
                    pre_installed: false,
                    style: BuildStyle::Cached,
                    depends: vec!["base".to_string()],
                },
                BuildUnit {
                    name: "aeson".to_string(),
                    version: "2.2.0".parse().unwrap(),
                    hash: None,
                    flags: HashMap::new(),
                    pre_installed: false,
                    style: BuildStyle::Source,
                    depends: vec!["base".to_string(), "text".to_string()],
                },
            ],
        };

        let summary = PlanSummary::from_plan(&plan);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.pre_installed, 1);
        assert_eq!(summary.cached, 1);
        assert_eq!(summary.to_build, 1);
    }

    #[test]
    fn test_plan_options_default() {
        let options = PlanOptions::default();
        assert!(!options.compiler_id.is_empty());
        assert!(!options.platform.is_empty());
        assert!(options.pre_installed.contains("base"));
        assert!(options.pre_installed.contains("ghc-prim"));
    }

    #[test]
    fn test_detect_platform() {
        let platform = detect_platform();
        assert!(platform.contains("-")); // Should be arch-os format
    }

    #[test]
    fn test_generate_build_plan() {
        let mut install_plan = InstallPlan::new();
        install_plan.add(ResolvedPackage {
            name: "base".to_string(),
            version: "4.18.0".parse().unwrap(),
            dependencies: vec![],
        });
        install_plan.add(ResolvedPackage {
            name: "aeson".to_string(),
            version: "2.2.0".parse().unwrap(),
            dependencies: vec!["base".to_string()],
        });

        let options = PlanOptions::default();
        let build_plan = generate_build_plan(&install_plan, &options).unwrap();

        assert_eq!(build_plan.packages.len(), 2);
        assert!(!build_plan.fingerprint.is_empty());

        // base should be pre-installed
        let base_unit = build_plan
            .packages
            .iter()
            .find(|p| p.name == "base")
            .unwrap();
        assert_eq!(base_unit.style, BuildStyle::PreInstalled);

        // aeson should be source
        let aeson_unit = build_plan
            .packages
            .iter()
            .find(|p| p.name == "aeson")
            .unwrap();
        assert_eq!(aeson_unit.style, BuildStyle::Source);
    }

    #[test]
    fn test_build_plan_fingerprint_changes() {
        let plan1 = BuildPlan {
            compiler_id: "ghc-9.8.2".to_string(),
            platform: "x86_64-linux".to_string(),
            fingerprint: "ignored".to_string(),
            packages: vec![BuildUnit {
                name: "test".to_string(),
                version: "1.0.0".parse().unwrap(),
                hash: None,
                flags: HashMap::new(),
                pre_installed: false,
                style: BuildStyle::Source,
                depends: vec![],
            }],
        };

        let plan2 = BuildPlan {
            compiler_id: "ghc-9.6.4".to_string(), // Different compiler
            platform: "x86_64-linux".to_string(),
            fingerprint: "ignored".to_string(),
            packages: vec![BuildUnit {
                name: "test".to_string(),
                version: "1.0.0".parse().unwrap(),
                hash: None,
                flags: HashMap::new(),
                pre_installed: false,
                style: BuildStyle::Source,
                depends: vec![],
            }],
        };

        let fp1 = compute_plan_fingerprint(&plan1.packages, &plan1.compiler_id, &plan1.platform);
        let fp2 = compute_plan_fingerprint(&plan2.packages, &plan2.compiler_id, &plan2.platform);

        assert_ne!(fp1, fp2);
    }
}
