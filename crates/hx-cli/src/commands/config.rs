//! Global configuration management commands.

use anyhow::Result;
use hx_cache::global_config_file;
use hx_config::{GlobalConfig, global_config_path, load_global_config};
use hx_ui::Output;
use std::process::Command;

/// Show current global configuration.
pub async fn show(output: &Output) -> Result<i32> {
    match load_global_config() {
        Ok(Some(config)) => {
            let toml = config.to_string().map_err(|e| anyhow::anyhow!("{}", e))?;
            output.info("Global configuration:");
            output.info("");
            println!("{}", toml);
            Ok(0)
        }
        Ok(None) => {
            output.info("No global configuration file found.");
            if let Some(path) = global_config_path() {
                output.info(&format!("Create one at: {}", path.display()));
            }
            output.info("");
            output.info("Run `hx config init` to create a default configuration.");
            Ok(0)
        }
        Err(e) => {
            output.error(&format!("Failed to load global config: {}", e));
            Ok(1)
        }
    }
}

/// Show path to global config file, or with `all`, every hx location.
pub async fn path(all: bool, output: &Output) -> Result<i32> {
    if all {
        return path_all(output);
    }
    match global_config_path() {
        Some(path) => {
            println!("{}", path.display());
            Ok(0)
        }
        None => {
            output.error("Could not determine global config path");
            Ok(1)
        }
    }
}

/// Print every location hx uses, one per line: label, path, and what chose it.
fn path_all(output: &Output) -> Result<i32> {
    let paths = match hx_cache::HxPaths::current() {
        Ok(paths) => paths,
        Err(e) => {
            output.error(&format!("Could not determine hx paths: {}", e));
            return Ok(1);
        }
    };
    print!("{}", format_paths(&paths));
    Ok(0)
}

fn format_paths(paths: &hx_cache::HxPaths) -> String {
    let rows = paths.entries();
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    rows.iter()
        .map(|(label, loc)| {
            format!(
                "{label:<width$}  {}  ({})\n",
                loc.path.display(),
                loc.source
            )
        })
        .collect()
}

/// Open global config in editor.
pub async fn edit(output: &Output) -> Result<i32> {
    let config_file = match global_config_file() {
        Ok(path) => path,
        Err(e) => {
            output.error(&format!("Could not determine config path: {}", e));
            return Ok(1);
        }
    };

    // Create config file with defaults if it doesn't exist
    if !config_file.exists() {
        output.info("Creating default global configuration...");
        let config = GlobalConfig::default();
        config
            .to_file(&config_file)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
    }

    // Get editor from EDITOR or VISUAL, fallback to common editors
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });

    output.info(&format!(
        "Opening {} in {}...",
        config_file.display(),
        editor
    ));

    let status = Command::new(&editor).arg(&config_file).status()?;

    if status.success() {
        Ok(0)
    } else {
        output.error("Editor exited with error");
        Ok(1)
    }
}

/// Set a configuration value.
pub async fn set(key: &str, value: &str, output: &Output) -> Result<i32> {
    let config_file = match global_config_file() {
        Ok(path) => path,
        Err(e) => {
            output.error(&format!("Could not determine config path: {}", e));
            return Ok(1);
        }
    };

    // Load existing config or create default
    let mut config = match load_global_config() {
        Ok(Some(c)) => c,
        Ok(None) => GlobalConfig::default(),
        Err(e) => {
            output.error(&format!("Failed to load config: {}", e));
            return Ok(1);
        }
    };

    // Parse the key and set the value
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["toolchain", "ghc"] => {
            config.toolchain.ghc = Some(value.to_string());
        }
        ["toolchain", "cabal"] => {
            config.toolchain.cabal = Some(value.to_string());
        }
        ["toolchain", "hls"] => {
            config.toolchain.hls = Some(value.to_string());
        }
        ["build", "optimization"] => {
            config.build.optimization = value.parse().map_err(|_| {
                anyhow::anyhow!("Invalid optimization level: {} (must be 0, 1, or 2)", value)
            })?;
        }
        ["build", "warnings"] => {
            config.build.warnings = parse_bool(value)?;
        }
        ["build", "werror"] => {
            config.build.werror = parse_bool(value)?;
        }
        ["build", "native"] => {
            config.build.native = parse_bool(value)?;
        }
        ["format", "formatter"] => {
            if value != "fourmolu" && value != "ormolu" {
                return Err(anyhow::anyhow!(
                    "Invalid formatter: {} (must be 'fourmolu' or 'ormolu')",
                    value
                ));
            }
            config.format.formatter = value.to_string();
        }
        ["lint", "hlint"] => {
            config.lint.hlint = parse_bool(value)?;
        }
        ["plugins", "enabled"] => {
            config.plugins.enabled = parse_bool(value)?;
        }
        ["plugins", "hook_timeout_ms"] => {
            config.plugins.hook_timeout_ms = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid timeout: {} (must be a number)", value))?;
        }
        _ => {
            output.error(&format!("Unknown configuration key: {}", key));
            output.info("");
            output.info("Available keys:");
            output.info("  toolchain.ghc         - Default GHC version");
            output.info("  toolchain.cabal       - Default Cabal version");
            output.info("  toolchain.hls         - Default HLS version");
            output.info("  build.optimization    - Optimization level (0, 1, 2)");
            output.info("  build.warnings        - Enable warnings (true/false)");
            output.info("  build.werror          - Treat warnings as errors (true/false)");
            output.info("  build.native          - Use native builds (true/false)");
            output.info("  format.formatter      - Formatter (fourmolu/ormolu)");
            output.info("  lint.hlint            - Enable hlint (true/false)");
            output.info("  plugins.enabled       - Enable plugins (true/false)");
            output.info("  plugins.hook_timeout_ms - Hook timeout in ms");
            return Ok(1);
        }
    }

    // Save config
    config
        .to_file(&config_file)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    output.status("Set", &format!("{} = {}", key, value));
    Ok(0)
}

/// Get a configuration value.
pub async fn get(key: &str, output: &Output) -> Result<i32> {
    let config = match load_global_config() {
        Ok(Some(c)) => c,
        Ok(None) => {
            output.info("No global configuration file found.");
            return Ok(1);
        }
        Err(e) => {
            output.error(&format!("Failed to load config: {}", e));
            return Ok(1);
        }
    };

    let parts: Vec<&str> = key.split('.').collect();
    let value: Option<String> = match parts.as_slice() {
        ["toolchain", "ghc"] => config.toolchain.ghc,
        ["toolchain", "cabal"] => config.toolchain.cabal,
        ["toolchain", "hls"] => config.toolchain.hls,
        ["build", "optimization"] => Some(config.build.optimization.to_string()),
        ["build", "warnings"] => Some(config.build.warnings.to_string()),
        ["build", "werror"] => Some(config.build.werror.to_string()),
        ["build", "native"] => Some(config.build.native.to_string()),
        ["format", "formatter"] => Some(config.format.formatter),
        ["lint", "hlint"] => Some(config.lint.hlint.to_string()),
        ["plugins", "enabled"] => Some(config.plugins.enabled.to_string()),
        ["plugins", "hook_timeout_ms"] => Some(config.plugins.hook_timeout_ms.to_string()),
        _ => {
            output.error(&format!("Unknown configuration key: {}", key));
            return Ok(1);
        }
    };

    match value {
        Some(v) => {
            println!("{}", v);
            Ok(0)
        }
        None => {
            output.info("(not set)");
            Ok(0)
        }
    }
}

/// Initialize global config file with defaults.
pub async fn init(force: bool, output: &Output) -> Result<i32> {
    let config_file = match global_config_file() {
        Ok(path) => path,
        Err(e) => {
            output.error(&format!("Could not determine config path: {}", e));
            return Ok(1);
        }
    };

    if config_file.exists() && !force {
        output.warn(&format!(
            "Config file already exists: {}",
            config_file.display()
        ));
        output.info("Use --force to overwrite.");
        return Ok(1);
    }

    // Generate TOML with comments (we use a hardcoded template
    // to include helpful comments that toml serialization wouldn't preserve)
    let toml_content = r#"# hx global configuration
# This file provides defaults for all hx projects.
# Project-local hx.toml settings take precedence.

[toolchain]
# ghc = "9.10.3"
# cabal = "3.16.1.0"
# hls = "2.14.0.0"

[build]
optimization = 1     # 0 = none, 1 = default, 2 = aggressive
warnings = true      # Enable -Wall
werror = false       # Treat warnings as errors
# native = false     # Use native GHC builds (experimental)

[format]
formatter = "fourmolu"  # or "ormolu"

[lint]
hlint = true

[plugins]
enabled = true
hook_timeout_ms = 5000
"#;

    // Ensure parent directory exists
    if let Some(parent) = config_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&config_file, toml_content)?;

    output.status("Created", &config_file.display().to_string());
    output.info("");
    output.info("Edit with: hx config edit");
    output.info("View with: hx config show");

    Ok(0)
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(anyhow::anyhow!(
            "Invalid boolean value: {} (use true/false)",
            value
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_cache::{BaseLocations, HxPaths};
    use std::ffi::OsString;

    // Asserts on rendered Unix-style paths; the resolver itself is tested on
    // Linux, macOS and Windows layouts in hx-cache.
    #[cfg(unix)]
    #[test]
    fn format_paths_lists_every_location_with_its_source() {
        let base = BaseLocations {
            home: "/home/u".into(),
            project_cache: "/home/u/.cache/hx".into(),
            project_config: "/home/u/.config/hx".into(),
            platform_cache: "/home/u/.cache".into(),
            platform_config: "/home/u/.config".into(),
        };
        let env = |key: &str| (key == "XDG_CACHE_HOME").then(|| OsString::from("/x"));
        let text = format_paths(&HxPaths::resolve(env, &base));

        assert!(text.contains("/home/u/.config/hx/config.toml  (default)"));
        assert!(text.contains("/x/hx/solver  ($XDG_CACHE_HOME)"));
        assert!(text.contains("/home/u/.hx/toolchains  (default)"));
        assert_eq!(
            text.lines().count(),
            HxPaths::resolve(env, &base).entries().len()
        );
    }
}
