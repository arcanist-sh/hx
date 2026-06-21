//! Error types for hx.

use std::path::PathBuf;

/// Result type alias using hx Error.
pub type Result<T> = std::result::Result<T, Error>;

/// Error codes for categorizing failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Tool not found in PATH
    ToolchainMissing,
    /// Wrong version of a tool
    ToolchainMismatch,
    /// HLS version doesn't match GHC
    HlsMismatch,
    /// Native library not found
    SystemDepMissing,
    /// Package resolution failed
    SolverFailure,
    /// Compilation error
    BuildFailure,
    /// Invalid configuration
    ConfigError,
    /// I/O error
    IoError,
    /// Command execution failed
    CommandFailed,
    /// Lock file error
    LockError,
}

impl ErrorCode {
    /// Process exit code for this error class, following hx's CLI conventions:
    /// 1 general, 3 config, 4 toolchain, 5 build/test.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::ToolchainMissing
            | ErrorCode::ToolchainMismatch
            | ErrorCode::HlsMismatch
            | ErrorCode::SystemDepMissing => 4,
            ErrorCode::ConfigError | ErrorCode::LockError => 3,
            ErrorCode::SolverFailure | ErrorCode::BuildFailure => 5,
            ErrorCode::IoError | ErrorCode::CommandFailed => 1,
        }
    }
}

/// A fix suggestion for an error.
#[derive(Debug, Clone)]
pub struct Fix {
    /// Description of what this fix does
    pub description: String,
    /// Command to run, if applicable
    pub command: Option<String>,
}

impl Fix {
    /// Create a fix with just a description.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            command: None,
        }
    }

    /// Create a fix with a command.
    pub fn with_command(description: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            command: Some(command.into()),
        }
    }
}

/// Structured error type for hx.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("toolchain not found: {tool}")]
    ToolchainMissing {
        tool: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
        fixes: Vec<Fix>,
    },

    #[error("toolchain version mismatch for {tool}: expected {expected}, found {found}")]
    ToolchainMismatch {
        tool: String,
        expected: String,
        found: String,
        fixes: Vec<Fix>,
    },

    #[error("configuration error: {message}")]
    Config {
        message: String,
        path: Option<PathBuf>,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
        fixes: Vec<Fix>,
    },

    #[error("I/O error: {message}")]
    Io {
        message: String,
        path: Option<PathBuf>,
        #[source]
        source: std::io::Error,
    },

    #[error("command failed: {command}")]
    CommandFailed {
        command: String,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        fixes: Vec<Fix>,
    },

    #[error("build failed")]
    BuildFailed {
        errors: Vec<String>,
        fixes: Vec<Fix>,
    },

    #[error("lock error: {message}")]
    Lock {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
        fixes: Vec<Fix>,
    },

    #[error("project not found")]
    ProjectNotFound {
        searched: Vec<PathBuf>,
        fixes: Vec<Fix>,
    },

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Get the error code for this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            Error::ToolchainMissing { .. } => ErrorCode::ToolchainMissing,
            Error::ToolchainMismatch { .. } => ErrorCode::ToolchainMismatch,
            Error::Config { .. } => ErrorCode::ConfigError,
            Error::Io { .. } => ErrorCode::IoError,
            Error::CommandFailed { .. } => ErrorCode::CommandFailed,
            Error::BuildFailed { .. } => ErrorCode::BuildFailure,
            Error::Lock { .. } => ErrorCode::LockError,
            Error::ProjectNotFound { .. } => ErrorCode::ConfigError,
            Error::Other(_) => ErrorCode::IoError,
        }
    }

    /// Get suggested fixes for this error.
    pub fn fixes(&self) -> &[Fix] {
        match self {
            Error::ToolchainMissing { fixes, .. } => fixes,
            Error::ToolchainMismatch { fixes, .. } => fixes,
            Error::Config { fixes, .. } => fixes,
            Error::CommandFailed { fixes, .. } => fixes,
            Error::BuildFailed { fixes, .. } => fixes,
            Error::Lock { fixes, .. } => fixes,
            Error::ProjectNotFound { fixes, .. } => fixes,
            Error::Io { .. } | Error::Other(_) => &[],
        }
    }

    /// Create a config error.
    pub fn config(message: impl Into<String>) -> Self {
        Error::Config {
            message: message.into(),
            path: None,
            source: None,
            fixes: vec![],
        }
    }

    /// Create a config error with a path.
    pub fn config_at(message: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Error::Config {
            message: message.into(),
            path: Some(path.into()),
            source: None,
            fixes: vec![],
        }
    }

    /// Create a toolchain missing error with default fix suggestions.
    pub fn toolchain_missing(tool: impl Into<String>) -> Self {
        let tool = tool.into();
        let fixes = match tool.as_str() {
            "ghc" => vec![
                Fix::with_command("Install GHC via ghcup", "ghcup install ghc"),
                Fix::with_command("Or install via hx", "hx toolchain install"),
            ],
            "cabal" => vec![
                Fix::with_command("Install Cabal via ghcup", "ghcup install cabal"),
                Fix::with_command("Or install via hx", "hx toolchain install"),
            ],
            "ghcup" => vec![Fix::new(
                "Install ghcup from https://www.haskell.org/ghcup/",
            )],
            "hls" | "haskell-language-server" => vec![
                Fix::with_command("Install HLS via ghcup", "ghcup install hls"),
                Fix::with_command("Or install via hx", "hx toolchain install --hls latest"),
            ],
            _ => vec![Fix::with_command(
                format!("Install {}", tool),
                "hx toolchain install",
            )],
        };

        Error::ToolchainMissing {
            tool,
            source: None,
            fixes,
        }
    }

    /// Create a toolchain mismatch error with default fix suggestions.
    pub fn toolchain_mismatch(
        tool: impl Into<String>,
        expected: impl Into<String>,
        found: impl Into<String>,
    ) -> Self {
        let tool = tool.into();
        let expected = expected.into();
        let found = found.into();

        let fixes = vec![
            Fix::with_command(
                format!("Install {} {}", tool, expected),
                format!(
                    "hx toolchain install --{} {}",
                    tool.to_lowercase(),
                    expected
                ),
            ),
            Fix::with_command(
                format!("Or use {} {} for this session", tool, expected),
                format!("ghcup set {} {}", tool.to_lowercase(), expected),
            ),
        ];

        Error::ToolchainMismatch {
            tool,
            expected,
            found,
            fixes,
        }
    }

    /// Create a project not found error with helpful suggestions.
    pub fn project_not_found(searched: Vec<PathBuf>) -> Self {
        Error::ProjectNotFound {
            searched,
            fixes: vec![
                Fix::with_command("Initialize a new project", "hx init"),
                Fix::new("Or navigate to a directory containing hx.toml or *.cabal"),
            ],
        }
    }

    /// Create a lock error with suggestions.
    pub fn lock_outdated() -> Self {
        Error::Lock {
            message: "lockfile is out of date".to_string(),
            source: None,
            fixes: vec![
                Fix::with_command("Update the lockfile", "hx lock"),
                Fix::with_command("Or force sync with current lock", "hx sync --force"),
            ],
        }
    }

    /// Create a build failed error with common suggestions.
    pub fn build_failed(errors: Vec<String>) -> Self {
        let mut fixes = vec![Fix::with_command(
            "See full compiler output",
            "hx build --verbose",
        )];

        // Add specific suggestions based on error content
        for error in &errors {
            if error.contains("Could not find module") {
                fixes.push(Fix::with_command(
                    "Missing dependency - add it",
                    "hx add <package-name>",
                ));
                break;
            }
            if error.contains("parse error") || error.contains("Parse error") {
                fixes.push(Fix::new("Check syntax near the reported line"));
                break;
            }
        }

        Error::BuildFailed { errors, fixes }
    }
}
