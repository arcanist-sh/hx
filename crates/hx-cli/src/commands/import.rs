//! Import from other build tools (Stack, Cabal).

use anyhow::{Context, Result, bail};
use hx_ui::Output;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::cli::ImportSource;

/// Run the import command.
pub async fn run(source: ImportSource, path: Option<String>, output: &Output) -> Result<i32> {
    match source {
        ImportSource::Stack => import_from_stack(path, output).await,
        ImportSource::Cabal => import_from_cabal(path, output).await,
    }
}

/// Import from stack.yaml.
async fn import_from_stack(path: Option<String>, output: &Output) -> Result<i32> {
    let stack_yaml = path.unwrap_or_else(|| "stack.yaml".to_string());
    let stack_path = Path::new(&stack_yaml);

    if !stack_path.exists() {
        output.error(&format!("{} not found", stack_yaml));
        output.info("Specify path with --path or run from a Stack project directory");
        return Ok(1);
    }

    output.status("Importing", &format!("from {}", stack_yaml));

    let content =
        fs::read_to_string(stack_path).with_context(|| format!("Failed to read {}", stack_yaml))?;

    // Parse stack.yaml (basic YAML parsing)
    let stack_config = parse_stack_yaml(&content)?;

    // Determine the GHC version: an explicit `compiler:` field wins over the
    // resolver-based guess, since the resolver only implies a default GHC.
    let ghc_version = stack_config
        .compiler
        .as_deref()
        .map(compiler_to_ghc)
        .unwrap_or_else(|| resolver_to_ghc(&stack_config.resolver));

    output.list_item("resolver", &stack_config.resolver);
    if let Some(compiler) = &stack_config.compiler {
        output.list_item("compiler", compiler);
    }
    output.list_item("ghc version", &ghc_version);

    // A stack project that lists local packages maps onto an hx workspace.
    // hx discovers workspace members from a cabal.project file, so build one
    // (extra-deps stay pinned at the workspace root in hx.toml).
    let cabal_project = build_cabal_project(&stack_config.packages);
    if cabal_project.is_some() {
        let members: Vec<&String> = stack_config
            .packages
            .iter()
            .filter(|p| p.as_str() != ".")
            .collect();
        output.list_item("workspace packages", &members.len().to_string());
        for pkg in &members {
            output.list_item("package", pkg);
        }
    }

    // Build hx.toml content
    let mut hx_toml = format!(
        r#"[project]
name = "{}"

[toolchain]
ghc = "{}"
"#,
        stack_config.name.as_deref().unwrap_or("project"),
        ghc_version
    );

    // Add dependencies
    if !stack_config.extra_deps.is_empty() {
        hx_toml.push_str("\n[dependencies]\n");
        for dep in &stack_config.extra_deps {
            let (name, version) = parse_dependency(dep);
            hx_toml.push_str(&format!("{} = \"{}\"\n", name, version));
            output.list_item("dependency", dep);
        }
    }

    // Check for existing hx.toml
    let hx_toml_path = Path::new("hx.toml");
    if hx_toml_path.exists() {
        output.warn("hx.toml already exists");
        output.info("Generated configuration:");
        println!("\n{}", hx_toml);
        output.info("Manually merge the above into your existing hx.toml");
        return Ok(0);
    }

    // Write hx.toml
    fs::write(hx_toml_path, &hx_toml).context("Failed to write hx.toml")?;

    output.status("Created", "hx.toml");

    // Write cabal.project so hx recognizes the workspace members.
    if let Some(content) = cabal_project {
        let cabal_project_path = Path::new("cabal.project");
        if cabal_project_path.exists() {
            output.warn("cabal.project already exists; leaving it unchanged");
            output.info("Ensure it lists your packages so hx detects the workspace");
        } else {
            fs::write(cabal_project_path, &content).context("Failed to write cabal.project")?;
            output.status("Created", "cabal.project");
            output.info("Imported a multi-package project; each member keeps its own .cabal");
        }
    }

    // Remind about lockfile
    output.info("Run `hx lock` to generate the lockfile");

    Ok(0)
}

/// Build a `cabal.project` body listing workspace members, or `None` when the
/// stack project has no local packages beyond the root (`.`).
///
/// hx detects a workspace from the presence of a cabal.project file and parses
/// its `packages:` directive. The single-line form is emitted deliberately:
/// hx's parser only attaches continuation lines once it has seen a package on
/// the `packages:` line itself.
fn build_cabal_project(packages: &[String]) -> Option<String> {
    if !packages.iter().any(|p| p != ".") {
        return None;
    }
    let paths: Vec<String> = packages.iter().map(|p| normalize_package_path(p)).collect();
    Some(format!("packages: {}\n", paths.join(" ")))
}

/// Normalize a stack package directory into a cabal.project path.
/// Bare names are made explicitly relative (`pkg` -> `./pkg`); already-relative
/// or absolute paths are kept as-is.
fn normalize_package_path(path: &str) -> String {
    let path = path.trim().trim_end_matches('/');
    if path == "." || path.starts_with("./") || path.starts_with("../") || path.starts_with('/') {
        path.to_string()
    } else {
        format!("./{path}")
    }
}

/// Import from cabal.project.
async fn import_from_cabal(path: Option<String>, output: &Output) -> Result<i32> {
    let explicit_path = path.is_some();
    let cabal_project = path.unwrap_or_else(|| "cabal.project".to_string());
    let cabal_path = Path::new(&cabal_project);

    // A `cabal.project` carries workspace members, a compiler pin, and
    // constraints. Most single-package libraries ship only a `.cabal` file with
    // no `cabal.project` — that is still a project we can adopt. In that case we
    // take the package name from the `.cabal` and leave the rest at defaults.
    let cabal_config = if cabal_path.exists() {
        output.status("Importing", &format!("from {}", cabal_project));
        let content = fs::read_to_string(cabal_path)
            .with_context(|| format!("Failed to read {}", cabal_project))?;
        parse_cabal_project(&content)
    } else if explicit_path {
        // The user named a file that does not exist — that is a real error.
        output.error(&format!("{} not found", cabal_project));
        return Ok(1);
    } else if find_cabal_package_name()?.is_some() {
        output.status("Importing", "from .cabal (single-package project)");
        CabalProjectConfig::default()
    } else {
        output.error("no cabal.project or .cabal file found in this directory");
        output.info("Run `hx import --from cabal` from a directory containing a Cabal project");
        return Ok(1);
    };

    // Find the package name from .cabal file
    let package_name = find_cabal_package_name()?;

    // Build hx.toml
    let mut hx_toml = format!(
        r#"[project]
name = "{}"
"#,
        package_name.as_deref().unwrap_or("project")
    );

    // Add GHC version if specified
    if let Some(ghc) = &cabal_config.with_compiler {
        let version = ghc.strip_prefix("ghc-").unwrap_or(ghc);
        hx_toml.push_str(&format!(
            r#"
[toolchain]
ghc = "{}"
"#,
            version
        ));
        output.list_item("ghc", version);
    }

    // Add constraints as dependencies
    if !cabal_config.constraints.is_empty() {
        hx_toml.push_str("\n[dependencies]\n");
        for constraint in &cabal_config.constraints {
            let (name, version) = parse_constraint(constraint);
            if !version.is_empty() {
                hx_toml.push_str(&format!("{} = \"{}\"\n", name, version));
                output.list_item("constraint", constraint);
            }
        }
    }

    // Check for existing hx.toml
    let hx_toml_path = Path::new("hx.toml");
    if hx_toml_path.exists() {
        output.warn("hx.toml already exists");
        output.info("Generated configuration:");
        println!("\n{}", hx_toml);
        return Ok(0);
    }

    fs::write(hx_toml_path, &hx_toml).context("Failed to write hx.toml")?;

    output.status("Created", "hx.toml");
    output.info("Run `hx lock` to generate the lockfile");

    Ok(0)
}

/// Basic stack.yaml configuration.
#[derive(Debug, Default)]
struct StackConfig {
    name: Option<String>,
    resolver: String,
    /// Explicit `compiler:` field (e.g. `ghc-9.14`), when present.
    compiler: Option<String>,
    extra_deps: Vec<String>,
    packages: Vec<String>,
}

/// Parse stack.yaml content.
fn parse_stack_yaml(content: &str) -> Result<StackConfig> {
    let mut config = StackConfig::default();
    let mut in_extra_deps = false;
    let mut in_packages = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        // Check for section starts
        if trimmed.starts_with("extra-deps:") {
            in_extra_deps = true;
            in_packages = false;
            continue;
        }

        if trimmed.starts_with("packages:") {
            in_packages = true;
            in_extra_deps = false;
            continue;
        }

        // Parse resolver
        if let Some(resolver) = trimmed.strip_prefix("resolver:") {
            config.resolver = strip_yaml_comment(resolver.trim()).to_string();
            in_extra_deps = false;
            in_packages = false;
            continue;
        }

        // Parse explicit compiler (takes precedence over the resolver guess)
        if let Some(compiler) = trimmed.strip_prefix("compiler:") {
            let compiler = strip_yaml_comment(compiler.trim());
            if !compiler.is_empty() {
                config.compiler = Some(compiler.to_string());
            }
            in_extra_deps = false;
            in_packages = false;
            continue;
        }

        // Parse list items
        if trimmed.starts_with("- ") {
            let value = strip_yaml_comment(trimmed.strip_prefix("- ").unwrap().trim()).to_string();
            if in_extra_deps {
                config.extra_deps.push(value);
            } else if in_packages {
                config.packages.push(value);
            }
            continue;
        }

        // Other top-level keys end current section
        if !trimmed.starts_with(' ') && !trimmed.starts_with('\t') && trimmed.contains(':') {
            in_extra_deps = false;
            in_packages = false;
        }
    }

    if config.resolver.is_empty() {
        bail!("No resolver found in stack.yaml");
    }

    Ok(config)
}

/// Strip a trailing YAML comment from a scalar value.
///
/// A `#` only begins a comment when it is at the start of the value or
/// preceded by whitespace, matching YAML's flow-scalar rules. This keeps
/// `#` characters that are legitimately part of a value intact while
/// dropping trailing `  # ...` annotations.
fn strip_yaml_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return value[..i].trim_end();
        }
    }
    value.trim_end()
}

/// Convert a Stack `compiler:` value (e.g. `ghc-9.14`) to a bare GHC version.
fn compiler_to_ghc(compiler: &str) -> String {
    compiler
        .strip_prefix("ghc-")
        .unwrap_or(compiler)
        .to_string()
}

/// Convert Stack resolver to GHC version.
fn resolver_to_ghc(resolver: &str) -> String {
    // LTS resolvers map to GHC versions
    // This is a simplified mapping; a real implementation would use a lookup table
    let resolver_ghc_map: HashMap<&str, &str> = [
        ("lts-22", "9.6.4"),
        ("lts-21", "9.4.8"),
        ("lts-20", "9.2.8"),
        ("lts-19", "9.0.2"),
        ("lts-18", "8.10.7"),
        ("nightly", "9.8.2"),
    ]
    .into_iter()
    .collect();

    // Try exact match first
    if let Some(ghc) = resolver_ghc_map.get(resolver) {
        return ghc.to_string();
    }

    // Try prefix match (e.g., lts-22.7 -> lts-22)
    for (prefix, ghc) in &resolver_ghc_map {
        if resolver.starts_with(prefix) {
            return ghc.to_string();
        }
    }

    // Check for ghc- prefix in resolver
    if let Some(version) = resolver.strip_prefix("ghc-") {
        return version.to_string();
    }

    // Default to recent GHC
    "9.6.4".to_string()
}

/// Parse a dependency string like "package-1.2.3" or "package-1.2.3@sha256:..."
fn parse_dependency(dep: &str) -> (String, String) {
    // Remove hash suffix if present
    let dep = dep.split('@').next().unwrap_or(dep);

    // Find the last dash followed by a digit (version separator)
    let mut last_dash = None;
    for (i, c) in dep.char_indices() {
        if c == '-' && dep[i + 1..].starts_with(|c: char| c.is_ascii_digit()) {
            last_dash = Some(i);
        }
    }

    if let Some(pos) = last_dash {
        (dep[..pos].to_string(), dep[pos + 1..].to_string())
    } else {
        (dep.to_string(), "*".to_string())
    }
}

/// Basic cabal.project configuration.
#[derive(Debug, Default)]
struct CabalProjectConfig {
    with_compiler: Option<String>,
    constraints: Vec<String>,
    packages: Vec<String>,
}

/// Parse cabal.project content.
fn parse_cabal_project(content: &str) -> CabalProjectConfig {
    let mut config = CabalProjectConfig::default();

    for line in content.lines() {
        let trimmed = line.trim();

        if let Some(compiler) = trimmed.strip_prefix("with-compiler:") {
            config.with_compiler = Some(compiler.trim().to_string());
        }

        if let Some(constraint) = trimmed.strip_prefix("constraints:") {
            for c in constraint.split(',') {
                let c = c.trim();
                if !c.is_empty() {
                    config.constraints.push(c.to_string());
                }
            }
        }

        if let Some(packages) = trimmed.strip_prefix("packages:") {
            for p in packages.split_whitespace() {
                config.packages.push(p.to_string());
            }
        }
    }

    config
}

/// Parse a constraint like "package ==1.2.3" or "package >= 1.0"
fn parse_constraint(constraint: &str) -> (String, String) {
    let parts: Vec<&str> = constraint.split_whitespace().collect();
    if parts.len() >= 2 {
        let name = parts[0];
        let version = parts[1..].join(" ");
        (name.to_string(), version)
    } else {
        (constraint.to_string(), String::new())
    }
}

/// Find package name from .cabal file in current directory.
fn find_cabal_package_name() -> Result<Option<String>> {
    let entries = fs::read_dir(".")?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "cabal").unwrap_or(false) {
            let content = fs::read_to_string(&path)?;
            for line in content.lines() {
                if let Some(name) = line.strip_prefix("name:") {
                    return Ok(Some(name.trim().to_string()));
                }
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_yaml_comment() {
        assert_eq!(
            strip_yaml_comment("yesod-static-1.6.1.0  # <1.6.1.1"),
            "yesod-static-1.6.1.0"
        );
        assert_eq!(strip_yaml_comment("# whole line"), "");
        assert_eq!(strip_yaml_comment("ghc-9.14"), "ghc-9.14");
        // A '#' not preceded by whitespace is part of the value, not a comment.
        assert_eq!(strip_yaml_comment("pkg-1.0#frag"), "pkg-1.0#frag");
        assert_eq!(
            strip_yaml_comment("nightly-2026-03-23"),
            "nightly-2026-03-23"
        );
    }

    #[test]
    fn test_parse_stack_yaml_strips_inline_dependency_comments() {
        // Regression for the misparsed dependency in issue #2: the trailing
        // comment must not leak into the version string.
        let yaml = "\
resolver: nightly-2026-03-23
extra-deps:
- haskeline-0.8.4.1
- yesod-static-1.6.1.0  # <1.6.1.1 to avoid https://github.com/psibi/crypton-conduit/issues/3
- yesod-test-1.6.19
";
        let config = parse_stack_yaml(yaml).unwrap();
        assert_eq!(
            config.extra_deps,
            vec![
                "haskeline-0.8.4.1",
                "yesod-static-1.6.1.0",
                "yesod-test-1.6.19"
            ]
        );
        let (name, version) = parse_dependency(&config.extra_deps[1]);
        assert_eq!(name, "yesod-static");
        assert_eq!(version, "1.6.1.0");
    }

    #[test]
    fn test_parse_stack_yaml_reads_explicit_compiler() {
        // Regression for the wrong GHC version in issue #2: an explicit
        // `compiler:` must be captured and must win over the resolver guess.
        let yaml = "\
resolver: nightly-2026-03-23
compiler: ghc-9.14
";
        let config = parse_stack_yaml(yaml).unwrap();
        assert_eq!(config.compiler.as_deref(), Some("ghc-9.14"));
        assert_eq!(compiler_to_ghc(config.compiler.as_ref().unwrap()), "9.14");
        // The resolver alone would have produced a different (default) version.
        assert_ne!(
            compiler_to_ghc("ghc-9.14"),
            resolver_to_ghc(&config.resolver)
        );
    }

    #[test]
    fn test_parse_stack_yaml_captures_local_packages() {
        let yaml = "\
resolver: lts-22.7
packages:
- hledger-lib
- hledger
- hledger-ui
- hledger-web
";
        let config = parse_stack_yaml(yaml).unwrap();
        assert_eq!(
            config.packages,
            vec!["hledger-lib", "hledger", "hledger-ui", "hledger-web"]
        );
        // The resolver still maps to its GHC default when no compiler is set.
        assert_eq!(resolver_to_ghc(&config.resolver), "9.6.4");
    }

    #[test]
    fn test_normalize_package_path() {
        assert_eq!(normalize_package_path("hledger-lib"), "./hledger-lib");
        assert_eq!(normalize_package_path("."), ".");
        assert_eq!(normalize_package_path("./pkg"), "./pkg");
        assert_eq!(normalize_package_path("../sibling"), "../sibling");
        assert_eq!(normalize_package_path("/abs/pkg"), "/abs/pkg");
        assert_eq!(normalize_package_path("pkg/"), "./pkg");
    }

    #[test]
    fn test_build_cabal_project_multi_package() {
        // A multi-package stack project becomes an hx workspace: a single-line
        // cabal.project listing every member (relative-path normalized).
        let packages = vec![
            "hledger-lib".to_string(),
            "hledger".to_string(),
            "hledger-ui".to_string(),
            "hledger-web".to_string(),
        ];
        assert_eq!(
            build_cabal_project(&packages).as_deref(),
            Some("packages: ./hledger-lib ./hledger ./hledger-ui ./hledger-web\n")
        );
    }

    #[test]
    fn test_build_cabal_project_single_package_is_none() {
        // A lone root package (or none at all) is not a workspace.
        assert_eq!(build_cabal_project(&[".".to_string()]), None);
        assert_eq!(build_cabal_project(&[]), None);
    }
}
