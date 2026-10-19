//! Configuration merging utilities.
//!
//! This module provides the `Combine` trait for merging configuration from
//! multiple sources, following Cargo's config.toml merging rules:
//!
//! - Numbers, strings, booleans: higher precedence config takes precedence
//! - Arrays: merged with higher precedence items placed earlier
//! - `Option<T>`: first `Some` value wins

use crate::{
    BhcConfig, BhcPlatformConfig, BuildConfig, CompilerConfig, FormatConfig, GhcConfig, LintConfig,
    Manifest, PluginConfig, PluginHookConfig, ProjectConfig, StackageConfig, ToolchainConfig,
};

/// Trait for combining configuration values.
///
/// The convention is that `self` has higher precedence than `other`.
pub trait Combine {
    /// Combine two values, preferring values in `self`.
    #[must_use]
    fn combine(self, other: Self) -> Self;
}

// ─── Option<T> ───────────────────────────────────────────────────────────────

impl<T> Combine for Option<T> {
    fn combine(self, other: Self) -> Self {
        self.or(other)
    }
}

// ─── Vec<T> ──────────────────────────────────────────────────────────────────

impl<T> Combine for Vec<T> {
    fn combine(mut self, other: Self) -> Self {
        self.extend(other);
        self
    }
}

// ─── Manifest ────────────────────────────────────────────────────────────────

impl Combine for Manifest {
    fn combine(self, other: Self) -> Self {
        Self {
            project: self.project.combine(other.project),
            toolchain: self.toolchain.combine(other.toolchain),
            compiler: self.compiler.combine(other.compiler),
            stackage: self.stackage.combine(other.stackage),
            bhc_platform: self.bhc_platform.combine(other.bhc_platform),
            build: self.build.combine(other.build),
            format: self.format.combine(other.format),
            lint: self.lint.combine(other.lint),
            plugins: self.plugins.combine(other.plugins),
            registries: {
                let mut regs = self.registries;
                for (k, v) in other.registries {
                    regs.entry(k).or_insert(v);
                }
                regs
            },
            dependencies: {
                let mut deps = self.dependencies;
                for (k, v) in other.dependencies {
                    deps.entry(k).or_insert(v);
                }
                deps
            },
        }
    }
}

impl Combine for ProjectConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            // Project name should come from the higher-precedence source
            name: if self.name.is_empty() {
                other.name
            } else {
                self.name
            },
            kind: self.kind, // Use self's kind
            resolver: self.resolver,
        }
    }
}

impl Combine for ToolchainConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            ghc: self.ghc.combine(other.ghc),
            cabal: self.cabal.combine(other.cabal),
            hls: self.hls.combine(other.hls),
        }
    }
}

impl Combine for CompilerConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            backend: self.backend, // Prefer self's backend
            version: self.version.combine(other.version),
            bhc: self.bhc.combine(other.bhc),
            ghc: self.ghc.combine(other.ghc),
        }
    }
}

impl Combine for BhcConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            profile: self.profile, // Prefer self
            emit_kernel_report: self.emit_kernel_report || other.emit_kernel_report,
            target: self.target.combine(other.target),
            tensor_fusion: self.tensor_fusion || other.tensor_fusion,
        }
    }
}

impl Combine for GhcConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            version: self.version.combine(other.version),
            profiling: self.profiling || other.profiling,
            split_sections: self.split_sections || other.split_sections,
        }
    }
}

impl Combine for StackageConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            snapshot: self.snapshot.combine(other.snapshot),
            allow_newer: self.allow_newer || other.allow_newer,
            extra_deps: {
                let mut deps = self.extra_deps;
                for (k, v) in other.extra_deps {
                    deps.entry(k).or_insert(v);
                }
                deps
            },
        }
    }
}

impl Combine for BhcPlatformConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            snapshot: self.snapshot.combine(other.snapshot),
            allow_newer: self.allow_newer || other.allow_newer,
            extra_deps: {
                let mut deps = self.extra_deps;
                for (k, v) in other.extra_deps {
                    deps.entry(k).or_insert(v);
                }
                deps
            },
            overlays: self.overlays.combine(other.overlays),
        }
    }
}

impl Combine for FormatConfig {
    fn combine(self, _other: Self) -> Self {
        Self {
            formatter: self.formatter, // Prefer self
        }
    }
}

impl Combine for LintConfig {
    fn combine(self, _other: Self) -> Self {
        Self {
            hlint: self.hlint, // Prefer self
        }
    }
}

impl Combine for BuildConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            optimization: self.optimization, // Prefer self
            warnings: self.warnings,
            werror: self.werror,
            ghc_flags: self.ghc_flags.combine(other.ghc_flags),
            src_dirs: if self.src_dirs.is_empty() {
                other.src_dirs
            } else {
                self.src_dirs
            },
            native: self.native,
        }
    }
}

impl Combine for PluginConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            enabled: self.enabled, // Prefer self
            hook_timeout_ms: self.hook_timeout_ms,
            paths: self.paths.combine(other.paths),
            continue_on_error: self.continue_on_error,
            hooks: self.hooks.combine(other.hooks),
        }
    }
}

impl Combine for PluginHookConfig {
    fn combine(self, other: Self) -> Self {
        Self {
            pre_build: self.pre_build.combine(other.pre_build),
            post_build: self.post_build.combine(other.post_build),
            pre_test: self.pre_test.combine(other.pre_test),
            post_test: self.post_test.combine(other.post_test),
            pre_run: self.pre_run.combine(other.pre_run),
            post_run: self.post_run.combine(other.post_run),
            pre_clean: self.pre_clean.combine(other.pre_clean),
            post_clean: self.post_clean.combine(other.post_clean),
            pre_lock: self.pre_lock.combine(other.pre_lock),
            post_lock: self.post_lock.combine(other.post_lock),
            init: self.init.combine(other.init),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_option_combine() {
        assert_eq!(Some(1).combine(Some(2)), Some(1));
        assert_eq!(None::<i32>.combine(Some(2)), Some(2));
        assert_eq!(Some(1).combine(None), Some(1));
        assert_eq!(None::<i32>.combine(None), None);
    }

    #[test]
    fn test_vec_combine() {
        let a = vec![1, 2];
        let b = vec![3, 4];
        assert_eq!(a.combine(b), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_toolchain_combine() {
        let a = ToolchainConfig {
            ghc: Some("9.8.2".to_string()),
            cabal: None,
            hls: None,
        };
        let b = ToolchainConfig {
            ghc: Some("9.6.4".to_string()),
            cabal: Some("3.12.1.0".to_string()),
            hls: Some("2.9.0.0".to_string()),
        };
        let combined = a.combine(b);
        assert_eq!(combined.ghc, Some("9.8.2".to_string())); // Self wins
        assert_eq!(combined.cabal, Some("3.12.1.0".to_string())); // From other
        assert_eq!(combined.hls, Some("2.9.0.0".to_string())); // From other
    }
}
