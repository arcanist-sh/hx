//! Nix integration command implementation.

use anyhow::{Context, Result};
use hx_config::{Project, find_project_root};
use hx_lock::Lockfile;
use hx_ui::Output;
use std::fs;

/// Generate a flake.nix file.
pub async fn flake(output_file: Option<String>, force: bool, output: &Output) -> Result<i32> {
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    let out_path = output_file.unwrap_or_else(|| "flake.nix".to_string());
    let full_path = project_root.join(&out_path);

    if full_path.exists() && !force {
        output.error(&format!(
            "{} already exists (use --force to overwrite)",
            out_path
        ));
        return Ok(1);
    }

    output.status("Generating", &out_path);

    // Load lockfile for dependencies
    let lockfile = Lockfile::from_file(project.lockfile_path()).ok();

    let flake_content = generate_flake_nix(&project, lockfile.as_ref());

    fs::write(&full_path, &flake_content)
        .with_context(|| format!("Failed to write {}", out_path))?;

    output.status("Generated", &out_path);
    output.info("Run `nix develop` to enter the development shell");
    output.info("Run `nix build` to build the project");

    Ok(0)
}

/// Generate a shell.nix file.
pub async fn shell(output_file: Option<String>, force: bool, output: &Output) -> Result<i32> {
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    let out_path = output_file.unwrap_or_else(|| "shell.nix".to_string());
    let full_path = project_root.join(&out_path);

    if full_path.exists() && !force {
        output.error(&format!(
            "{} already exists (use --force to overwrite)",
            out_path
        ));
        return Ok(1);
    }

    output.status("Generating", &out_path);

    let shell_content = generate_shell_nix(&project);

    fs::write(&full_path, &shell_content)
        .with_context(|| format!("Failed to write {}", out_path))?;

    output.status("Generated", &out_path);
    output.info("Run `nix-shell` to enter the development shell");

    Ok(0)
}

/// Generate flake.nix content.
fn generate_flake_nix(project: &Project, lockfile: Option<&Lockfile>) -> String {
    let name = project.name();
    let ghc_version = project
        .manifest
        .toolchain
        .ghc
        .as_deref()
        .unwrap_or(hx_toolchain::RECOMMENDED_GHC_VERSION);

    // Convert GHC version to nixpkgs attribute name
    let ghc_attr = format!("ghc{}", ghc_version.replace('.', ""));

    // Get dependencies from lockfile
    let deps: Vec<String> = lockfile
        .map(|l| {
            l.packages
                .iter()
                .filter(|p| !is_builtin_package(&p.name))
                .map(|p| p.name.clone())
                .collect()
        })
        .unwrap_or_default();

    let deps_list = if deps.is_empty() {
        String::new()
    } else {
        format!(
            "\n            {}",
            deps.iter()
                .map(|d| format!("haskellPackages.{}", d.replace('-', "_")))
                .collect::<Vec<_>>()
                .join("\n            ")
        )
    };

    format!(
        r#"{{
  description = "{name} - Haskell project";

  inputs = {{
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  }};

  outputs = {{ self, nixpkgs, flake-utils }}:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${{system}};
        haskellPackages = pkgs.haskell.packages.{ghc_attr};
      in {{
        packages.default = haskellPackages.callCabal2nix "{name}" ./. {{}};

        devShells.default = pkgs.mkShell {{
          buildInputs = [
            haskellPackages.ghc
            haskellPackages.cabal-install
            haskellPackages.haskell-language-server
            haskellPackages.fourmolu
            haskellPackages.hlint{deps_list}
            pkgs.zlib
            pkgs.pkg-config
          ];

          shellHook = ''
            echo "hx development shell"
            echo "GHC: $(ghc --version)"
            echo "Cabal: $(cabal --version | head -1)"
          '';
        }};
      }});
}}
"#,
        name = name,
        ghc_attr = ghc_attr,
        deps_list = deps_list
    )
}

/// Generate shell.nix content.
fn generate_shell_nix(project: &Project) -> String {
    let ghc_version = project
        .manifest
        .toolchain
        .ghc
        .as_deref()
        .unwrap_or(hx_toolchain::RECOMMENDED_GHC_VERSION);

    let ghc_attr = format!("ghc{}", ghc_version.replace('.', ""));

    format!(
        r#"{{ pkgs ? import <nixpkgs> {{}} }}:

let
  haskellPackages = pkgs.haskell.packages.{ghc_attr};
in
pkgs.mkShell {{
  buildInputs = [
    haskellPackages.ghc
    haskellPackages.cabal-install
    haskellPackages.haskell-language-server
    haskellPackages.fourmolu
    haskellPackages.hlint
    pkgs.zlib
    pkgs.pkg-config
  ];

  shellHook = ''
    echo "hx development shell (shell.nix)"
    echo "GHC: $(ghc --version)"
  '';
}}
"#,
        ghc_attr = ghc_attr
    )
}

/// Check if a package is a built-in GHC package.
fn is_builtin_package(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "ghc-prim"
            | "ghc-bignum"
            | "integer-gmp"
            | "template-haskell"
            | "ghc"
            | "ghc-boot"
            | "ghc-boot-th"
            | "ghc-heap"
            | "ghci"
            | "hpc"
            | "libiserv"
            | "rts"
            | "array"
            | "binary"
            | "bytestring"
            | "containers"
            | "deepseq"
            | "directory"
            | "exceptions"
            | "filepath"
            | "mtl"
            | "parsec"
            | "pretty"
            | "process"
            | "stm"
            | "text"
            | "time"
            | "transformers"
            | "unix"
            | "Win32"
    )
}
