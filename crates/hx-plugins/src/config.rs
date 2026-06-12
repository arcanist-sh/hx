//! Plugin configuration types.

use crate::hooks::HookEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Plugin system configuration from hx.toml `[plugins]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    /// Whether plugins are enabled.
    pub enabled: bool,

    /// Timeout for hook execution in milliseconds.
    pub hook_timeout_ms: u64,

    /// Additional paths to search for plugins.
    pub paths: Vec<String>,

    /// Whether to continue on hook failure.
    pub continue_on_error: bool,

    /// Hook configuration.
    #[serde(default)]
    pub hooks: HookConfig,

    /// Whether project-local plugins (`.hx/plugins`) may be loaded.
    ///
    /// Never read from the manifest: local plugins run arbitrary code, so the
    /// project that ships them cannot be the one to authorize them. The CLI
    /// sets this after checking the user's trust list in the global config.
    #[serde(skip)]
    pub trust_local: bool,
}

impl PluginConfig {
    /// Create a new plugin config with defaults.
    pub fn new() -> Self {
        PluginConfig {
            enabled: true,
            hook_timeout_ms: 5000,
            paths: vec![],
            continue_on_error: false,
            hooks: HookConfig::default(),
            trust_local: false,
        }
    }

    /// Path to a project's local plugins directory.
    pub fn local_plugins_dir(project_root: &std::path::Path) -> PathBuf {
        project_root.join(".hx").join("plugins")
    }

    /// Get the timeout as a Duration.
    pub fn hook_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.hook_timeout_ms)
    }

    /// Get all plugin search paths, including defaults.
    ///
    /// The project-local directory is only included when the project has been
    /// trusted (see [`PluginConfig::trust_local`]).
    pub fn all_paths(&self, project_root: &std::path::Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // Project-local plugins first, but only for trusted projects
        if self.trust_local {
            paths.push(Self::local_plugins_dir(project_root));
        }

        // User-specified paths
        for path in &self.paths {
            let expanded = shellexpand::tilde(path);
            paths.push(PathBuf::from(expanded.as_ref()));
        }

        // Global plugins last
        if let Some(config_dir) = dirs::config_dir() {
            paths.push(config_dir.join("hx").join("plugins"));
        }

        paths
    }

    /// Get scripts for a specific hook event.
    pub fn scripts_for_hook(&self, event: HookEvent) -> &[String] {
        match event {
            HookEvent::PreBuild => &self.hooks.pre_build,
            HookEvent::PostBuild => &self.hooks.post_build,
            HookEvent::PreTest => &self.hooks.pre_test,
            HookEvent::PostTest => &self.hooks.post_test,
            HookEvent::PreRun => &self.hooks.pre_run,
            HookEvent::PostRun => &self.hooks.post_run,
            HookEvent::PreClean => &self.hooks.pre_clean,
            HookEvent::PostClean => &self.hooks.post_clean,
            HookEvent::PreLock => &self.hooks.pre_lock,
            HookEvent::PostLock => &self.hooks.post_lock,
            HookEvent::Init => &self.hooks.init,
        }
    }
}

/// Hook-specific configuration mapping events to scripts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookConfig {
    /// Scripts to run before build.
    pub pre_build: Vec<String>,

    /// Scripts to run after build.
    pub post_build: Vec<String>,

    /// Scripts to run before tests.
    pub pre_test: Vec<String>,

    /// Scripts to run after tests.
    pub post_test: Vec<String>,

    /// Scripts to run before run command.
    pub pre_run: Vec<String>,

    /// Scripts to run after run completes.
    pub post_run: Vec<String>,

    /// Scripts to run before clean.
    pub pre_clean: Vec<String>,

    /// Scripts to run after clean.
    pub post_clean: Vec<String>,

    /// Scripts to run before lock generation.
    pub pre_lock: Vec<String>,

    /// Scripts to run after lock completes.
    pub post_lock: Vec<String>,

    /// Scripts to run on project initialization.
    pub init: Vec<String>,
}

impl HookConfig {
    /// Check if any hooks are configured.
    pub fn has_any_hooks(&self) -> bool {
        !self.pre_build.is_empty()
            || !self.post_build.is_empty()
            || !self.pre_test.is_empty()
            || !self.post_test.is_empty()
            || !self.pre_run.is_empty()
            || !self.post_run.is_empty()
            || !self.pre_clean.is_empty()
            || !self.post_clean.is_empty()
            || !self.pre_lock.is_empty()
            || !self.post_lock.is_empty()
            || !self.init.is_empty()
    }

    /// Get a map of event to scripts.
    pub fn as_map(&self) -> HashMap<HookEvent, &[String]> {
        let mut map = HashMap::new();
        map.insert(HookEvent::PreBuild, self.pre_build.as_slice());
        map.insert(HookEvent::PostBuild, self.post_build.as_slice());
        map.insert(HookEvent::PreTest, self.pre_test.as_slice());
        map.insert(HookEvent::PostTest, self.post_test.as_slice());
        map.insert(HookEvent::PreRun, self.pre_run.as_slice());
        map.insert(HookEvent::PostRun, self.post_run.as_slice());
        map.insert(HookEvent::PreClean, self.pre_clean.as_slice());
        map.insert(HookEvent::PostClean, self.post_clean.as_slice());
        map.insert(HookEvent::PreLock, self.pre_lock.as_slice());
        map.insert(HookEvent::PostLock, self.post_lock.as_slice());
        map.insert(HookEvent::Init, self.init.as_slice());
        map
    }
}

// Use dirs crate for platform-independent config directory
mod dirs {
    use std::path::PathBuf;

    pub fn config_dir() -> Option<PathBuf> {
        directories::BaseDirs::new().map(|dirs| dirs.config_dir().to_path_buf())
    }
}

// Simple tilde expansion
mod shellexpand {
    use std::borrow::Cow;

    pub fn tilde(path: &str) -> Cow<'_, str> {
        if path.starts_with("~/")
            && let Some(home) = directories::BaseDirs::new()
        {
            let home_str = home.home_dir().to_string_lossy();
            return Cow::Owned(format!("{}{}", home_str, &path[1..]));
        }
        Cow::Borrowed(path)
    }
}

// Conversion from hx_config::PluginConfig to hx_plugins::PluginConfig
impl From<hx_config::PluginConfig> for PluginConfig {
    fn from(config: hx_config::PluginConfig) -> Self {
        PluginConfig {
            enabled: config.enabled,
            hook_timeout_ms: config.hook_timeout_ms,
            paths: config.paths,
            continue_on_error: config.continue_on_error,
            hooks: HookConfig::from(config.hooks),
            trust_local: false,
        }
    }
}

impl From<hx_config::PluginHookConfig> for HookConfig {
    fn from(hooks: hx_config::PluginHookConfig) -> Self {
        HookConfig {
            pre_build: hooks.pre_build,
            post_build: hooks.post_build,
            pre_test: hooks.pre_test,
            post_test: hooks.post_test,
            pre_run: hooks.pre_run,
            post_run: hooks.post_run,
            pre_clean: hooks.pre_clean,
            post_clean: hooks.post_clean,
            pre_lock: hooks.pre_lock,
            post_lock: hooks.post_lock,
            init: hooks.init,
        }
    }
}
