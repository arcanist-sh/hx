//! Plugin management commands.

use anyhow::Result;
use hx_config::{Project, find_project_root};
use hx_plugins::{PluginConfig, PluginManager, discover_plugins};
use hx_ui::Output;
use std::path::PathBuf;

/// List available plugins.
pub async fn list(output: &Output) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Convert config
    let mut config: PluginConfig = project.manifest.plugins.clone().into();
    config.trust_local = hx_config::is_project_trusted(&project_root);

    if !config.enabled {
        output.warn("Plugins are disabled in hx.toml");
        output.info("Set [plugins].enabled = true to enable plugins");
        return Ok(0);
    }

    output.status("Plugins", project.name());

    if !config.trust_local {
        output.warn("Project is not trusted: local plugins (.hx/plugins) are excluded");
        output.info("Run `hx plugins trust` to allow them after reviewing the scripts");
    }

    // Discover available plugins
    let plugins = discover_plugins(&config, &project_root)?;

    if plugins.is_empty() {
        output.info("No plugins found");
        output.info("Plugin search paths:");
        for path in config.all_paths(&project_root) {
            output.info(&format!("  {}", path.display()));
        }
        return Ok(0);
    }

    output.info(&format!("Found {} plugin(s):", plugins.len()));
    for plugin in &plugins {
        output.info(&format!("  {} ({})", plugin.name, plugin.path.display()));
    }

    // Show search paths
    output.verbose("Plugin search paths:");
    for path in config.all_paths(&project_root) {
        let exists = path.exists();
        let status = if exists { "exists" } else { "not found" };
        output.verbose(&format!("  {} ({})", path.display(), status));
    }

    Ok(0)
}

/// Show plugin system status.
pub async fn status(output: &Output) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Convert config
    let mut config: PluginConfig = project.manifest.plugins.clone().into();
    config.trust_local = hx_config::is_project_trusted(&project_root);

    output.status("Plugin Status", project.name());

    output.info(&format!(
        "Local plugins trusted: {}",
        if config.trust_local {
            "yes"
        } else {
            "no (run `hx plugins trust` to allow)"
        }
    ));

    // Enabled status
    if config.enabled {
        output.info("Status: Enabled");
    } else {
        output.info("Status: Disabled");
    }

    // Hook timeout
    output.info(&format!("Hook timeout: {}ms", config.hook_timeout_ms));

    // Continue on error
    output.info(&format!("Continue on error: {}", config.continue_on_error));

    // Configured hooks
    let hooks = &config.hooks;
    let hook_count = count_configured_hooks(hooks);

    if hook_count > 0 {
        output.info(&format!("Configured hooks: {}", hook_count));

        if !hooks.pre_build.is_empty() {
            output.info(&format!("  pre_build: {:?}", hooks.pre_build));
        }
        if !hooks.post_build.is_empty() {
            output.info(&format!("  post_build: {:?}", hooks.post_build));
        }
        if !hooks.pre_test.is_empty() {
            output.info(&format!("  pre_test: {:?}", hooks.pre_test));
        }
        if !hooks.post_test.is_empty() {
            output.info(&format!("  post_test: {:?}", hooks.post_test));
        }
        if !hooks.pre_run.is_empty() {
            output.info(&format!("  pre_run: {:?}", hooks.pre_run));
        }
        if !hooks.post_run.is_empty() {
            output.info(&format!("  post_run: {:?}", hooks.post_run));
        }
        if !hooks.pre_clean.is_empty() {
            output.info(&format!("  pre_clean: {:?}", hooks.pre_clean));
        }
        if !hooks.post_clean.is_empty() {
            output.info(&format!("  post_clean: {:?}", hooks.post_clean));
        }
        if !hooks.pre_lock.is_empty() {
            output.info(&format!("  pre_lock: {:?}", hooks.pre_lock));
        }
        if !hooks.post_lock.is_empty() {
            output.info(&format!("  post_lock: {:?}", hooks.post_lock));
        }
        if !hooks.init.is_empty() {
            output.info(&format!("  init: {:?}", hooks.init));
        }
    } else {
        output.info("No hooks configured");
    }

    // Discover available plugins
    let plugins = discover_plugins(&config, &project_root)?;
    output.info(&format!("Available plugins: {}", plugins.len()));

    Ok(0)
}

/// Run a plugin script directly.
pub async fn run_script(script: String, args: Vec<String>, output: &Output) -> Result<i32> {
    // Find project root
    let project_root = find_project_root(".")?;
    let project = Project::load(&project_root)?;

    // Convert config
    let mut config: PluginConfig = project.manifest.plugins.clone().into();
    config.trust_local = hx_config::is_project_trusted(&project_root);

    if !config.enabled {
        output.error("Plugins are disabled in hx.toml");
        output.info("Set [plugins].enabled = true to enable plugins");
        return Ok(1);
    }

    // Resolve script path
    let script_path = PathBuf::from(&script);
    let script_path = if script_path.is_absolute() {
        script_path
    } else {
        // Try project-local plugins directory first
        let local_path = project_root.join(".hx").join("plugins").join(&script);
        if local_path.exists() {
            local_path
        } else {
            // Try current directory
            std::env::current_dir()?.join(&script)
        }
    };

    if !script_path.exists() {
        output.error(&format!("Script not found: {}", script_path.display()));
        return Ok(1);
    }

    output.status("Running", &script_path.display().to_string());

    // Create and initialize plugin manager
    let mut manager = PluginManager::new(config)?;
    manager.initialize()?;

    // Load the script
    manager.load_plugin(&script_path)?;

    // Check for a main function and run it
    // For now, just loading the script is enough - it will execute top-level code
    output.info("Script executed successfully");

    // If args were provided, try to run a command with them
    if !args.is_empty() {
        let cmd_name = &args[0];
        if manager.has_command(cmd_name) {
            let exit_code = manager.run_command(cmd_name, &args[1..])?;
            return Ok(exit_code);
        }
    }

    Ok(0)
}

/// Trust this project's local plugins.
pub async fn trust(output: &Output) -> Result<i32> {
    let project_root = find_project_root(".")?;
    let local_dir = PluginConfig::local_plugins_dir(&project_root);

    match hx_config::trust_project(&project_root) {
        Ok(true) => {
            output.status("Trusted", &project_root.display().to_string());
            output.info(&format!(
                "Local plugins in {} will now be loaded",
                local_dir.display()
            ));
            Ok(0)
        }
        Ok(false) => {
            output.info("Project is already trusted");
            Ok(0)
        }
        Err(e) => {
            output.error(&format!("Failed to update trust list: {}", e));
            Ok(1)
        }
    }
}

/// Revoke trust for this project's local plugins.
pub async fn untrust(output: &Output) -> Result<i32> {
    let project_root = find_project_root(".")?;

    match hx_config::untrust_project(&project_root) {
        Ok(true) => {
            output.status("Untrusted", &project_root.display().to_string());
            Ok(0)
        }
        Ok(false) => {
            output.info("Project was not trusted");
            Ok(0)
        }
        Err(e) => {
            output.error(&format!("Failed to update trust list: {}", e));
            Ok(1)
        }
    }
}

/// Count the number of configured hooks.
fn count_configured_hooks(hooks: &hx_plugins::HookConfig) -> usize {
    hooks.pre_build.len()
        + hooks.post_build.len()
        + hooks.pre_test.len()
        + hooks.post_test.len()
        + hooks.pre_run.len()
        + hooks.post_run.len()
        + hooks.pre_clean.len()
        + hooks.post_clean.len()
        + hooks.pre_lock.len()
        + hooks.post_lock.len()
        + hooks.init.len()
}
