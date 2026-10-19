//! Project initialization command.

use anyhow::Result;
use hx_config::{BhcProfile, CompilerBackend, MANIFEST_FILENAME, Manifest, ProjectKind};
use hx_ui::{Output, Spinner};
use std::fs;
use std::path::PathBuf;

/// Run the init command.
///
/// Smart defaults:
/// - `hx init` initializes in current directory
/// - `hx init myapp` creates myapp/ and initializes there
/// - Defaults to executable (use --lib for library)
/// - Project name defaults to directory name
pub async fn run(
    path: Option<String>,
    lib: bool,
    name: Option<String>,
    ci: bool,
    backend: Option<CompilerBackend>,
    output: &Output,
) -> Result<i32> {
    let spinner = Spinner::new("Initializing project...");

    // Determine project directory
    let (project_dir, create_dir) = if let Some(p) = path {
        let dir = PathBuf::from(&p);
        // If path is provided, we'll create the directory
        (dir, true)
    } else {
        // No path = current directory
        (std::env::current_dir()?, false)
    };

    // Determine project name (from --name, or from directory name)
    let project_name = name.unwrap_or_else(|| {
        project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("myproject")
            .to_string()
    });

    // Validate project name
    if !is_valid_package_name(&project_name) {
        spinner.finish_error("Invalid project name");
        output.error(&format!(
            "'{}' is not a valid Haskell package name",
            project_name
        ));
        output.info(
            "Package names must start with a letter and contain only letters, numbers, and hyphens",
        );
        return Ok(1);
    }

    // Determine project kind (default to executable)
    let kind = if lib {
        ProjectKind::Lib
    } else {
        ProjectKind::Bin
    };

    // Create project directory if needed
    if create_dir && !project_dir.exists() {
        fs::create_dir_all(&project_dir)?;
    } else if !create_dir && !project_dir.exists() {
        spinner.finish_error("Directory does not exist");
        output.error(&format!(
            "Directory '{}' does not exist",
            project_dir.display()
        ));
        return Ok(1);
    }

    // Check if hx.toml already exists
    let manifest_path = project_dir.join(MANIFEST_FILENAME);
    if manifest_path.exists() {
        spinner.finish_warning("Project already initialized");
        output.warn(&format!("{} already exists", MANIFEST_FILENAME));
        return Ok(1);
    }

    // Create hx.toml
    let mut manifest = Manifest::new(&project_name, kind);

    // Set BHC backend if requested
    if let Some(CompilerBackend::Bhc) = backend {
        manifest.compiler.backend = CompilerBackend::Bhc;
        manifest.compiler.bhc.profile = BhcProfile::Default;
    }

    manifest.to_file(&manifest_path)?;

    // Create .gitignore
    let gitignore_path = project_dir.join(".gitignore");
    if !gitignore_path.exists() {
        fs::write(
            &gitignore_path,
            r#"# hx
.hx/
dist-newstyle/

# Cabal
*.hi
*.o
*.dyn_hi
*.dyn_o
"#,
        )?;
    }

    // Create src directory
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;

    // Create main source file
    match kind {
        ProjectKind::Bin => {
            let main_path = src_dir.join("Main.hs");
            if !main_path.exists() {
                fs::write(
                    &main_path,
                    r#"module Main where

main :: IO ()
main = putStrLn "Hello, Haskell!"
"#,
                )?;
            }
        }
        ProjectKind::Lib => {
            let lib_path = src_dir.join("Lib.hs");
            if !lib_path.exists() {
                fs::write(
                    &lib_path,
                    format!(
                        r#"module Lib
    ( someFunc
    ) where

someFunc :: IO ()
someFunc = putStrLn "{}"
"#,
                        project_name
                    ),
                )?;
            }
        }
    }

    // Create .cabal file
    let cabal_path = project_dir.join(format!("{}.cabal", project_name));
    if !cabal_path.exists() {
        let cabal_content = generate_cabal(&project_name, kind);
        fs::write(&cabal_path, cabal_content)?;
    }

    // Create .editorconfig
    let editorconfig_path = project_dir.join(".editorconfig");
    if !editorconfig_path.exists() {
        fs::write(
            &editorconfig_path,
            r#"root = true

[*]
indent_style = space
indent_size = 2
end_of_line = lf
charset = utf-8
trim_trailing_whitespace = true
insert_final_newline = true

[*.hs]
indent_size = 2

[*.cabal]
indent_size = 2

[Makefile]
indent_style = tab
"#,
        )?;
    }

    // Create GitHub Actions CI workflow if requested
    if ci {
        let workflows_dir = project_dir.join(".github").join("workflows");
        fs::create_dir_all(&workflows_dir)?;
        let ci_path = workflows_dir.join("ci.yml");
        if !ci_path.exists() {
            fs::write(&ci_path, generate_ci_workflow(&project_name, kind))?;
        }
    }

    spinner.finish_success(format!(
        "Created {} project: {}",
        kind.as_str(),
        project_name
    ));

    output.info("");
    output.info("Next steps:");
    output.info(&format!("  cd {}", project_dir.display()));
    output.info("  hx build");
    if kind == ProjectKind::Bin {
        output.info("  hx run");
    }

    Ok(0)
}

fn generate_cabal(name: &str, kind: ProjectKind) -> String {
    match kind {
        ProjectKind::Bin => format!(
            r#"cabal-version:      3.0
name:               {name}
version:            0.1.0.0
synopsis:           A Haskell project
license:            MIT
author:             Author
maintainer:         author@example.com
build-type:         Simple

executable {name}
    main-is:          Main.hs
    hs-source-dirs:   src
    default-language: GHC2021
    build-depends:
        base ^>=4.17 || ^>=4.18 || ^>=4.19 || ^>=4.20 || ^>=4.21 || ^>=4.22
    ghc-options:      -Wall
"#
        ),
        ProjectKind::Lib => format!(
            r#"cabal-version:      3.0
name:               {name}
version:            0.1.0.0
synopsis:           A Haskell library
license:            MIT
author:             Author
maintainer:         author@example.com
build-type:         Simple

library
    exposed-modules:  Lib
    hs-source-dirs:   src
    default-language: GHC2021
    build-depends:
        base ^>=4.17 || ^>=4.18 || ^>=4.19 || ^>=4.20 || ^>=4.21 || ^>=4.22
    ghc-options:      -Wall
"#
        ),
    }
}

fn generate_ci_workflow(_name: &str, kind: ProjectKind) -> String {
    let run_target = match kind {
        ProjectKind::Bin => {
            r#"
      - name: Run executable
        run: cabal run
"#
        }
        ProjectKind::Lib => "",
    };

    format!(
        r#"name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  build:
    name: Build and Test
    runs-on: ${{{{ matrix.os }}}}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest]
        ghc: ['9.8.2', '9.6.6']

    steps:
      - uses: actions/checkout@v4

      - name: Setup Haskell
        uses: haskell-actions/setup@v2
        with:
          ghc-version: ${{{{ matrix.ghc }}}}
          cabal-version: '3.12'

      - name: Cache Cabal
        uses: actions/cache@v4
        with:
          path: |
            ~/.cabal/store
            dist-newstyle
          key: ${{{{ runner.os }}}}-ghc-${{{{ matrix.ghc }}}}-${{{{ hashFiles('**/*.cabal') }}}}
          restore-keys: |
            ${{{{ runner.os }}}}-ghc-${{{{ matrix.ghc }}}}-

      - name: Install dependencies
        run: cabal build all --only-dependencies

      - name: Build
        run: cabal build all

      - name: Run tests
        run: cabal test all
{run_target}
  lint:
    name: Lint and Format
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v4

      - name: Setup Haskell
        uses: haskell-actions/setup@v2
        with:
          ghc-version: '9.8.2'
          cabal-version: '3.12'

      - name: Check formatting with fourmolu
        uses: haskell-actions/run-fourmolu@v10
        with:
          version: "0.16.2.0"

      - name: Lint with hlint
        uses: haskell-actions/hlint-scan@v1
        with:
          path: src/
"#
    )
}

/// Check if a string is a valid Haskell package name.
fn is_valid_package_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let mut chars = name.chars();

    // Must start with a letter
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }

    // Rest must be letters, digits, or hyphens
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return false;
        }
    }

    // Can't end with a hyphen
    if name.ends_with('-') {
        return false;
    }

    true
}
