//! Persistent compilation server for fast incremental rebuilds.
//!
//! This module provides a long-running GHCi process that can reload modules
//! quickly instead of spawning fresh GHC processes for each build.

use hx_core::{Error, Fix, Result};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Configuration for the compilation server.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Source directories to watch
    pub src_dirs: Vec<PathBuf>,
    /// Package databases to use
    pub package_dbs: Vec<PathBuf>,
    /// Packages to expose
    pub packages: Vec<String>,
    /// Path to GHCi executable
    pub ghc_path: PathBuf,
    /// Idle timeout before auto-shutdown (0 = never)
    pub idle_timeout_ms: u64,
    /// GHC language extensions to enable
    pub extensions: Vec<String>,
    /// Additional GHCi options
    pub ghci_options: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            src_dirs: vec![PathBuf::from("src")],
            package_dbs: Vec::new(),
            packages: Vec::new(),
            ghc_path: PathBuf::from("ghci"),
            idle_timeout_ms: 0, // No timeout by default
            extensions: Vec::new(),
            ghci_options: Vec::new(),
        }
    }
}

/// Result of a reload operation.
#[derive(Debug, Clone)]
pub struct ReloadResult {
    /// Whether the reload succeeded
    pub success: bool,
    /// Modules that were recompiled
    pub modules_recompiled: Vec<String>,
    /// Time taken for the reload
    pub duration: Duration,
    /// Error messages
    pub errors: Vec<String>,
    /// Warning messages
    pub warnings: Vec<String>,
}

/// Result of a type-check operation.
#[derive(Debug, Clone)]
pub struct TypeCheckResult {
    /// Whether the type check succeeded
    pub success: bool,
    /// Error messages
    pub errors: Vec<String>,
    /// Warning messages
    pub warnings: Vec<String>,
    /// Time taken
    pub duration: Duration,
}

/// Status of the compilation server.
#[derive(Debug, Clone)]
pub struct ServerStatus {
    /// Whether the server is running
    pub running: bool,
    /// Number of loaded modules
    pub loaded_modules: usize,
    /// Time since last activity
    pub idle_time: Duration,
    /// GHC version
    pub ghc_version: Option<String>,
    /// Loaded module names
    pub module_names: Vec<String>,
}

/// Internal state of the GHCi process.
struct GhciProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    loaded_modules: HashSet<String>,
    last_activity: Instant,
    ghc_version: Option<String>,
}

/// Parsed output from a GHCi reload command.
type ReloadOutput = (bool, Vec<String>, Vec<String>, Vec<String>);

/// Persistent compilation server.
pub struct CompilationServer {
    inner: Arc<Mutex<Option<GhciProcess>>>,
    project_root: PathBuf,
    config: ServerConfig,
}

impl CompilationServer {
    /// Start a new compilation server.
    pub async fn start(project_root: PathBuf, config: ServerConfig) -> Result<Self> {
        // Clean up any stale socket file before starting
        let socket_path = server_socket_path(&project_root);
        if socket_path.exists() {
            debug!("Removing stale socket file: {}", socket_path.display());
            let _ = std::fs::remove_file(&socket_path);
        }

        let server = Self {
            inner: Arc::new(Mutex::new(None)),
            project_root,
            config,
        };

        // Start the GHCi process
        server.spawn_ghci().await?;

        // Create the socket file to indicate server is running
        let socket_path = server_socket_path(&server.project_root);
        if let Some(parent) = socket_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Write PID to socket file for stale detection
        if let Err(e) = std::fs::write(&socket_path, std::process::id().to_string()) {
            warn!("Failed to create socket file: {}", e);
        }

        Ok(server)
    }

    /// Connect to an existing server or start a new one.
    pub async fn connect_or_start(project_root: PathBuf, config: ServerConfig) -> Result<Self> {
        // Check if server socket exists and is valid
        let socket_path = server_socket_path(&project_root);

        if socket_path.exists() {
            // Check if the PID in the socket file is still running
            if let Ok(pid_str) = std::fs::read_to_string(&socket_path)
                && let Ok(pid) = pid_str.trim().parse::<u32>()
                && is_process_running(pid)
            {
                // Server is already running, but we can't connect to it yet
                // (IPC not implemented), so warn and start fresh
                warn!(
                    "Existing server (PID {}) found but IPC not implemented, starting new",
                    pid
                );
            }
            // Socket exists but server is dead or invalid - clean up
            debug!("Cleaning up stale socket file");
            let _ = std::fs::remove_file(&socket_path);
        }

        Self::start(project_root, config).await
    }

    /// Spawn the GHCi process.
    async fn spawn_ghci(&self) -> Result<()> {
        let ghci_path = self.config.ghc_path.display().to_string();

        let mut cmd = Command::new(&ghci_path);

        // Set working directory
        cmd.current_dir(&self.project_root);

        // Add source directories
        for src_dir in &self.config.src_dirs {
            cmd.arg("-i");
            cmd.arg(src_dir);
        }

        // Add package databases
        for db in &self.config.package_dbs {
            cmd.arg("-package-db");
            cmd.arg(db);
        }

        // Add packages
        for pkg in &self.config.packages {
            cmd.arg("-package");
            cmd.arg(pkg);
        }

        // Add extensions
        for ext in &self.config.extensions {
            cmd.arg(format!("-X{}", ext));
        }

        // Add custom GHCi options
        for opt in &self.config.ghci_options {
            cmd.arg(opt);
        }

        // Configure for non-interactive use
        cmd.arg("-ignore-dot-ghci");
        cmd.arg("-fno-ghci-history");
        // Note: Don't use -v0 as it suppresses the prompt which we need to detect readiness

        // Set up pipes
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        debug!("Starting GHCi: {:?}", cmd);

        let mut child = cmd.spawn().map_err(|e| Error::ToolchainMissing {
            tool: "ghci".to_string(),
            source: Some(Box::new(e)),
            fixes: vec![Fix::with_command(
                "Ensure GHC is installed",
                "ghcup install ghc",
            )],
        })?;

        let stdin = child.stdin.take().ok_or_else(|| Error::BuildFailed {
            errors: vec!["Failed to capture GHCi stdin".to_string()],
            fixes: vec![],
        })?;

        let stdout = child.stdout.take().ok_or_else(|| Error::BuildFailed {
            errors: vec!["Failed to capture GHCi stdout".to_string()],
            fixes: vec![],
        })?;

        let mut process = GhciProcess {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            loaded_modules: HashSet::new(),
            last_activity: Instant::now(),
            ghc_version: None,
        };

        // Wait for GHCi to be ready and get version
        process.ghc_version = Self::read_ghci_version(&mut process.stdout)?;

        *self.inner.lock().await = Some(process);

        info!("Compilation server started");
        Ok(())
    }

    /// Read GHCi startup output to detect version.
    /// Note: We can't easily wait for the prompt since it doesn't end with a newline.
    /// Instead, we read lines until we see the version info.
    fn read_ghci_version(stdout: &mut BufReader<ChildStdout>) -> Result<Option<String>> {
        let mut version = None;
        let mut line = String::new();

        // Read lines looking for version info
        // We'll read at most a few lines since GHCi startup is fast
        for _ in 0..10 {
            line.clear();
            match stdout.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim();

                    // Check for version info
                    if trimmed.starts_with("GHCi, version") {
                        version = Some(trimmed.to_string());
                        // Version line found, GHCi should be ready
                        break;
                    }
                }
                Err(e) => {
                    warn!("Error reading GHCi output: {}", e);
                    break;
                }
            }
        }

        Ok(version)
    }

    /// Reload all modules.
    pub async fn reload(&self) -> Result<ReloadResult> {
        let start = Instant::now();
        let mut guard = self.inner.lock().await;

        let process = guard.as_mut().ok_or_else(|| Error::BuildFailed {
            errors: vec!["Server not running".to_string()],
            fixes: vec![Fix::with_command("Start the server", "hx server start")],
        })?;

        process.last_activity = Instant::now();

        // Send reload command
        writeln!(process.stdin, ":reload").map_err(|e| Error::Io {
            message: "Failed to write to GHCi".to_string(),
            path: None,
            source: e,
        })?;

        process.stdin.flush().map_err(|e| Error::Io {
            message: "Failed to flush GHCi stdin".to_string(),
            path: None,
            source: e,
        })?;

        // Read output until prompt
        let (success, modules_recompiled, errors, warnings) =
            Self::read_reload_output(&mut process.stdout)?;

        // Update loaded modules
        for module in &modules_recompiled {
            process.loaded_modules.insert(module.clone());
        }

        Ok(ReloadResult {
            success,
            modules_recompiled,
            duration: start.elapsed(),
            errors,
            warnings,
        })
    }

    /// Read the output from a reload command.
    fn read_reload_output(stdout: &mut BufReader<ChildStdout>) -> Result<ReloadOutput> {
        let mut modules = Vec::new();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut success = true;
        let mut line = String::new();

        loop {
            line.clear();
            match stdout.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim();

                    // Check for prompt (end of output)
                    if trimmed.ends_with(">") {
                        break;
                    }

                    // Parse output
                    if trimmed.contains("Compiling") {
                        // Extract module name
                        if let Some(module) = Self::extract_module_name(trimmed) {
                            modules.push(module);
                        }
                    } else if trimmed.contains("error:") || trimmed.contains("Error:") {
                        success = false;
                        errors.push(trimmed.to_string());
                    } else if trimmed.contains("warning:") || trimmed.contains("Warning:") {
                        warnings.push(trimmed.to_string());
                    } else if trimmed.contains("Failed") {
                        success = false;
                    }
                }
                Err(e) => {
                    warn!("Error reading GHCi output: {}", e);
                    break;
                }
            }
        }

        Ok((success, modules, errors, warnings))
    }

    /// Extract module name from a "Compiling" line.
    fn extract_module_name(line: &str) -> Option<String> {
        // Format: "[1 of 5] Compiling Module.Name"
        if let Some(idx) = line.find("Compiling") {
            let rest = line[idx + "Compiling".len()..].trim_start();
            let module_name = rest.split_whitespace().next()?;
            return Some(module_name.to_string());
        }
        None
    }

    /// Type-check a specific file.
    pub async fn type_check(&self, file: &Path) -> Result<TypeCheckResult> {
        let start = Instant::now();
        let mut guard = self.inner.lock().await;

        let process = guard.as_mut().ok_or_else(|| Error::BuildFailed {
            errors: vec!["Server not running".to_string()],
            fixes: vec![Fix::with_command("Start the server", "hx server start")],
        })?;

        process.last_activity = Instant::now();

        // Send type-check command
        writeln!(process.stdin, ":type-check {}", file.display()).map_err(|e| Error::Io {
            message: "Failed to write to GHCi".to_string(),
            path: None,
            source: e,
        })?;

        process.stdin.flush().map_err(|e| Error::Io {
            message: "Failed to flush GHCi stdin".to_string(),
            path: None,
            source: e,
        })?;

        // Read output
        let (success, _, errors, warnings) = Self::read_reload_output(&mut process.stdout)?;

        Ok(TypeCheckResult {
            success,
            errors,
            warnings,
            duration: start.elapsed(),
        })
    }

    /// Get the current server status.
    pub async fn status(&self) -> Result<ServerStatus> {
        let guard = self.inner.lock().await;

        match guard.as_ref() {
            Some(process) => Ok(ServerStatus {
                running: true,
                loaded_modules: process.loaded_modules.len(),
                idle_time: process.last_activity.elapsed(),
                ghc_version: process.ghc_version.clone(),
                module_names: process.loaded_modules.iter().cloned().collect(),
            }),
            None => Ok(ServerStatus {
                running: false,
                loaded_modules: 0,
                idle_time: Duration::ZERO,
                ghc_version: None,
                module_names: Vec::new(),
            }),
        }
    }

    /// Shutdown the server.
    pub async fn shutdown(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;

        if let Some(mut process) = guard.take() {
            // Send quit command
            let _ = writeln!(process.stdin, ":quit");
            let _ = process.stdin.flush();

            // Wait for process to exit
            let _ = process.child.wait();

            info!("Compilation server stopped");
        }

        // Remove socket file
        let socket_path = server_socket_path(&self.project_root);
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        Ok(())
    }

    /// Load a module by name.
    pub async fn load_module(&self, module: &str) -> Result<ReloadResult> {
        let start = Instant::now();
        let mut guard = self.inner.lock().await;

        let process = guard.as_mut().ok_or_else(|| Error::BuildFailed {
            errors: vec!["Server not running".to_string()],
            fixes: vec![Fix::with_command("Start the server", "hx server start")],
        })?;

        process.last_activity = Instant::now();

        // Send load command
        writeln!(process.stdin, ":load {}", module).map_err(|e| Error::Io {
            message: "Failed to write to GHCi".to_string(),
            path: None,
            source: e,
        })?;

        process.stdin.flush().map_err(|e| Error::Io {
            message: "Failed to flush GHCi stdin".to_string(),
            path: None,
            source: e,
        })?;

        // Read output
        let (success, modules_recompiled, errors, warnings) =
            Self::read_reload_output(&mut process.stdout)?;

        // Update loaded modules
        for m in &modules_recompiled {
            process.loaded_modules.insert(m.clone());
        }

        Ok(ReloadResult {
            success,
            modules_recompiled,
            duration: start.elapsed(),
            errors,
            warnings,
        })
    }

    /// Check if the server is running.
    pub async fn is_running(&self) -> bool {
        let guard = self.inner.lock().await;
        guard.is_some()
    }
}

impl Drop for CompilationServer {
    fn drop(&mut self) {
        // Best-effort cleanup of GHCi process
        if let Ok(mut guard) = self.inner.try_lock()
            && let Some(mut process) = guard.take()
        {
            let _ = writeln!(process.stdin, ":quit");
            let _ = process.stdin.flush();
            let _ = process.child.wait();
        }

        // Clean up socket file
        let socket_path = server_socket_path(&self.project_root);
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }
    }
}

/// Get the path to the server socket file for a project.
pub fn server_socket_path(project_root: &Path) -> PathBuf {
    project_root.join(".hx").join("server.sock")
}

/// Check if a compilation server is running for a project.
///
/// This checks if the socket file exists AND the process is still alive.
pub fn is_server_running(project_root: &Path) -> bool {
    let socket_path = server_socket_path(project_root);
    if !socket_path.exists() {
        return false;
    }

    // Read PID from socket file and check if process is alive
    if let Ok(pid_str) = std::fs::read_to_string(&socket_path)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
    {
        return is_process_running(pid);
    }

    // Socket exists but we can't determine if server is running
    // Assume it's stale
    false
}

/// Check if a process with the given PID is running.
#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    // On Unix, we can use kill with signal 0 to check if process exists
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn is_process_running(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let result = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        result != 0 && exit_code == STILL_ACTIVE
    }
}

#[cfg(windows)]
unsafe extern "system" {
    fn OpenProcess(
        dwDesiredAccess: u32,
        bInheritHandle: i32,
        dwProcessId: u32,
    ) -> *mut std::ffi::c_void;
    fn GetExitCodeProcess(hProcess: *mut std::ffi::c_void, lpExitCode: *mut u32) -> i32;
    fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.src_dirs, vec![PathBuf::from("src")]);
        assert!(config.packages.is_empty());
        assert_eq!(config.idle_timeout_ms, 0);
        assert!(config.package_dbs.is_empty());
        assert!(config.extensions.is_empty());
        assert!(config.ghci_options.is_empty());
        assert_eq!(config.ghc_path, PathBuf::from("ghci"));
    }

    #[test]
    fn test_server_config_custom() {
        let config = ServerConfig {
            src_dirs: vec![PathBuf::from("src"), PathBuf::from("lib")],
            package_dbs: vec![PathBuf::from("/path/to/db")],
            packages: vec!["base".to_string(), "text".to_string()],
            ghc_path: PathBuf::from("/usr/bin/ghci"),
            idle_timeout_ms: 60000,
            extensions: vec!["OverloadedStrings".to_string()],
            ghci_options: vec!["-Wall".to_string()],
        };
        assert_eq!(config.src_dirs.len(), 2);
        assert_eq!(config.packages.len(), 2);
        assert_eq!(config.idle_timeout_ms, 60000);
        assert_eq!(config.extensions.len(), 1);
    }

    #[test]
    fn test_server_socket_path() {
        let project = PathBuf::from("/home/user/project");
        let socket = server_socket_path(&project);
        assert_eq!(socket, PathBuf::from("/home/user/project/.hx/server.sock"));
    }

    #[test]
    fn test_server_socket_path_nested() {
        let project = PathBuf::from("/a/b/c/d/project");
        let socket = server_socket_path(&project);
        assert_eq!(socket, PathBuf::from("/a/b/c/d/project/.hx/server.sock"));
    }

    #[test]
    fn test_is_server_running_no_socket() {
        // Non-existent path should return false
        assert!(!is_server_running(Path::new("/nonexistent/path")));
    }

    #[test]
    fn test_extract_module_name() {
        assert_eq!(
            CompilationServer::extract_module_name("[1 of 5] Compiling Data.Text"),
            Some("Data.Text".to_string())
        );
        assert_eq!(
            CompilationServer::extract_module_name("[2 of 5] Compiling Main ( src/Main.hs )"),
            Some("Main".to_string())
        );
        assert_eq!(
            CompilationServer::extract_module_name("Some other output"),
            None
        );
    }

    #[test]
    fn test_extract_module_name_qualified() {
        assert_eq!(
            CompilationServer::extract_module_name("[1 of 10] Compiling Control.Monad.Trans.State"),
            Some("Control.Monad.Trans.State".to_string())
        );
    }

    #[test]
    fn test_extract_module_name_with_brackets() {
        assert_eq!(
            CompilationServer::extract_module_name(
                "[15 of 20] Compiling Foo.Bar ( src/Foo/Bar.hs, interpreted )"
            ),
            Some("Foo.Bar".to_string())
        );
    }

    #[test]
    fn test_extract_module_name_single_module() {
        assert_eq!(
            CompilationServer::extract_module_name("[1 of 1] Compiling Main"),
            Some("Main".to_string())
        );
    }

    #[test]
    fn test_server_status_default() {
        let status = ServerStatus {
            running: false,
            loaded_modules: 0,
            idle_time: Duration::ZERO,
            ghc_version: None,
            module_names: Vec::new(),
        };
        assert!(!status.running);
        assert_eq!(status.loaded_modules, 0);
        assert!(status.ghc_version.is_none());
        assert!(status.module_names.is_empty());
    }

    #[test]
    fn test_server_status_running() {
        let status = ServerStatus {
            running: true,
            loaded_modules: 5,
            idle_time: Duration::from_secs(30),
            ghc_version: Some("GHCi, version 9.8.2".to_string()),
            module_names: vec!["Main".to_string(), "Lib".to_string()],
        };
        assert!(status.running);
        assert_eq!(status.loaded_modules, 5);
        assert!(status.ghc_version.is_some());
        assert_eq!(status.module_names.len(), 2);
    }

    #[test]
    fn test_reload_result_success() {
        let result = ReloadResult {
            success: true,
            modules_recompiled: vec!["Main".to_string(), "Lib".to_string()],
            duration: Duration::from_millis(150),
            errors: Vec::new(),
            warnings: vec!["unused import".to_string()],
        };
        assert!(result.success);
        assert_eq!(result.modules_recompiled.len(), 2);
        assert!(result.errors.is_empty());
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn test_reload_result_failure() {
        let result = ReloadResult {
            success: false,
            modules_recompiled: vec!["Main".to_string()],
            duration: Duration::from_millis(50),
            errors: vec!["type error".to_string(), "scope error".to_string()],
            warnings: Vec::new(),
        };
        assert!(!result.success);
        assert_eq!(result.errors.len(), 2);
    }

    #[test]
    fn test_type_check_result_success() {
        let result = TypeCheckResult {
            success: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            duration: Duration::from_millis(20),
        };
        assert!(result.success);
        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_type_check_result_with_warnings() {
        let result = TypeCheckResult {
            success: true,
            errors: Vec::new(),
            warnings: vec!["unused variable 'x'".to_string()],
            duration: Duration::from_millis(25),
        };
        assert!(result.success);
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn test_type_check_result_failure() {
        let result = TypeCheckResult {
            success: false,
            errors: vec!["Type mismatch".to_string()],
            warnings: Vec::new(),
            duration: Duration::from_millis(30),
        };
        assert!(!result.success);
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn test_reload_result_clone() {
        let result = ReloadResult {
            success: true,
            modules_recompiled: vec!["Test".to_string()],
            duration: Duration::from_millis(100),
            errors: Vec::new(),
            warnings: Vec::new(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.success, result.success);
        assert_eq!(cloned.modules_recompiled, result.modules_recompiled);
    }

    #[test]
    fn test_server_status_clone() {
        let status = ServerStatus {
            running: true,
            loaded_modules: 3,
            idle_time: Duration::from_secs(10),
            ghc_version: Some("9.8.2".to_string()),
            module_names: vec!["A".to_string(), "B".to_string()],
        };
        let cloned = status.clone();
        assert_eq!(cloned.running, status.running);
        assert_eq!(cloned.loaded_modules, status.loaded_modules);
        assert_eq!(cloned.module_names, status.module_names);
    }

    #[test]
    fn test_server_config_clone() {
        let config = ServerConfig {
            src_dirs: vec![PathBuf::from("src")],
            packages: vec!["base".to_string()],
            ..Default::default()
        };
        let cloned = config.clone();
        assert_eq!(cloned.src_dirs, config.src_dirs);
        assert_eq!(cloned.packages, config.packages);
    }
}
