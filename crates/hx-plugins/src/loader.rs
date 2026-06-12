//! Plugin discovery and loading.

use crate::config::PluginConfig;
use crate::error::{PluginError, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Discovered plugin information.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    /// Path to the plugin file.
    pub path: PathBuf,

    /// Plugin name (filename without extension).
    pub name: String,

    /// Whether this is a project-local plugin.
    pub is_local: bool,
}

impl DiscoveredPlugin {
    /// Create a new discovered plugin.
    pub fn new(path: PathBuf, is_local: bool) -> Self {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        DiscoveredPlugin {
            path,
            name,
            is_local,
        }
    }
}

/// Discover plugins in the configured paths.
pub fn discover_plugins(
    config: &PluginConfig,
    project_root: &Path,
) -> Result<Vec<DiscoveredPlugin>> {
    let mut plugins = Vec::new();
    let paths = config.all_paths(project_root);
    let local_dir = PluginConfig::local_plugins_dir(project_root);

    for base_path in paths.iter() {
        if !base_path.exists() {
            debug!("Plugin path does not exist: {}", base_path.display());
            continue;
        }

        let is_local = *base_path == local_dir;

        // Find all .scm files in this directory
        let pattern = base_path.join("*.scm");
        match glob::glob(&pattern.to_string_lossy()) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    debug!("Discovered plugin: {}", entry.display());
                    plugins.push(DiscoveredPlugin::new(entry, is_local));
                }
            }
            Err(e) => {
                debug!("Failed to glob plugins in {}: {}", base_path.display(), e);
            }
        }
    }

    info!("Discovered {} plugins", plugins.len());
    Ok(plugins)
}

/// Find a specific plugin by name.
pub fn find_plugin(
    name: &str,
    config: &PluginConfig,
    project_root: &Path,
) -> Result<DiscoveredPlugin> {
    let paths = config.all_paths(project_root);
    let local_dir = PluginConfig::local_plugins_dir(project_root);

    // Add .scm extension if not present
    let filename = if name.ends_with(".scm") {
        name.to_string()
    } else {
        format!("{}.scm", name)
    };

    for base_path in paths.iter() {
        let plugin_path = base_path.join(&filename);
        if plugin_path.exists() {
            let is_local = *base_path == local_dir;
            return Ok(DiscoveredPlugin::new(plugin_path, is_local));
        }
    }

    Err(PluginError::not_found(PathBuf::from(name)))
}

/// Check if the plugins directory exists for a project.
pub fn plugins_dir_exists(project_root: &Path) -> bool {
    project_root.join(".hx").join("plugins").exists()
}

/// Create the plugins directory for a project.
pub fn create_plugins_dir(project_root: &Path) -> Result<PathBuf> {
    let plugins_dir = project_root.join(".hx").join("plugins");

    if !plugins_dir.exists() {
        std::fs::create_dir_all(&plugins_dir).map_err(|e| {
            PluginError::io(
                format!(
                    "failed to create plugins directory: {}",
                    plugins_dir.display()
                ),
                e,
            )
        })?;
    }

    Ok(plugins_dir)
}

/// Get information about plugin directories.
pub struct PluginPaths {
    /// Project-local plugins directory.
    pub local: PathBuf,
    /// Global plugins directory.
    pub global: Option<PathBuf>,
    /// Custom paths from configuration.
    pub custom: Vec<PathBuf>,
}

impl PluginPaths {
    /// Get plugin paths for a project.
    pub fn for_project(config: &PluginConfig, project_root: &Path) -> Self {
        let local = project_root.join(".hx").join("plugins");

        let global =
            directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("hx").join("plugins"));

        let custom = config
            .paths
            .iter()
            .map(|p| {
                let expanded = shellexpand(p);
                PathBuf::from(expanded)
            })
            .collect();

        PluginPaths {
            local,
            global,
            custom,
        }
    }

    /// Check which paths exist.
    pub fn existing(&self) -> Vec<&PathBuf> {
        let mut paths = Vec::new();

        if self.local.exists() {
            paths.push(&self.local);
        }

        for path in &self.custom {
            if path.exists() {
                paths.push(path);
            }
        }

        if let Some(ref global) = self.global
            && global.exists()
        {
            paths.push(global);
        }

        paths
    }
}

fn shellexpand(path: &str) -> String {
    if path.starts_with("~/")
        && let Some(dirs) = directories::BaseDirs::new()
    {
        return format!("{}{}", dirs.home_dir().display(), &path[1..]);
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_discover_plugins_empty() {
        let temp = tempdir().unwrap();
        let config = PluginConfig::default();
        let plugins = discover_plugins(&config, temp.path()).unwrap();
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_create_plugins_dir() {
        let temp = tempdir().unwrap();
        let plugins_dir = create_plugins_dir(temp.path()).unwrap();
        assert!(plugins_dir.exists());
        assert!(plugins_dir.ends_with(".hx/plugins"));
    }
}
