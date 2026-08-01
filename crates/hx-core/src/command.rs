//! Command execution utilities.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tracing::{debug, instrument};

use crate::error::{Error, Fix};

/// Output from a command execution.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// How long the command took
    pub duration: Duration,
}

impl CommandOutput {
    /// Check if the command succeeded.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Which of a child process's streams a streamed line originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    /// The child's standard output.
    Stdout,
    /// The child's standard error.
    Stderr,
}

/// A command runner that captures output and provides structured results.
#[derive(Debug, Clone, Default)]
pub struct CommandRunner {
    /// Working directory for commands
    pub working_dir: Option<std::path::PathBuf>,
    /// Environment variables to set
    pub env: Vec<(String, String)>,
    /// Whether to inherit the parent environment
    pub inherit_env: bool,
}

impl CommandRunner {
    /// Create a new command runner.
    pub fn new() -> Self {
        Self {
            working_dir: None,
            env: Vec::new(),
            inherit_env: true,
        }
    }

    /// Set the working directory.
    pub fn with_working_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.working_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Add an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Configure PATH to use a specific toolchain bin directory.
    ///
    /// Prepends the given bin directory to PATH, ensuring tools from that
    /// directory are found first. Can be called multiple times to add
    /// multiple bin directories.
    pub fn with_ghc_bin(mut self, bin_dir: impl AsRef<Path>) -> Self {
        // Check if we already have a PATH in our env list
        let current_path = self
            .env
            .iter()
            .rev()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());

        let bin_str = bin_dir.as_ref().to_string_lossy();

        #[cfg(windows)]
        let separator = ";";
        #[cfg(not(windows))]
        let separator = ":";

        let new_path = format!("{}{}{}", bin_str, separator, current_path);
        self.env.push(("PATH".into(), new_path));
        self
    }

    /// Run a command and capture output.
    #[instrument(skip(self, args), fields(program = %program.as_ref().to_string_lossy()))]
    pub async fn run<S, I>(&self, program: S, args: I) -> Result<CommandOutput, Error>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        let program_ref = program.as_ref();
        let args_vec: Vec<_> = args
            .into_iter()
            .map(|a| a.as_ref().to_os_string())
            .collect();

        debug!(
            "Running command: {} {:?}",
            program_ref.to_string_lossy(),
            args_vec
        );

        let mut cmd = Command::new(program_ref);
        cmd.args(&args_vec)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(ref dir) = self.working_dir {
            cmd.current_dir(dir);
        }

        if !self.inherit_env {
            cmd.env_clear();
        }

        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        let start = Instant::now();

        let output = cmd
            .output()
            .await
            .map_err(|e| Self::spawn_error(program_ref, e))?;

        let duration = start.elapsed();

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        debug!(
            exit_code = exit_code,
            duration_ms = duration.as_millis(),
            "Command completed"
        );

        Ok(CommandOutput {
            exit_code,
            stdout,
            stderr,
            duration,
        })
    }

    /// Map a spawn/exec failure to a structured, actionable error.
    ///
    /// A `NotFound` becomes a [`Error::ToolchainMissing`] with an install fix;
    /// anything else becomes a generic [`Error::Io`].
    fn spawn_error(program: &OsStr, e: std::io::Error) -> Error {
        let program_str = program.to_string_lossy().to_string();
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::ToolchainMissing {
                tool: program_str.clone(),
                source: Some(Box::new(e)),
                fixes: vec![Fix::with_command(
                    format!("Install {}", program_str),
                    "hx toolchain install".to_string(),
                )],
            }
        } else {
            Error::Io {
                message: format!("failed to execute {}", program_str),
                path: None,
                source: e,
            }
        }
    }

    /// Run a command, streaming each complete line of stdout/stderr to
    /// `on_line` as it is produced, while still capturing the full output into
    /// the returned [`CommandOutput`].
    ///
    /// Unlike [`run`](Self::run) — which buffers everything and only returns
    /// once the child exits — this surfaces progress live. `on_line` is
    /// invoked on the current task (never concurrently) for every line, tagged
    /// with the [`OutputStream`] it came from. The captured `stdout`/`stderr`
    /// on the result are still populated for post-hoc parsing.
    #[instrument(skip(self, args, on_line), fields(program = %program.as_ref().to_string_lossy()))]
    pub async fn run_streaming<S, I, F>(
        &self,
        program: S,
        args: I,
        mut on_line: F,
    ) -> Result<CommandOutput, Error>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
        F: FnMut(OutputStream, &str),
    {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let program_ref = program.as_ref();
        let args_vec: Vec<_> = args
            .into_iter()
            .map(|a| a.as_ref().to_os_string())
            .collect();

        debug!(
            "Streaming command: {} {:?}",
            program_ref.to_string_lossy(),
            args_vec
        );

        let mut cmd = Command::new(program_ref);
        cmd.args(&args_vec)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(ref dir) = self.working_dir {
            cmd.current_dir(dir);
        }
        if !self.inherit_env {
            cmd.env_clear();
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        let start = Instant::now();

        let mut child = cmd.spawn().map_err(|e| Self::spawn_error(program_ref, e))?;

        // stdout/stderr are always `Some` because we configured them as piped.
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let mut out_lines = BufReader::new(stdout).lines();
        let mut err_lines = BufReader::new(stderr).lines();

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut out_done = false;
        let mut err_done = false;

        // Interleave both streams as lines arrive. The loop guard guarantees at
        // least one branch is enabled whenever we enter `select!`.
        while !(out_done && err_done) {
            tokio::select! {
                line = out_lines.next_line(), if !out_done => match line {
                    Ok(Some(l)) => {
                        on_line(OutputStream::Stdout, &l);
                        stdout_buf.push_str(&l);
                        stdout_buf.push('\n');
                    }
                    _ => out_done = true,
                },
                line = err_lines.next_line(), if !err_done => match line {
                    Ok(Some(l)) => {
                        on_line(OutputStream::Stderr, &l);
                        stderr_buf.push_str(&l);
                        stderr_buf.push('\n');
                    }
                    _ => err_done = true,
                },
            }
        }

        let status = child.wait().await.map_err(|e| Error::Io {
            message: format!("failed to wait for {}", program_ref.to_string_lossy()),
            path: None,
            source: e,
        })?;

        let duration = start.elapsed();
        let exit_code = status.code().unwrap_or(-1);

        debug!(
            exit_code = exit_code,
            duration_ms = duration.as_millis(),
            "Streaming command completed"
        );

        Ok(CommandOutput {
            exit_code,
            stdout: stdout_buf,
            stderr: stderr_buf,
            duration,
        })
    }

    /// Run a command with the parent's stdio inherited, so its output streams
    /// straight to the terminal and it can read from stdin, and return its exit
    /// code.
    ///
    /// Used for interactive passthrough (e.g. `hx run`), where the child's
    /// output must reach the user live and an interactive program needs stdin.
    /// No output is captured.
    #[instrument(skip(self, args), fields(program = %program.as_ref().to_string_lossy()))]
    pub async fn run_inherited<S, I>(&self, program: S, args: I) -> Result<i32, Error>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        let program_ref = program.as_ref();
        let args_vec: Vec<_> = args
            .into_iter()
            .map(|a| a.as_ref().to_os_string())
            .collect();

        debug!(
            "Running (inherited stdio): {} {:?}",
            program_ref.to_string_lossy(),
            args_vec
        );

        let mut cmd = Command::new(program_ref);
        cmd.args(&args_vec)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        if let Some(ref dir) = self.working_dir {
            cmd.current_dir(dir);
        }
        if !self.inherit_env {
            cmd.env_clear();
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        let status = cmd
            .status()
            .await
            .map_err(|e| Self::spawn_error(program_ref, e))?;

        Ok(status.code().unwrap_or(-1))
    }

    /// Run a command and return an error if it fails.
    pub async fn run_checked<S, I>(&self, program: S, args: I) -> Result<CommandOutput, Error>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        let program_str = program.as_ref().to_string_lossy().to_string();
        let output = self.run(program, args).await?;

        if !output.success() {
            return Err(Error::CommandFailed {
                command: program_str,
                exit_code: Some(output.exit_code),
                stdout: output.stdout,
                stderr: output.stderr,
                fixes: vec![],
            });
        }

        Ok(output)
    }
}

// The streaming tests rely on a POSIX shell to produce deterministic
// stdout/stderr and exit codes; they are skipped on non-unix targets.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_streaming_streams_lines_and_captures_output() {
        let runner = CommandRunner::new();
        let mut seen: Vec<(OutputStream, String)> = Vec::new();

        let output = runner
            .run_streaming(
                "sh",
                ["-c", "echo out1; echo err1 1>&2; echo out2"],
                |stream, line| seen.push((stream, line.to_string())),
            )
            .await
            .expect("command should run");

        assert_eq!(output.exit_code, 0);
        assert!(output.success());
        // Full output is still captured for post-hoc parsing.
        assert!(output.stdout.contains("out1"));
        assert!(output.stdout.contains("out2"));
        assert!(output.stderr.contains("err1"));
        // And every line was delivered live to the callback.
        assert_eq!(seen.len(), 3);
        assert!(
            seen.iter()
                .any(|(s, l)| *s == OutputStream::Stderr && l == "err1")
        );
        assert!(
            seen.iter()
                .any(|(s, l)| *s == OutputStream::Stdout && l == "out2")
        );
    }

    #[tokio::test]
    async fn run_streaming_reports_nonzero_exit_code() {
        let runner = CommandRunner::new();
        let output = runner
            .run_streaming("sh", ["-c", "exit 3"], |_, _| {})
            .await
            .expect("command should run");
        assert_eq!(output.exit_code, 3);
        assert!(!output.success());
    }

    #[tokio::test]
    async fn run_streaming_missing_tool_is_toolchain_missing() {
        let runner = CommandRunner::new();
        let err = runner
            .run_streaming(
                "hx-definitely-not-a-real-binary",
                std::iter::empty::<&str>(),
                |_, _| {},
            )
            .await
            .expect_err("missing tool should error");
        assert!(matches!(err, Error::ToolchainMissing { .. }));
    }

    #[tokio::test]
    async fn run_inherited_returns_exit_code() {
        let runner = CommandRunner::new();
        let code = runner
            .run_inherited("sh", ["-c", "exit 7"])
            .await
            .expect("command should run");
        assert_eq!(code, 7);
    }
}
