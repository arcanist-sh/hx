//! Test-suite discovery for BHC test runs.
//!
//! BHC has no `test` subcommand, so `hx test` drives tests itself: it reads the
//! package's `test-suite` stanzas, compiles each suite's `main-is` to a native
//! executable with BHC, and runs it. A suite passes when its executable exits 0,
//! which is exactly cabal's `exitcode-stdio-1.0` contract.

use hx_solver::{Dependency, PackageBuildInfo, TestSuiteConfig, parse_cabal_full};
use std::path::{Path, PathBuf};

/// The only test-suite interface BHC test runs support.
pub const EXITCODE_STDIO: &str = "exitcode-stdio-1.0";

/// Conventional test entry points, used when a package declares no test suite.
const CONVENTIONAL_ENTRIES: [&str; 4] = [
    "test/Main.hs",
    "test/Spec.hs",
    "tests/Main.hs",
    "tests/Spec.hs",
];

/// The test framework a suite uses, inferred from its `build-depends`.
///
/// It only decides how `hx test --pattern` is spelled on the suite's command
/// line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestFramework {
    /// hspec (`--match PATTERN`).
    Hspec,
    /// tasty (`--pattern PATTERN`).
    Tasty,
    /// Anything else, including plain `base` mains.
    Unknown,
}

impl TestFramework {
    /// Infer the framework from a suite's dependencies.
    pub fn from_deps(deps: &[Dependency]) -> Self {
        let has = |name: &str| deps.iter().any(|d| d.name == name);
        if has("hspec") {
            Self::Hspec
        } else if has("tasty") {
            Self::Tasty
        } else {
            Self::Unknown
        }
    }

    /// The suite arguments that select tests matching `pattern`.
    ///
    /// hspec and tasty get their own filter flag (the cabal path forwards the
    /// same flags through `--test-option`). For an unknown framework the
    /// pattern is passed through verbatim as the only argument, leaving its
    /// interpretation to the test program.
    pub fn pattern_args(self, pattern: &str) -> Vec<String> {
        match self {
            Self::Hspec => vec!["--match".to_string(), pattern.to_string()],
            Self::Tasty => vec!["--pattern".to_string(), pattern.to_string()],
            Self::Unknown => vec![pattern.to_string()],
        }
    }
}

/// A test suite BHC can build and run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BhcTestSuite {
    /// Suite name (the cabal stanza name).
    pub name: String,
    /// The suite's main source file, relative to the package root.
    pub main_file: PathBuf,
    /// Module search paths, relative to the package root: the suite's
    /// `hs-source-dirs` plus the package library's when the suite depends on it.
    pub import_dirs: Vec<PathBuf>,
    /// Language extensions from `default-extensions`.
    pub extensions: Vec<String>,
    /// The suite's test framework.
    pub framework: TestFramework,
}

/// A test-suite stanza that was found but cannot be run with BHC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSuite {
    /// Suite name.
    pub name: String,
    /// Why it was skipped.
    pub reason: String,
}

/// The result of scanning a package for test suites.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestSuiteDiscovery {
    /// Runnable suites, in declaration order.
    pub suites: Vec<BhcTestSuite>,
    /// Stanzas that were found but cannot be run.
    pub skipped: Vec<SkippedSuite>,
}

/// Discover the test suites of the package rooted at `package_root`.
///
/// The `.cabal` file's `test-suite` stanzas are authoritative. When there is no
/// `.cabal` file, or it declares no test suite, a conventional entry point
/// (`test/Main.hs`, `test/Spec.hs`, `tests/…`) is used as a single suite named
/// `<package>-test`, importing from `fallback_src_dirs` and the test directory.
pub fn discover_test_suites(
    package_root: &Path,
    package_name: &str,
    cabal_file: Option<&Path>,
    fallback_src_dirs: &[String],
) -> std::io::Result<TestSuiteDiscovery> {
    let mut discovery = TestSuiteDiscovery::default();

    if let Some(cabal_file) = cabal_file {
        let content = std::fs::read_to_string(cabal_file)?;
        let info = parse_cabal_full(&content);
        discovery = suites_from_cabal(&info, package_root);
        if !discovery.suites.is_empty() || !discovery.skipped.is_empty() {
            return Ok(discovery);
        }
    }

    if let Some(suite) = conventional_suite(package_root, package_name, fallback_src_dirs) {
        discovery.suites.push(suite);
    }
    Ok(discovery)
}

/// Convert parsed cabal `test-suite` stanzas into runnable suites.
pub fn suites_from_cabal(info: &PackageBuildInfo, package_root: &Path) -> TestSuiteDiscovery {
    let mut discovery = TestSuiteDiscovery::default();
    for stanza in &info.test_suites {
        match suite_from_stanza(stanza, info, package_root) {
            Ok(suite) => discovery.suites.push(suite),
            Err(reason) => discovery.skipped.push(SkippedSuite {
                name: stanza.name.clone(),
                reason,
            }),
        }
    }
    discovery
}

fn suite_from_stanza(
    stanza: &TestSuiteConfig,
    info: &PackageBuildInfo,
    package_root: &Path,
) -> Result<BhcTestSuite, String> {
    // Cabal requires `type`; treat a missing one as exitcode-stdio, which is
    // what `main-is` implies.
    let test_type = stanza.test_type.as_deref().unwrap_or(EXITCODE_STDIO);
    if test_type != EXITCODE_STDIO {
        return Err(format!(
            "type `{test_type}` is not supported by BHC test runs (only {EXITCODE_STDIO})"
        ));
    }
    let Some(main_is) = stanza.main_is.as_deref() else {
        return Err("no `main-is` field".to_string());
    };

    let suite_dirs = source_dirs(&stanza.hs_source_dirs);
    // `main-is` is relative to one of the suite's source dirs.
    let main_file = suite_dirs
        .iter()
        .map(|dir| normalize(&dir.join(main_is)))
        .find(|candidate| package_root.join(candidate).is_file())
        .ok_or_else(|| {
            format!(
                "main-is `{main_is}` not found in hs-source-dirs ({})",
                display_dirs(&suite_dirs)
            )
        })?;

    let mut import_dirs = suite_dirs;
    // A suite that depends on its own package imports the library's modules.
    if let Some(lib) = &info.library
        && stanza.build_depends.iter().any(|d| d.name == info.name)
    {
        for dir in source_dirs(&lib.hs_source_dirs) {
            if !import_dirs.contains(&dir) {
                import_dirs.push(dir);
            }
        }
    }

    Ok(BhcTestSuite {
        name: stanza.name.clone(),
        main_file,
        import_dirs,
        extensions: stanza.default_extensions.clone(),
        framework: TestFramework::from_deps(&stanza.build_depends),
    })
}

/// The fallback suite for packages without a `test-suite` stanza.
fn conventional_suite(
    package_root: &Path,
    package_name: &str,
    src_dirs: &[String],
) -> Option<BhcTestSuite> {
    let entry = CONVENTIONAL_ENTRIES
        .iter()
        .map(PathBuf::from)
        .find(|rel| package_root.join(rel).is_file())?;
    let test_dir = entry.parent().map(Path::to_path_buf).unwrap_or_default();

    let mut import_dirs: Vec<PathBuf> = src_dirs.iter().map(PathBuf::from).collect();
    if !import_dirs.contains(&test_dir) {
        import_dirs.push(test_dir);
    }

    Some(BhcTestSuite {
        name: format!("{package_name}-test"),
        main_file: entry,
        import_dirs,
        extensions: Vec::new(),
        framework: TestFramework::Unknown,
    })
}

/// A stanza's `hs-source-dirs`, defaulting to the package root like cabal.
fn source_dirs(dirs: &[String]) -> Vec<PathBuf> {
    if dirs.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        dirs.iter().map(PathBuf::from).collect()
    }
}

/// Drop `.` components so `./Main.hs` displays as `Main.hs`.
fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

fn display_dirs(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CABAL: &str = r#"
cabal-version: 2.4
name:          demo
version:       0.1.0

library
  hs-source-dirs:  src
  exposed-modules: Demo
  build-depends:   base

test-suite demo-test
  type:           exitcode-stdio-1.0
  main-is:        Main.hs
  hs-source-dirs: test
  build-depends:  base, demo
  default-extensions: OverloadedStrings

test-suite spec
  type:           exitcode-stdio-1.0
  main-is:        Spec.hs
  hs-source-dirs: spec
  build-depends:  base, hspec

test-suite docs
  type:        detailed-0.9
  test-module: Docs
"#;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn discovers_exitcode_suites_from_cabal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "demo.cabal", CABAL);
        write(root, "test/Main.hs", "main = pure ()\n");
        write(root, "spec/Spec.hs", "main = pure ()\n");

        let found =
            discover_test_suites(root, "demo", Some(&root.join("demo.cabal")), &[]).unwrap();

        assert_eq!(found.suites.len(), 2);
        let demo = &found.suites[0];
        assert_eq!(demo.name, "demo-test");
        assert_eq!(demo.main_file, PathBuf::from("test/Main.hs"));
        // Depends on its own library, so the library's dirs are importable.
        assert_eq!(
            demo.import_dirs,
            vec![PathBuf::from("test"), PathBuf::from("src")]
        );
        assert_eq!(demo.extensions, vec!["OverloadedStrings"]);
        assert_eq!(demo.framework, TestFramework::Unknown);

        let spec = &found.suites[1];
        assert_eq!(spec.import_dirs, vec![PathBuf::from("spec")]);
        assert_eq!(spec.framework, TestFramework::Hspec);

        assert_eq!(found.skipped.len(), 1);
        assert_eq!(found.skipped[0].name, "docs");
        assert!(found.skipped[0].reason.contains("detailed-0.9"));
    }

    #[test]
    fn missing_main_is_file_is_skipped_with_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "demo.cabal", CABAL);
        write(root, "spec/Spec.hs", "main = pure ()\n");

        let found =
            discover_test_suites(root, "demo", Some(&root.join("demo.cabal")), &[]).unwrap();
        let skipped = found
            .skipped
            .iter()
            .find(|s| s.name == "demo-test")
            .unwrap();
        assert!(skipped.reason.contains("main-is `Main.hs` not found"));
        assert!(skipped.reason.contains("test"));
    }

    #[test]
    fn default_source_dir_is_package_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "p.cabal",
            "name: p\nversion: 1\ntest-suite t\n  type: exitcode-stdio-1.0\n  main-is: T.hs\n",
        );
        write(root, "T.hs", "main = pure ()\n");

        let found = discover_test_suites(root, "p", Some(&root.join("p.cabal")), &[]).unwrap();
        assert_eq!(found.suites[0].main_file, PathBuf::from("T.hs"));
        assert_eq!(found.suites[0].import_dirs, vec![PathBuf::from(".")]);
    }

    #[test]
    fn falls_back_to_conventional_entry_without_stanzas() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "tests/Spec.hs", "main = pure ()\n");

        let found = discover_test_suites(root, "demo", None, &["src".to_string()]).unwrap();
        assert_eq!(found.suites.len(), 1);
        let suite = &found.suites[0];
        assert_eq!(suite.name, "demo-test");
        assert_eq!(suite.main_file, PathBuf::from("tests/Spec.hs"));
        assert_eq!(
            suite.import_dirs,
            vec![PathBuf::from("src"), PathBuf::from("tests")]
        );
    }

    #[test]
    fn conventional_entry_prefers_test_main() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "test/Spec.hs", "main = pure ()\n");
        write(root, "test/Main.hs", "main = pure ()\n");

        let found = discover_test_suites(root, "demo", None, &[]).unwrap();
        assert_eq!(found.suites[0].main_file, PathBuf::from("test/Main.hs"));
    }

    #[test]
    fn nothing_found_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let found = discover_test_suites(tmp.path(), "demo", None, &[]).unwrap();
        assert_eq!(found, TestSuiteDiscovery::default());
    }

    #[test]
    fn pattern_args_follow_the_framework() {
        assert_eq!(
            TestFramework::Hspec.pattern_args("parser"),
            vec!["--match", "parser"]
        );
        assert_eq!(
            TestFramework::Tasty.pattern_args("parser"),
            vec!["--pattern", "parser"]
        );
        assert_eq!(
            TestFramework::Unknown.pattern_args("parser"),
            vec!["parser"]
        );
    }

    #[test]
    fn framework_is_inferred_from_deps() {
        let deps = |names: &[&str]| -> Vec<Dependency> {
            names.iter().map(|n| Dependency::new(*n)).collect()
        };
        assert_eq!(
            TestFramework::from_deps(&deps(&["base", "hspec"])),
            TestFramework::Hspec
        );
        assert_eq!(
            TestFramework::from_deps(&deps(&["base", "tasty", "tasty-hunit"])),
            TestFramework::Tasty
        );
        assert_eq!(
            TestFramework::from_deps(&deps(&["base", "HUnit"])),
            TestFramework::Unknown
        );
    }
}
