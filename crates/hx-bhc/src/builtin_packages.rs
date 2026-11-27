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
            haskell_name: "stm",
            bhc_crate: "bhc-concurrent",
            synthetic_version: "2.5.3",
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

/// `-D` flags defining `VERSION_<pkg>` and `MIN_VERSION_<pkg>` for each
/// package of a build plan, as cabal's `cabal_macros.h` does for GHC, so
/// version-conditional CPP (`#if MIN_VERSION_exceptions(0,10,6)`) selects the
/// code for the versions actually built against. Packages bhc provides are
/// left out: bhc defines their macros at its own versions.
pub fn version_macro_flags<'a>(
    packages: impl IntoIterator<Item = (&'a str, String)>,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut flags = Vec::new();
    for (name, version) in packages {
        if is_builtin(name) {
            continue;
        }
        let macro_name: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        if !seen.insert(macro_name.clone()) {
            continue;
        }
        let mut parts = version.split('.').map(|c| c.parse::<u64>().unwrap_or(0));
        let (a, b, c) = (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        );
        flags.push(format!("-DVERSION_{macro_name}=\"{version}\""));
        flags.push(format!(
            "-DMIN_VERSION_{macro_name}(major1,major2,minor)=((major1)<{a}||(major1)=={a}&&((major2)<{b}||(major2)=={b}&&(minor)<={c}))"
        ));
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_macros_for_non_builtin_packages() {
        let flags = version_macro_flags([
            ("exceptions", "0.10.12".to_string()),
            ("base", "4.20.0.0".to_string()),
            ("data-default", "0.8".to_string()),
        ]);
        assert!(flags.contains(&"-DVERSION_exceptions=\"0.10.12\"".to_string()));
        assert!(flags.iter().any(|f| f.starts_with(
            "-DMIN_VERSION_exceptions(major1,major2,minor)=((major1)<0||(major1)==0&&((major2)<10||(major2)==10&&(minor)<=12))"
        )));
        assert!(
            flags
                .iter()
                .any(|f| f.starts_with("-DMIN_VERSION_data_default("))
        );
        assert!(!flags.iter().any(|f| f.contains("_base")));
    }

    #[test]
    fn test_builtin_packages_contains_expected() {
        let builtins = builtin_packages();
        assert!(builtins.contains_key("base"));
        assert!(builtins.contains_key("text"));
        assert!(builtins.contains_key("containers"));
        assert!(builtins.contains_key("transformers"));
        assert!(builtins.contains_key("mtl"));
        assert!(builtins.contains_key("stm"));
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
