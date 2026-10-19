//! LSP server implementation.

use crate::state::{ServerState, WorkspaceState};
use hx_cabal::{diagnostic_flags, parse_ghc_json, parse_ghc_text, supports_json_diagnostics};
use hx_core::{DiagnosticReport, DiagnosticSeverity, GhcDiagnostic, QuickFix};
use hx_toolchain::Toolchain;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, error, info, warn};

/// The hx Language Server.
pub struct HxLanguageServer {
    /// LSP client for sending notifications.
    client: Client,
    /// Server state.
    state: Arc<ServerState>,
}

impl HxLanguageServer {
    /// Create a new language server instance.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Arc::new(ServerState::new()),
        }
    }

    /// Initialize a workspace and detect GHC version.
    async fn init_workspace(&self, root: PathBuf) -> Arc<WorkspaceState> {
        let workspace = self.state.add_workspace(root.clone());

        // Detect GHC version
        let toolchain = Toolchain::detect().await;
        if let Some(version) = toolchain.ghc.status.version() {
            let version_str = version.to_string();
            let supports_json = supports_json_diagnostics(&version_str);

            // Update workspace state (we need interior mutability here)
            // For now, just log it - in a real impl we'd use RwLock
            info!(
                "Workspace {} using GHC {} (JSON diagnostics: {})",
                root.display(),
                version_str,
                supports_json
            );
        }

        workspace
    }

    /// Run GHC to check a file and get diagnostics.
    async fn check_file(&self, file: &PathBuf) -> DiagnosticReport {
        let workspace = match self.state.find_workspace(file) {
            Some(ws) => ws,
            None => {
                warn!("No workspace found for file: {}", file.display());
                return DiagnosticReport::new();
            }
        };

        // Determine GHC flags
        let ghc_version = workspace
            .ghc_version
            .as_deref()
            .unwrap_or(hx_toolchain::RECOMMENDED_GHC_VERSION);
        let extra_flags = diagnostic_flags(ghc_version);

        // Build GHC command
        let mut cmd = Command::new("ghc");
        cmd.arg("-fno-code") // Don't generate code, just check
            .arg("-fforce-recomp") // Always recompile
            .args(&extra_flags)
            .arg(file)
            .current_dir(&workspace.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!(
            "Running: ghc -fno-code {:?} {}",
            extra_flags,
            file.display()
        );

        match cmd.output().await {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let combined = format!("{}\n{}", stdout, stderr);

                // Parse diagnostics based on format
                if workspace.supports_json {
                    parse_ghc_json(&combined)
                } else {
                    parse_ghc_text(&combined)
                }
            }
            Err(e) => {
                error!("Failed to run GHC: {}", e);
                DiagnosticReport::new()
            }
        }
    }

    /// Publish diagnostics to the client.
    async fn publish_diagnostics(&self, uri: Url, diagnostics: Vec<GhcDiagnostic>) {
        let lsp_diags: Vec<Diagnostic> = diagnostics.iter().map(ghc_to_lsp_diagnostic).collect();

        self.client.publish_diagnostics(uri, lsp_diags, None).await;
    }

    /// Handle a file being opened or saved.
    async fn on_file_change(&self, uri: &Url) {
        let path = match uri.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                warn!("Invalid file URI: {}", uri);
                return;
            }
        };

        // Only check Haskell files
        if !is_haskell_file(&path) {
            return;
        }

        info!("Checking file: {}", path.display());

        let report = self.check_file(&path).await;

        // Update workspace diagnostics
        if let Some(workspace) = self.state.find_workspace(&path) {
            workspace.update_diagnostics(report.clone());
        }

        // Publish diagnostics for each affected file
        for (file, diags) in &report.by_file {
            if let Ok(uri) = Url::from_file_path(file) {
                self.publish_diagnostics(uri, diags.clone()).await;
            }
        }

        // If no errors for this file, clear its diagnostics
        if report.for_file(&path).map(|d| d.is_empty()).unwrap_or(true) {
            self.publish_diagnostics(uri.clone(), vec![]).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for HxLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        info!("LSP server initializing");

        // Initialize workspace(s)
        if let Some(folders) = params.workspace_folders {
            for folder in folders {
                if let Ok(path) = folder.uri.to_file_path() {
                    self.init_workspace(path).await;
                }
            }
        } else if let Some(root_uri) = params.root_uri
            && let Ok(path) = root_uri.to_file_path()
        {
            self.init_workspace(path).await;
        }

        self.state.set_initialized(true).await;

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "hx-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        info!("LSP server initialized");
    }

    async fn shutdown(&self) -> Result<()> {
        info!("LSP server shutting down");
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.on_file_change(&params.text_document.uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.on_file_change(&params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // Clear diagnostics for closed file
        self.publish_diagnostics(params.text_document.uri, vec![])
            .await;
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let path = match uri.to_file_path() {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        let workspace = match self.state.find_workspace(&path) {
            Some(ws) => ws,
            None => return Ok(None),
        };

        let diagnostics = workspace.get_diagnostics(&path);
        let range = params.range;

        // Find diagnostics that overlap with the requested range
        let mut actions = Vec::new();

        for diag in diagnostics {
            if let Some(span) = &diag.span {
                // Check if diagnostic overlaps with requested range
                if span.start_line <= range.end.line + 1 && span.end_line > range.start.line {
                    // Add quick fixes as code actions
                    for fix in &diag.fixes {
                        if let Some(action) = quick_fix_to_code_action(fix, uri, &diag) {
                            actions.push(CodeActionOrCommand::CodeAction(action));
                        }
                    }
                }
            }
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }
}

/// Convert a GHC diagnostic to an LSP diagnostic.
fn ghc_to_lsp_diagnostic(diag: &GhcDiagnostic) -> Diagnostic {
    let severity = match diag.severity {
        DiagnosticSeverity::Error => tower_lsp::lsp_types::DiagnosticSeverity::ERROR,
        DiagnosticSeverity::Warning => tower_lsp::lsp_types::DiagnosticSeverity::WARNING,
        DiagnosticSeverity::Info => tower_lsp::lsp_types::DiagnosticSeverity::INFORMATION,
        DiagnosticSeverity::Hint => tower_lsp::lsp_types::DiagnosticSeverity::HINT,
    };

    let range = match &diag.span {
        Some(span) => Range {
            start: Position {
                line: span.start_line.saturating_sub(1),
                character: span.start_col.saturating_sub(1),
            },
            end: Position {
                line: span.end_line.saturating_sub(1),
                character: span.end_col.saturating_sub(1),
            },
        },
        None => Range::default(),
    };

    let mut message = diag.message.clone();
    if !diag.hints.is_empty() {
        message.push_str("\n\n");
        message.push_str(&diag.hints.join("\n"));
    }

    Diagnostic {
        range,
        severity: Some(severity),
        code: diag.code.clone().map(NumberOrString::String),
        code_description: None,
        source: Some("ghc".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Convert a quick fix to an LSP code action.
fn quick_fix_to_code_action(fix: &QuickFix, uri: &Url, diag: &GhcDiagnostic) -> Option<CodeAction> {
    // If there's a text edit, create a workspace edit
    let edit = fix.edit.as_ref().map(|text_edit| {
        let range = Range {
            start: Position {
                line: text_edit.range.start_line.saturating_sub(1),
                character: text_edit.range.start_col.saturating_sub(1),
            },
            end: Position {
                line: text_edit.range.end_line.saturating_sub(1),
                character: text_edit.range.end_col.saturating_sub(1),
            },
        };

        let lsp_edit = TextEdit {
            range,
            new_text: text_edit.new_text.clone(),
        };

        let changes = std::collections::HashMap::from([(uri.clone(), vec![lsp_edit])]);

        WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }
    });

    // If there's a command, create a command action
    let command = fix
        .command
        .as_ref()
        .map(|cmd| tower_lsp::lsp_types::Command {
            title: fix.title.clone(),
            command: "hx.runCommand".to_string(),
            arguments: Some(vec![serde_json::Value::String(cmd.clone())]),
        });

    // Only create action if we have either an edit or a command
    if edit.is_none() && command.is_none() {
        return None;
    }

    let lsp_diag = ghc_to_lsp_diagnostic(diag);

    Some(CodeAction {
        title: fix.title.clone(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![lsp_diag]),
        edit,
        command,
        is_preferred: Some(fix.is_preferred),
        disabled: None,
        data: None,
    })
}

/// Check if a path is a Haskell source file.
fn is_haskell_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext == "hs" || ext == "lhs")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_core::SourceSpan;

    #[test]
    fn test_ghc_to_lsp_diagnostic() {
        let diag = GhcDiagnostic {
            span: Some(SourceSpan::new(PathBuf::from("Main.hs"), 10, 5, 10, 8)),
            severity: DiagnosticSeverity::Error,
            code: Some("GHC-88464".to_string()),
            warning_flag: None,
            message: "Variable not in scope: foo".to_string(),
            hints: vec!["Perhaps you meant 'fooBar'".to_string()],
            fixes: vec![],
        };

        let lsp_diag = ghc_to_lsp_diagnostic(&diag);

        assert_eq!(
            lsp_diag.severity,
            Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
        );
        assert_eq!(lsp_diag.range.start.line, 9); // 0-indexed
        assert_eq!(lsp_diag.range.start.character, 4); // 0-indexed
        assert!(lsp_diag.message.contains("Variable not in scope"));
        assert!(lsp_diag.message.contains("Perhaps you meant"));
    }

    #[test]
    fn test_is_haskell_file() {
        assert!(is_haskell_file(&PathBuf::from("Main.hs")));
        assert!(is_haskell_file(&PathBuf::from("Lib.lhs")));
        assert!(!is_haskell_file(&PathBuf::from("main.rs")));
        assert!(!is_haskell_file(&PathBuf::from("Cargo.toml")));
    }
}
