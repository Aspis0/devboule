//! clippy runner.
//!
//! `cargo clippy --message-format=json` emits one JSON object per line on stdout.
//! We parse only `"reason":"compiler-message"` objects and pull the diagnostic
//! `level`, `message`, optional lint `code.code`, and the PRIMARY span's
//! `file_name`/`line_start`. Severity/category via `severity_from_clippy`.
//!
//! Granularity is Coarse: clippy is crate-level and slow, so A3 runs it on the
//! coarse debounce, not per-file.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_clippy;
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

/// This runner's trigger granularity (crate-level → coarse).
pub fn granularity() -> Granularity {
    Granularity::Coarse
}

#[derive(Deserialize)]
struct CargoLine {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    message: Option<RustcMessage>,
}

#[derive(Deserialize)]
struct RustcMessage {
    #[serde(default)]
    message: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    code: Option<RustcCode>,
    #[serde(default)]
    spans: Vec<RustcSpan>,
}

#[derive(Deserialize)]
struct RustcCode {
    #[serde(default)]
    code: String,
}

#[derive(Deserialize)]
struct RustcSpan {
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    line_start: u32,
    #[serde(default)]
    is_primary: bool,
}

/// Parse `cargo clippy --message-format=json` stdout into raw findings. PURE — no
/// IO. Malformed lines and non-`compiler-message` objects are skipped; a message
/// with no primary span and no spans at all is dropped (we can't anchor it to a
/// file). Tolerant: a single bad line never aborts the whole parse.
pub fn parse_clippy(stdout: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: CargoLine = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // tolerant: skip non-JSON / malformed lines
        };
        if parsed.reason != "compiler-message" {
            continue;
        }
        let msg = match parsed.message {
            Some(m) => m,
            None => continue,
        };
        // Skip the trailing "aborting due to N previous errors" summaries: they
        // carry no code and no span.
        let span = msg
            .spans
            .iter()
            .find(|s| s.is_primary)
            .or_else(|| msg.spans.first());
        let span = match span {
            Some(s) if !s.file_name.is_empty() => s,
            _ => continue,
        };
        let (severity, category) = severity_from_clippy(&msg.level);
        let lint = msg.code.as_ref().map(|c| c.code.as_str()).unwrap_or("");
        let title = if lint.is_empty() {
            truncate_title(&msg.message)
        } else {
            format!("{lint}: {}", truncate_title(&msg.message))
        };
        out.push(RawFinding {
            file: span.file_name.replace('\\', "/"),
            line: Some(span.line_start),
            severity,
            category,
            source: "clippy".to_string(),
            title,
            body: truncate_body(&msg.message),
        });
    }
    out
}

/// Trim a diagnostic message to a single-line title (first line, capped).
/// PRIVACY: a clippy diagnostic can echo a string literal (e.g. on an inline
/// secret), so redact secret-shaped tokens BEFORE capping.
fn truncate_title(msg: &str) -> String {
    let safe = redact_secrets(msg.lines().next().unwrap_or("").trim());
    cap(&safe, 200)
}

/// Cap the body. clippy messages can echo source literals, so redact secret-
/// shaped tokens BEFORE capping. We also cap to keep shards small.
fn truncate_body(msg: &str) -> String {
    cap(&redact_secrets(msg.trim()), 1000)
}

/// Run clippy from the project root using the project's own clippy config. Absent
/// `cargo` → empty (no error). Coarse: ignores `target.file_rel_path`.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("cargo") {
        return Vec::new();
    }
    // No `--` extra args: let the project's clippy.toml / lints config drive.
    // A full clippy compile is slow on a cold target; allow a generous budget.
    let stdout = run_capture_with_timeout(
        "cargo",
        &["clippy", "--message-format=json", "--quiet"],
        root,
        Duration::from_secs(300),
    );
    match stdout {
        Some(s) => parse_clippy(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_clippy_warning_with_primary_span() {
        // A trimmed but real-shaped clippy JSON line.
        let line = r#"{"reason":"compiler-message","message":{"message":"unused variable: `x`","level":"warning","code":{"code":"unused_variables"},"spans":[{"file_name":"src/main.rs","line_start":3,"is_primary":true}]}}"#;
        let findings = parse_clippy(line);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/main.rs");
        assert_eq!(f.line, Some(3));
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.category, Category::Correctness);
        assert_eq!(f.source, "clippy");
        assert!(f.title.starts_with("unused_variables: "));
    }

    #[test]
    fn maps_error_level_to_high() {
        let line = r#"{"reason":"compiler-message","message":{"message":"mismatched types","level":"error","code":{"code":"E0308"},"spans":[{"file_name":"src/lib.rs","line_start":10,"is_primary":true}]}}"#;
        let findings = parse_clippy(line);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].line, Some(10));
    }

    #[test]
    fn skips_non_compiler_message_and_summary() {
        let lines = "\
{\"reason\":\"compiler-artifact\",\"target\":{}}\n\
{\"reason\":\"compiler-message\",\"message\":{\"message\":\"aborting due to previous error\",\"level\":\"error\",\"spans\":[]}}\n\
{\"reason\":\"build-finished\",\"success\":false}";
        let findings = parse_clippy(lines);
        // The summary has no span → dropped; artifact/build-finished ignored.
        assert!(findings.is_empty());
    }

    #[test]
    fn tolerant_to_malformed_line() {
        let lines = "not json at all\n{bad}\n";
        assert!(parse_clippy(lines).is_empty());
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_clippy("").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let line = r#"{"reason":"compiler-message","message":{"message":"this string literal AKIAIOSFODNN7EXAMPLE could be a const","level":"warning","code":{"code":"clippy::literal"},"spans":[{"file_name":"src/main.rs","line_start":3,"is_primary":true}]}}"#;
        let findings = parse_clippy(line);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(
            !f.title.contains("AKIAIOSFODNN7EXAMPLE"),
            "leaked in title: {}",
            f.title
        );
        assert!(
            !f.body.contains("AKIAIOSFODNN7EXAMPLE"),
            "leaked in body: {}",
            f.body
        );
    }

    #[test]
    fn picks_primary_span_over_first() {
        let line = r#"{"reason":"compiler-message","message":{"message":"m","level":"warning","spans":[{"file_name":"a.rs","line_start":1,"is_primary":false},{"file_name":"b.rs","line_start":99,"is_primary":true}]}}"#;
        let findings = parse_clippy(line);
        assert_eq!(findings.len(), 1);
        // The primary span (b.rs) wins over the first span (a.rs).
        assert_eq!(findings[0].file, "b.rs");
        assert_eq!(findings[0].line, Some(99));
    }
}
