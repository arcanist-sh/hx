//! Clean command implementation.

use anyhow::Result;
use hx_cache::{clean_global_cache, clean_project_cache};
use hx_config::{Project, find_project_root};
use hx_plugins::HookEvent;
use hx_ui::{Output, Spinner};

use crate::plugins::PluginHooks;

/// Run the clean command.
pub async fn run(global: bool, output: &Output) -> Result<i32> {
    if global {
        output.status("Cleaning", "global cache");

        let spinner = Spinner::new("Removing global cache...");

        match clean_global_cache() {
            Ok(()) => {
                spinner.finish_success("Global cache cleaned");
                Ok(0)
            }
            Err(e) => {
                spinner.finish_error("Failed to clean global cache");
                output.print_error(&e);
                Ok(1)
            }
        }
    } else {
        // Clean project cache
        let project_root = find_project_root(".")?;
        let project = Project::load(&project_root)?;

        // Initialize plugin hooks only if this project actually configures
        // clean hooks. Spinning up the plugin runtime otherwise dominates the
        // cost of an operation that is just an `rm -rf .hx`.
        let has_clean_hooks = !project.manifest.plugins.hooks.pre_clean.is_empty()
            || !project.manifest.plugins.hooks.post_clean.is_empty();
        let mut hooks = if has_clean_hooks {
            PluginHooks::from_project(&project, None)
        } else {
            None
        };
        if let Some(ref mut h) = hooks {
            if let Err(e) = h.initialize() {
                output.verbose(&format!("Plugin initialization warning: {}", e));
            }
        }

        // Run pre-clean hooks
        if let Some(ref mut h) = hooks {
            if !h.run_pre_hook(HookEvent::PreClean, output) {
                output.error("Pre-clean hook failed");
                return Ok(6); // Hook failure exit code
            }
        }

        output.status("Cleaning", &project_root.display().to_string());

        let spinner = Spinner::new("Removing project cache...");

        match clean_project_cache(&project_root) {
            Ok(()) => {
                spinner.finish_success("Project cache cleaned");

                // Run post-clean hooks
                if let Some(ref mut h) = hooks {
                    h.run_post_hook(HookEvent::PostClean, output);
                }

                Ok(0)
            }
            Err(e) => {
                spinner.finish_error("Failed to clean project cache");

                // Run post-clean hooks even on failure
                if let Some(ref mut h) = hooks {
                    h.run_post_hook(HookEvent::PostClean, output);
                }

                output.print_error(&e);
                Ok(1)
            }
        }
    }
}
