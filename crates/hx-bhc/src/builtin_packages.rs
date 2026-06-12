//! BHC builtin package mapping.
//!
//! Maps standard Haskell package names to BHC's built-in equivalents.
//! When building with BHC, these packages are provided by the compiler's
//! standard library and do not need to be compiled from source.

use std::collections::HashMap;

/// A BHC builtin package entry.
#[derive(Debug, Clone)]
pub struct BuiltinPackage {
    /// The Haskell package name (e.g. "base").
    pub haskell_name: &'static str,
    /// The BHC crate that provides this package (e.g. "bhc-base").
    pub bhc_crate: &'static str,
    /// The synthetic package ID to register (e.g. "base-4.19.0-bhc-builtin").
    pub synthetic_version: &'static str,
}

/// Return the mapping of Haskell package names to BHC builtins.
///
/// These packages are provided by BHC's standard library and do not need
/// to be compiled from source during `hx build --backend bhc`.
pub fn builtin_packages() -> HashMap<&'static str, BuiltinPackage> {
    let entries = [
        BuiltinPackage {
            haskell_name: "base",
            bhc_crate: "bhc-base",
            synthetic_version: "4.19.0",
        },
        BuiltinPackage {
            haskell_name: "ghc-prim",
            bhc_crate: "bhc-prelude",
            synthetic_version: "0.11.0",
        },
        BuiltinPackage {
            haskell_name: "integer-gmp",
            bhc_crate: "bhc-prelude",
            synthetic_version: "1.1",
        },
        BuiltinPackage {
            haskell_name: "text",
            bhc_crate: "bhc-text",
            synthetic_version: "2.1.0",
        },
        BuiltinPackage {
            haskell_name: "bytestring",
            bhc_crate: "bhc-text",
            synthetic_version: "0.12.1",
        },
        BuiltinPackage {
            haskell_name: "containers",
            bhc_crate: "bhc-containers",
            synthetic_version: "0.7.0",
        },
        BuiltinPackage {
            haskell_name: "transformers",
            bhc_crate: "bhc-transformers",
            synthetic_version: "0.6.1",
        },
        BuiltinPackage {
            haskell_name: "mtl",
            bhc_crate: "bhc-transformers",
            synthetic_version: "2.3.1",
        },
        BuiltinPackage {
            haskell_name: "array",
            bhc_crate: "bhc-base",
            synthetic_version: "0.5.6",
        },
        BuiltinPackage {
            haskell_name: "deepseq",
            bhc_crate: "bhc-base",
            synthetic_version: "1.5.0",
        },
        BuiltinPackage {
            haskell_name: "directory",
            bhc_crate: "bhc-system",
            synthetic_version: "1.3.8",
        },
        BuiltinPackage {
            haskell_name: "filepath",
            bhc_crate: "bhc-system",
            synthetic_version: "1.4.200",
        },
    ];

    entries.into_iter().map(|e| (e.haskell_name, e)).collect()
}

/// Check if a package name is a BHC builtin.
pub fn is_builtin(name: &str) -> bool {
    builtin_packages().contains_key(name)
}

/// Get the synthetic package ID for a BHC builtin package.
///
/// Returns a deterministic ID like `base-4.19.0-bhc-builtin` that can be
/// used as a package identifier in the build plan.
pub fn builtin_package_id(name: &str) -> Option<String> {
    builtin_packages()
        .get(name)
        .map(|pkg| format!("{}-{}-bhc-builtin", pkg.haskell_name, pkg.synthetic_version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_packages_contains_expected() {
        let builtins = builtin_packages();
        assert!(builtins.contains_key("base"));
        assert!(builtins.contains_key("text"));
        assert!(builtins.contains_key("containers"));
        assert!(builtins.contains_key("transformers"));
        assert!(builtins.contains_key("mtl"));
        assert!(builtins.contains_key("deepseq"));
        assert!(builtins.contains_key("ghc-prim"));
        assert!(!builtins.contains_key("aeson"));
    }

    #[test]
    fn test_is_builtin() {
        assert!(is_builtin("base"));
        assert!(is_builtin("text"));
        assert!(is_builtin("bytestring"));
        assert!(!is_builtin("aeson"));
        assert!(!is_builtin("pandoc"));
    }

    #[test]
    fn test_builtin_package_id() {
        assert_eq!(
            builtin_package_id("base"),
            Some("base-4.19.0-bhc-builtin".to_string())
        );
        assert_eq!(
            builtin_package_id("text"),
            Some("text-2.1.0-bhc-builtin".to_string())
        );
        assert_eq!(builtin_package_id("aeson"), None);
    }
}
