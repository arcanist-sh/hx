//! Steel Scheme engine wrapper and PluginSystem trait.

use crate::commands::CustomCommand;
use crate::config::PluginConfig;
use crate::context::{ContextGuard, PluginContext};
use crate::error::{PluginError, Result};
use crate::hooks::{HookEvent, HookResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use steel::steel_vm::engine::Engine;
use tracing::{debug, info, warn};

/// Trait for plugin system implementations.
///
/// This abstraction allows for potential future alternative runtimes
/// while providing a consistent interface.
pub trait PluginSystem {
    /// Initialize the plugin system.
    fn initialize(&mut self) -> Result<()>;

    /// Load a plugin from a file path.
    fn load_plugin(&mut self, path: &Path) -> Result<()>;

    /// Run hooks for an event.
    fn run_hook(&mut self, event: HookEvent, ctx: &PluginContext) -> Result<HookResult>;

    /// Run a custom command.
    fn run_command(&mut self, name: &str, args: &[String]) -> Result<i32>;

    /// Register the hx API functions.
    fn register_api(&mut self) -> Result<()>;

    /// Get registered custom commands.
    fn commands(&self) -> &HashMap<String, CustomCommand>;
}

/// Steel Scheme-based plugin engine.
pub struct SteelEngine {
    /// The Steel VM engine.
    engine: Engine,

    /// Loaded plugin scripts.
    loaded_plugins: Vec<PathBuf>,

    /// Registered custom commands.
    commands: HashMap<String, CustomCommand>,

    /// Plugin configuration.
    config: PluginConfig,
}

impl SteelEngine {
    /// Create a new Steel engine with the given configuration.
    pub fn new(config: PluginConfig) -> Self {
        SteelEngine {
            engine: Engine::new(),
            loaded_plugins: Vec::new(),
            commands: HashMap::new(),
            config,
        }
    }

    /// Create a new Steel engine with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(PluginConfig::new())
    }

    /// Get the plugin configuration.
    pub fn config(&self) -> &PluginConfig {
        &self.config
    }

    /// Check if a plugin is already loaded.
    pub fn is_loaded(&self, path: &Path) -> bool {
        self.loaded_plugins.iter().any(|p| p == path)
    }

    /// Get the list of loaded plugins.
    pub fn loaded_plugins(&self) -> &[PathBuf] {
        &self.loaded_plugins
    }

    /// Evaluate Scheme code (takes ownership of the string).
    pub fn eval(&mut self, code: String) -> Result<()> {
        self.engine
            .run(code)
            .map_err(|e| PluginError::runtime("eval", e.to_string()))?;
        Ok(())
    }

    /// Load and evaluate a file.
    fn load_file(&mut self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            PluginError::io(format!("failed to read plugin file: {}", path.display()), e)
        })?;

        self.engine.run(content).map_err(|e| {
            PluginError::load(path.to_path_buf(), format!("Steel evaluation error: {}", e))
        })?;

        Ok(())
    }

    /// Check if a function is defined in the engine.
    fn has_function(&mut self, name: &str) -> bool {
        engine_has_function(&mut self.engine, name)
    }

    /// Call a function with no arguments.
    fn call_function(&mut self, name: &str) -> Result<()> {
        engine_call_function(&mut self.engine, name)
    }
}

/// Check if a function is defined in an engine.
fn engine_has_function(engine: &mut Engine, name: &str) -> bool {
    let check_code = format!("(if (defined? '{}) #t #f)", name);
    match engine.run(check_code) {
        Ok(results) => {
            if let Some(result) = results.into_iter().next() {
                matches!(result, steel::SteelVal::BoolV(true))
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// Call a function with no arguments in an engine.
fn engine_call_function(engine: &mut Engine, name: &str) -> Result<()> {
    let call_code = format!("({})", name);
    engine
        .run(call_code)
        .map_err(|e| PluginError::runtime(name, e.to_string()))?;
    Ok(())
}

/// Create a fresh engine with the hx API and prelude loaded.
fn new_initialized_engine() -> Result<Engine> {
    let mut engine = Engine::new();
    crate::api::register_all(&mut engine)?;
    engine
        .run(include_str!("prelude.scm").to_string())
        .map_err(|e| PluginError::runtime("prelude", e.to_string()))?;
    Ok(engine)
}

/// Run hook scripts in a fresh engine on the current thread.
///
/// Used by the timeout-enforcing path: the worker thread cannot share the
/// main engine (it is not thread-safe), so the scripts are loaded into an
/// isolated engine with the same API and prelude.
fn run_hook_isolated(
    scripts: &[PathBuf],
    hook_fn: &str,
    ctx: &PluginContext,
    continue_on_error: bool,
) -> Result<HookResult> {
    let _guard = ContextGuard::new(ctx.clone());
    let start = Instant::now();
    let mut engine = new_initialized_engine()?;

    for path in scripts {
        let content = std::fs::read_to_string(path).map_err(|e| {
            PluginError::io(format!("failed to read plugin file: {}", path.display()), e)
        })?;
        engine.run(content).map_err(|e| {
            PluginError::load(path.clone(), format!("Steel evaluation error: {}", e))
        })?;

        if engine_has_function(&mut engine, hook_fn) {
            match engine_call_function(&mut engine, hook_fn) {
                Ok(()) => debug!("Hook {} completed successfully", hook_fn),
                Err(e) => {
                    warn!("Hook {} failed: {}", hook_fn, e);
                    if !continue_on_error {
                        return Ok(HookResult::failure(start.elapsed(), e.to_string()));
                    }
                }
            }
        }
    }

    Ok(HookResult::success(start.elapsed()))
}

impl PluginSystem for SteelEngine {
    fn initialize(&mut self) -> Result<()> {
        info!("Initializing Steel plugin engine");

        // Register the hx API
        self.register_api()?;

        // Load prelude/standard library if needed
        let prelude = include_str!("prelude.scm").to_string();
        self.eval(prelude)?;

        debug!("Steel engine initialized");
        Ok(())
    }

    fn load_plugin(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Err(PluginError::not_found(path.to_path_buf()));
        }

        if self.is_loaded(path) {
            debug!("Plugin already loaded: {}", path.display());
            return Ok(());
        }

        info!("Loading plugin: {}", path.display());
        self.load_file(path)?;
        self.loaded_plugins.push(path.to_path_buf());

        Ok(())
    }

    fn run_hook(&mut self, event: HookEvent, ctx: &PluginContext) -> Result<HookResult> {
        // Clone the scripts to avoid borrow checker issues
        let scripts: Vec<String> = self.config.scripts_for_hook(event).to_vec();

        if scripts.is_empty() {
            return Ok(HookResult::skipped());
        }

        debug!("Running {} hook with {} scripts", event, scripts.len());

        let project_root = ctx.project_root.clone();

        // Resolve all script paths up front
        let mut resolved = Vec::with_capacity(scripts.len());
        for script in &scripts {
            resolved.push(self.find_script(script, &project_root)?);
        }

        let timeout = self.config.hook_timeout();
        let hook_fn = event.scheme_function();

        // hook_timeout_ms = 0 disables the timeout: run in the shared engine
        if timeout.is_zero() {
            let _guard = ContextGuard::new(ctx.clone());
            let start = Instant::now();

            for script_path in &resolved {
                if !self.is_loaded(script_path) {
                    self.load_plugin(script_path)?;
                }

                if self.has_function(hook_fn) {
                    match self.call_function(hook_fn) {
                        Ok(()) => {
                            debug!("Hook {} completed successfully", hook_fn);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            warn!("Hook {} failed: {}", hook_fn, e);

                            if !self.config.continue_on_error {
                                return Ok(HookResult::failure(duration, e.to_string()));
                            }
                        }
                    }
                }
            }

            return Ok(HookResult::success(start.elapsed()));
        }

        // Enforce the timeout by running the hook on a worker thread with its
        // own engine; a hung script cannot stall the command indefinitely.
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx_clone = ctx.clone();
        let hook_fn = hook_fn.to_string();
        let continue_on_error = self.config.continue_on_error;
        let start = Instant::now();

        std::thread::Builder::new()
            .name("hx-plugin-hook".into())
            .spawn(move || {
                let result = run_hook_isolated(&resolved, &hook_fn, &ctx_clone, continue_on_error);
                let _ = tx.send(result);
            })
            .map_err(|e| PluginError::io("failed to spawn hook thread".to_string(), e))?;

        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                warn!(
                    "{} hook timed out after {}ms; abandoning hook thread",
                    event,
                    timeout.as_millis()
                );
                Ok(HookResult::failure(
                    start.elapsed(),
                    format!(
                        "hook timed out after {}ms (configure [plugins].hook_timeout_ms to adjust)",
                        timeout.as_millis()
                    ),
                ))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(PluginError::runtime(
                event.scheme_function(),
                "hook thread terminated unexpectedly".to_string(),
            )),
        }
    }

    fn run_command(&mut self, name: &str, args: &[String]) -> Result<i32> {
        if !self.commands.contains_key(name) {
            return Err(PluginError::unknown_command(name));
        }

        // Set up arguments in the engine
        let args_list = args
            .iter()
            .map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(" ");

        let call_code = format!("(hx/run-command \"{}\" (list {}))", name, args_list);

        match self.engine.run(call_code) {
            Ok(results) => {
                // Try to get an exit code from the result
                if let Some(steel::SteelVal::IntV(code)) = results.into_iter().next() {
                    return Ok(code as i32);
                }
                Ok(0)
            }
            Err(e) => Err(PluginError::runtime(name, e.to_string())),
        }
    }

    fn register_api(&mut self) -> Result<()> {
        // Register the hx API functions
        // These are registered using Steel's FFI mechanism
        crate::api::register_all(&mut self.engine)?;
        Ok(())
    }

    fn commands(&self) -> &HashMap<String, CustomCommand> {
        &self.commands
    }
}

impl SteelEngine {
    /// Find a script file in the plugin paths.
    fn find_script(&self, name: &str, project_root: &Path) -> Result<PathBuf> {
        let paths = self.config.all_paths(project_root);

        for base_path in &paths {
            let script_path = base_path.join(name);
            if script_path.exists() {
                return Ok(script_path);
            }
        }

        Err(PluginError::not_found(PathBuf::from(name)))
    }

    /// Register a custom command from Scheme.
    pub fn register_command(&mut self, cmd: CustomCommand) {
        info!("Registering custom command: {}", cmd.name);
        self.commands.insert(cmd.name.clone(), cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = SteelEngine::with_defaults();
        assert!(engine.loaded_plugins().is_empty());
        assert!(engine.commands().is_empty());
    }
}
