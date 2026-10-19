//! Single-file Haskell script execution.

use anyhow::{Context, Result};
use hx_core::CommandRunner;
use hx_core::hash::{Digest, Sha256, to_hex};
use hx_ui::{Output, Spinner};
use std::fs;
use std::path::{Path, PathBuf};

/// Run a single-file Haskell script.
pub async fn run(file: &str, args: Vec<String>, output: &Output) -> Result<i32> {
    let script_path = Path::new(file);

    if !script_path.exists() {
        output.error(&format!("Script not found: {}", file));
        return Ok(1);
    }

    // Read and parse the script
    let script_content =
        fs::read_to_string(script_path).with_context(|| format!("Failed to read {}", file))?;

    // Parse script header for dependencies
    let script_info = parse_script_header(&script_content);

    output.status("Running", file);

    // Compute cache key
    let cache_key = compute_script_hash(&script_content, &script_info.dependencies);
    let cache_dir = get_script_cache_dir()?;
    let cached_exe = cache_dir.join(&cache_key);

    // Check if we have a cached executable
    if cached_exe.exists() {
        output.verbose("Using cached executable");
        return run_executable(&cached_exe, &args, output).await;
    }

    // Need to compile
    let spinner = Spinner::new("Compiling script...");

    // Create a temporary build directory
    let build_dir = cache_dir.join(format!("{}-build", cache_key));
    fs::create_dir_all(&build_dir)?;

    // Copy script to build directory
    let script_name = script_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("script");
    let build_script = build_dir.join(format!("{}.hs", script_name));
    fs::copy(script_path, &build_script)?;

    // Build GHC command
    let runner = CommandRunner::new().with_working_dir(&build_dir);

    let mut ghc_args = vec![
        "-O".to_string(),
        "-o".to_string(),
        cached_exe.to_string_lossy().to_string(),
        build_script.to_string_lossy().to_string(),
    ];

    // Add package dependencies
    for dep in &script_info.dependencies {
        ghc_args.push("-package".to_string());
        ghc_args.push(dep.clone());
    }

    // Add language extensions
    for ext in &script_info.extensions {
        ghc_args.push(format!("-X{}", ext));
    }

    let compile_result = runner
        .run("ghc", ghc_args.iter().map(|s| s.as_str()))
        .await?;

    // Clean up build directory
    let _ = fs::remove_dir_all(&build_dir);

    if !compile_result.success() {
        spinner.finish_error("Compilation failed");
        eprintln!("{}", compile_result.stderr);
        return Ok(5);
    }

    spinner.finish_success("Compiled");

    // Run the executable
    run_executable(&cached_exe, &args, output).await
}

/// Script metadata parsed from header comments.
#[derive(Debug, Default)]
struct ScriptInfo {
    dependencies: Vec<String>,
    extensions: Vec<String>,
}

/// Parse script header for dependencies and extensions.
///
/// Supports formats:
/// -- hx: dep1, dep2, dep3
/// -- depends: dep1, dep2
/// -- language: Extension1, Extension2
/// {- cabal:
/// build-depends: base, text, aeson
/// -}
fn parse_script_header(content: &str) -> ScriptInfo {
    let mut info = ScriptInfo::default();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip shebang
        if trimmed.starts_with("#!") {
            continue;
        }

        // Stop at first non-comment, non-empty line
        if !trimmed.is_empty() && !trimmed.starts_with("--") && !trimmed.starts_with("{-") {
            break;
        }

        // Parse -- hx: dep1, dep2
        if let Some(deps) = trimmed.strip_prefix("-- hx:") {
            for dep in deps.split(',') {
                let dep = dep.trim();
                if !dep.is_empty() {
                    info.dependencies.push(dep.to_string());
                }
            }
        }

        // Parse -- depends: dep1, dep2
        if let Some(deps) = trimmed.strip_prefix("-- depends:") {
            for dep in deps.split(',') {
                let dep = dep.trim();
                if !dep.is_empty() {
                    info.dependencies.push(dep.to_string());
                }
            }
        }

        // Parse -- language: Ext1, Ext2
        if let Some(exts) = trimmed.strip_prefix("-- language:") {
            for ext in exts.split(',') {
                let ext = ext.trim();
                if !ext.is_empty() {
                    info.extensions.push(ext.to_string());
                }
            }
        }

        // Parse {- cabal: build-depends: ... -}
        if trimmed.contains("build-depends:")
            && let Some(deps_part) = trimmed.split("build-depends:").nth(1)
        {
            let deps_str = deps_part.trim_end_matches("-}").trim();
            for dep in deps_str.split(',') {
                let dep = dep.split_whitespace().next().unwrap_or("").trim();
                if !dep.is_empty() && dep != "base" {
                    info.dependencies.push(dep.to_string());
                }
            }
        }
    }

    // Always include base
    if !info.dependencies.contains(&"base".to_string()) {
        info.dependencies.insert(0, "base".to_string());
    }

    info
}

/// Compute a hash for the script and its dependencies.
fn compute_script_hash(content: &str, deps: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    for dep in deps {
        hasher.update(dep.as_bytes());
    }
    let result = hasher.finalize();
    to_hex(result)[..16].to_string()
}

/// Get the script cache directory (see [`hx_cache::script_cache_dir`]).
fn get_script_cache_dir() -> Result<PathBuf> {
    let cache_dir = hx_cache::script_cache_dir()?;

    fs::create_dir_all(&cache_dir)?;
    Ok(cache_dir)
}

/// Run a compiled executable.
async fn run_executable(exe: &Path, args: &[String], output: &Output) -> Result<i32> {
    output.verbose(&format!("Executing: {} {}", exe.display(), args.join(" ")));

    let status = std::process::Command::new(exe)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute {}", exe.display()))?;

    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod hash_golden_tests {
    //! Script hashes name cached script environments. Pinned to exact output so a hashing-library upgrade cannot
    //! silently change them.
    use super::*;

    #[test]
    fn script_hash_is_stable() {
        let deps = vec!["text".to_string(), "aeson >= 2".to_string()];
        assert_eq!(
            compute_script_hash("main = putStrLn \"hi\"\n", &deps),
            "8c6ca178d9879ca2"
        );
    }
}
