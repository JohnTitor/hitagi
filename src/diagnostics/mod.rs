use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use lsp_types::{Diagnostic, DiagnosticSeverity, Range, Uri};
use serde_json::Value;

use crate::doc::position::lsp_position_from_span;
use crate::doc::uri::path_to_uri;

pub fn run_check(root: &Path, command: &[String]) -> Result<HashMap<Uri, Vec<Diagnostic>>, String> {
    let (program, args) = split_command(command)?;

    let mut cmd = Command::new(program);
    cmd.args(args);
    if !has_message_format(command) {
        cmd.arg("--message-format=json");
    }
    cmd.current_dir(root);

    let output = cmd.output().map_err(|err| err.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut diagnostics: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();

    for line in stdout.lines() {
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if value.get("reason").and_then(|v| v.as_str()) != Some("compiler-message") {
            continue;
        }

        let message = match value.get("message") {
            Some(v) => v,
            None => continue,
        };

        let level = message.get("level").and_then(|v| v.as_str()).unwrap_or("error");
        let msg_text = message
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("rustc error");

        let spans = match message.get("spans").and_then(|v| v.as_array()) {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };

        let span = spans
            .iter()
            .find(|span| span.get("is_primary").and_then(|v| v.as_bool()) == Some(true))
            .unwrap_or(&spans[0]);

        let file_name = match span.get("file_name").and_then(|v| v.as_str()) {
            Some(name) => name,
            None => continue,
        };

        let start = lsp_position_from_span(
            span.get("line_start").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
            span.get("column_start").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
        );
        let end = lsp_position_from_span(
            span.get("line_end").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
            span.get("column_end").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
        );

        let range = Range { start, end };
        let severity = map_severity(level);

        let diagnostic = Diagnostic {
            range,
            severity,
            code: None,
            code_description: None,
            source: Some("cargo".to_string()),
            message: msg_text.to_string(),
            related_information: None,
            tags: None,
            data: None,
        };

        if let Some(uri) = uri_from_file(root, file_name) {
            diagnostics.entry(uri).or_default().push(diagnostic);
        }
    }

    Ok(diagnostics)
}

fn split_command(command: &[String]) -> Result<(String, Vec<String>), String> {
    let mut iter = command.iter();
    let program = iter
        .next()
        .ok_or_else(|| "checkCommand is empty".to_string())?;
    let args = iter.cloned().collect();
    Ok((program.clone(), args))
}

fn has_message_format(command: &[String]) -> bool {
    command.iter().any(|arg| arg.contains("--message-format"))
}

fn map_severity(level: &str) -> Option<DiagnosticSeverity> {
    match level {
        "error" => Some(DiagnosticSeverity::ERROR),
        "warning" => Some(DiagnosticSeverity::WARNING),
        "note" => Some(DiagnosticSeverity::HINT),
        "help" => Some(DiagnosticSeverity::INFORMATION),
        _ => Some(DiagnosticSeverity::INFORMATION),
    }
}

fn uri_from_file(root: &Path, file_name: &str) -> Option<Uri> {
    let path = PathBuf::from(file_name);
    let full = if path.is_absolute() { path } else { root.join(path) };
    path_to_uri(&full)
}
