//! Plugin system integration for CLI commands.
//!
//! This module provides helpers for running pre/post hooks in command handlers.

use hx_config::Project;
use hx_plugins::{
    BuildContext, HookEvent, PluginConfig, PluginContext, PluginManager, TestContext,
};
use hx_ui::Output;
use std::time::Duration;

/// Warn (once per invocation) when local plugins exist but the project is untrusted.
pub fn warn_if_untrusted_local_plugins(config: &PluginConfig, project_root: &std::path::Path) {
    if config.trust_local {
        return;
    }
    let local_dir = PluginConfig::local_plugins_dir(project_root);
    let has_local_plugins = std::fs::read_dir(&local_dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|ext| ext == "scm"))
        })
        .unwrap_or(false);
    if has_local_plugins {
        Output::new().warn(&format!(
            "Project-local plugins in {} were NOT loaded because this project is not trusted.\n  Run `hx plugins trust` to allow them after reviewing the scripts.",
            local_dir.display()
        ));
    }
}

/// A helper for running plugin hooks in command handlers.
pub struct PluginHooks {
    manager: Option<PluginManager>,
    project_root: std::path::PathBuf,
    project_name: String,
    ghc_version: Option<String>,
    continue_on_error: bool,
}

impl PluginHooks {
    /// Create a new plugin hooks helper from a project.
    ///
    /// Returns None if plugins are disabled.
    pub fn from_project(project: &Project, ghc_version: Option<String>) -> Option<Self> {
        let mut config: PluginConfig = project.manifest.plugins.clone().into();

        if !config.enabled {
            return None;
        }

        config.trust_local = hx_config::is_project_trusted(&project.root);
        warn_if_untrusted_local_plugins(&config, &project.root);

        let manager = match PluginManager::new(config.clone()) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Failed to create plugin manager: {}", e);
                return None;
            }
        };

        Some(PluginHooks {
            manager: Some(manager),
            project_root: project.root.clone(),
            project_name: project.name().to_string(),
            ghc_version,
            continue_on_error: config.continue_on_error,
        })
    }

    /// Initialize the plugin system and load plugins.
    pub fn initialize(&mut self) -> Result<(), hx_plugins::PluginError> {
        if let Some(ref mut manager) = self.manager {
            manager.initialize()?;
            manager.load_all(&self.project_root)?;
        }
        Ok(())
    }

    /// Run a pre-hook (e.g., PreBuild, PreTest, PreRun).
    pub fn run_pre_hook(&mut self, event: HookEvent, output: &Output) -> bool {
        self.run_hook(event, None, None, output)
    }

    /// Run a post-hook with optional build context.
    pub fn run_post_build_hook(
        &mut self,
        success: bool,
        duration: Duration,
        warnings: Vec<String>,
        errors: Vec<String>,
        output: &Output,
    ) -> bool {
        let build_ctx = BuildContext {
            success,
            duration,
            warnings,
            errors,
        };
        self.run_hook(HookEvent::PostBuild, Some(build_ctx), None, output)
    }

    /// Run a post-hook with optional test context.
    pub fn run_post_test_hook(
        &mut self,
        passed: bool,
        passed_count: usize,
        failed_count: usize,
        skipped_count: usize,
        output: &Output,
    ) -> bool {
        let test_ctx = TestContext {
            passed,
            passed_count,
            failed_count,
            skipped_count,
            duration: Duration::ZERO, // Duration not tracked yet
        };
        self.run_hook(HookEvent::PostTest, None, Some(test_ctx), output)
    }

    /// Run a simple post-hook (no build/test context).
    pub fn run_post_hook(&mut self, event: HookEvent, output: &Output) -> bool {
        self.run_hook(event, None, None, output)
    }

    /// Run a hook with the given event and optional contexts.
    fn run_hook(
        &mut self,
        event: HookEvent,
        build: Option<BuildContext>,
        test: Option<TestContext>,
        output: &Output,
    ) -> bool {
        let manager = match self.manager.as_mut() {
            Some(m) => m,
            None => return true, // No manager, hooks pass
        };

        // Check if there are any hooks configured for this event
        let scripts = manager.config().scripts_for_hook(event);
        if scripts.is_empty() {
            return true;
        }

        let event_name = event.config_key();
        output.verbose(&format!("Running {} hooks...", event_name));

        let mut ctx = PluginContext::new(self.project_root.clone(), self.project_name.clone());
        ctx.ghc_version = self.ghc_version.clone();
        ctx.build = build;
        ctx.test = test;

        match manager.run_hook(event, &ctx) {
            Ok(result) => {
                if result.success {
                    let duration = result.duration;
                    if duration.as_millis() > 0 {
                        output.verbose(&format!(
                            "{} hooks completed in {:.2}s",
                            event_name,
                            duration.as_secs_f64()
                        ));
                    }
                    true
                } else {
                    if let Some(err) = &result.error {
                        output.warn(&format!("{} hook failed: {}", event_name, err));
                    }
                    if self.continue_on_error {
                        output.verbose("Continuing despite hook failure (continue_on_error=true)");
                        true
                    } else {
                        false
                    }
                }
            }
            Err(e) => {
                output.warn(&format!("{} hook error: {}", event_name, e));
                if self.continue_on_error {
                    output.verbose("Continuing despite hook error (continue_on_error=true)");
                    true
                } else {
                    false
                }
            }
        }
    }
}
