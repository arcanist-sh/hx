//! BHC diagnostic output parsing.

use hx_compiler::{Diagnostic, DiagnosticSeverity};
use std::path::PathBuf;

/// Parse BHC compiler output into structured diagnostics.
///
/// BHC uses a format similar to GHC but with some extensions:
/// - `file:line:col: error[E0001]: message`
/// - `file:line:col: warning[W0001]: message`
/// - `file:line:col: hint: message`
pub fn parse_bhc_output(output: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut current_diagnostic: Option<Diagnostic> = None;
    let mut note_lines: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            // Finalize current diagnostic if any
            if let Some(mut diag) = current_diagnostic.take() {
                for note in note_lines.drain(..) {
                    diag = diag.with_note(note);
                }
                diagnostics.push(diag);
            }
            continue;
        }

        // Check for note/help continuation lines
        if trimmed.starts_with("note:") || trimmed.starts_with("help:") {
            let note_content = trimmed
                .split_once(':')
                .map(|(_, msg)| msg.trim())
                .unwrap_or(trimmed);
            note_lines.push(note_content.to_string());
            continue;
        }

        // Parse main diagnostic line
        if let Some(diag) = parse_diagnostic_line(line) {
            // Finalize previous diagnostic if any
            if let Some(mut prev_diag) = current_diagnostic.take() {
                for note in note_lines.drain(..) {
                    prev_diag = prev_diag.with_note(note);
                }
                diagnostics.push(prev_diag);
            }
            current_diagnostic = Some(diag);
        }
    }

    // Finalize last diagnostic if any
    if let Some(mut diag) = current_diagnostic.take() {
        for note in note_lines.drain(..) {
            diag = diag.with_note(note);
        }
        diagnostics.push(diag);
    }

    diagnostics
}

/// Parse a single diagnostic line.
fn parse_diagnostic_line(line: &str) -> Option<Diagnostic> {
    // Try to match: file:line:col: severity[code]: message
    // or: file:line:col: severity: message

    // Split at the first occurrence of ": error", ": warning", or ": hint",
    // preferring them in that order when a line contains more than one.
    let pos = [": error", ": warning", ": hint"]
        .iter()
        .find_map(|marker| line.find(marker))?;
    let (location, rest) = line.split_at(pos);
    let severity_and_message = rest.strip_prefix(": ")?;

    // Parse severity and code
    let (severity, code, message) = parse_severity_and_message(severity_and_message)?;

    // Parse location
    let (file, line_num, col_num) = parse_location(location)?;

    let mut diag = match severity {
        DiagnosticSeverity::Error => Diagnostic::error(message),
        DiagnosticSeverity::Warning => Diagnostic::warning(message),
        DiagnosticSeverity::Hint => Diagnostic {
            severity: DiagnosticSeverity::Hint,
            code: None,
            message: message.to_string(),
            file: None,
            line: None,
            column: None,
            notes: Vec::new(),
        },
    };

    diag.file = Some(file);
    diag.line = line_num;
    diag.column = col_num;
    if let Some(code) = code {
        diag = diag.with_code(code);
    }

    Some(diag)
}

/// Parse severity, optional code, and message from a string like "error[E0001]: message".
fn parse_severity_and_message(s: &str) -> Option<(DiagnosticSeverity, Option<String>, &str)> {
    let (severity_part, message) = s.split_once(':')?;
    let severity_part = severity_part.trim();
    let message = message.trim();

    let (severity, code) = if severity_part.starts_with("error") {
        let code = extract_code(severity_part);
        (DiagnosticSeverity::Error, code)
    } else if severity_part.starts_with("warning") {
        let code = extract_code(severity_part);
        (DiagnosticSeverity::Warning, code)
    } else if severity_part.starts_with("hint") {
        (DiagnosticSeverity::Hint, None)
    } else {
        return None;
    };

    Some((severity, code, message))
}

/// Extract error code from "error[E0001]" format.
fn extract_code(s: &str) -> Option<String> {
    if let Some(start) = s.find('[')
        && let Some(end) = s.find(']')
    {
        return Some(s[start + 1..end].to_string());
    }
    None
}

/// Parse a location string like "src/Foo.hs:10:5".
fn parse_location(location: &str) -> Option<(PathBuf, Option<u32>, Option<u32>)> {
    let parts: Vec<&str> = location.split(':').collect();
    match parts.len() {
        1 => Some((PathBuf::from(parts[0]), None, None)),
        2 => {
            let line = parts[1].parse().ok();
            Some((PathBuf::from(parts[0]), line, None))
        }
        _ => {
            let line = parts[1].parse().ok();
            let col = parts[2].parse().ok();
            Some((PathBuf::from(parts[0]), line, col))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bhc_error() {
        let output = "src/Main.hs:15:10: error[E0001]: Variable not in scope: undefined";
        let diagnostics = parse_bhc_output(output);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].code, Some("E0001".to_string()));
        assert_eq!(diagnostics[0].message, "Variable not in scope: undefined");
        assert_eq!(diagnostics[0].file, Some(PathBuf::from("src/Main.hs")));
        assert_eq!(diagnostics[0].line, Some(15));
        assert_eq!(diagnostics[0].column, Some(10));
    }

    #[test]
    fn test_parse_bhc_warning() {
        let output = "src/Lib.hs:23:5: warning[W0001]: Unused variable 'x'";
        let diagnostics = parse_bhc_output(output);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Warning);
        assert_eq!(diagnostics[0].code, Some("W0001".to_string()));
    }

    #[test]
    fn test_parse_bhc_without_code() {
        let output = "src/Main.hs:15:10: error: Variable not in scope";
        let diagnostics = parse_bhc_output(output);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].code, None);
    }

    #[test]
    fn test_parse_bhc_with_notes() {
        let output = r#"src/Main.hs:15:10: error: Type mismatch
note: Expected type 'Int'
note: Actual type 'String'
"#;
        let diagnostics = parse_bhc_output(output);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].notes.len(), 2);
        assert_eq!(diagnostics[0].notes[0], "Expected type 'Int'");
        assert_eq!(diagnostics[0].notes[1], "Actual type 'String'");
    }

    #[test]
    fn test_parse_multiple_diagnostics() {
        let output = r#"src/Main.hs:15:10: error: First error

src/Lib.hs:20:5: warning: A warning
"#;
        let diagnostics = parse_bhc_output(output);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[1].severity, DiagnosticSeverity::Warning);
    }
}
