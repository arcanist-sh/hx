//! Core types and orchestration for hx.
//!
//! This crate provides shared types, error handling, and command execution
//! utilities used across all hx crates.

pub mod command;
pub mod diagnostic;
pub mod env;
pub mod error;
pub mod fsutil;
pub mod version;

pub use command::{CommandOutput, CommandRunner};
pub use diagnostic::{
    DiagnosticReport, DiagnosticSeverity, GhcDiagnostic, QuickFix, SourceSpan, TextEdit,
};
pub use env::EnvVars;
pub use error::{Error, ErrorCode, Fix, Result};
pub use fsutil::atomic_write;
pub use version::Version;

/// Exit codes for hx CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    /// Success
    Success = 0,
    /// General error
    GeneralError = 1,
    /// Usage error (bad arguments)
    UsageError = 2,
    /// Configuration error
    ConfigError = 3,
    /// Toolchain error
    ToolchainError = 4,
    /// Build/test failure
    BuildError = 5,
}

impl From<ExitCode> for i32 {
    fn from(code: ExitCode) -> Self {
        code as i32
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        std::process::ExitCode::from(code as u8)
    }
}
