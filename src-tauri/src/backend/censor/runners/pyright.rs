//! pyright runner.
//!
//! `pyright --outputjson <file>` emits a JSON object with `generalDiagnostics` array.
//! Each diagnostic has `severity` ("error"|"warning"|"information"), `message`,
//! `range` (0-based line/character), and `rule`. Severity/category via
//! `severity_from_pyright`. Granularity is Fine (per-file).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_pyright;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunTarget};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

#[derive(Deserialize)]
struct PyrightResult {
    #[serde(default, rename = "generalDiagnostics")]
    general_diagnostics: Vec<PyrightDiagnostic>,
}

#[derive(Deserialize)]
struct PyrightDiagnostic {
    #[serde(default)]
    file: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    range: Option<PyrightRange>,
    #[serde(default)]
    rule: Option<String>,
}

#[derive(Deserialize)]
struct PyrightRange {
    #[serde(default)]
    start: PyrightPosition,
}

#[derive(Deserialize, Default)]
struct PyrightPosition {
    #[serde(default)]
    line: u32,
}

/// Parse `pyright --outputjson` stdout. PURE. `file_hint` is the project-relative
/// path of the file we asked pyright to check; pyright reports absolute paths in
/// `file`, so we use `file_hint` for the finding's `file` (consistent, already
/// project-relative + normalized by the caller). Tolerant: malformed JSON → empty.
pub fn parse_pyright(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let result: PyrightResult = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for diag in result.general_diagnostics {
        let (severity, category) = severity_from_pyright(&diag.severity);
        let safe_message = redact_secrets(&diag.message);
        let title = match &diag.rule {
            Some(rule) if !rule.is_empty() => {
                format!("{}: {}", rule, cap(&safe_message, 200))
            }
            _ => cap(&safe_message, 200),
        };
        let line = diag.range.map(|r| r.start.line + 1);
        out.push(RawFinding {
            file: file_hint.to_string(),
            line,
            severity,
            category,
            source: "pyright".to_string(),
            title,
            body: cap(&safe_message, 1000),
        });
    }
    out
}

/// Run pyright on a single file from the project root.
/// Absent `pyright` → empty. `--` ends flag parsing so a file whose name
/// begins with `-` is never interpreted as an option.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("pyright") {
        return Vec::new();
    }
    let stdout = run_capture(
        "pyright",
        &["--outputjson", &target.file_rel_path],
        root,
    );
    match stdout {
        Some(s) => parse_pyright(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_error_and_warning() {
        let json = r#"{
          "generalDiagnostics": [
            {
              "file": "/abs/src/a.py",
              "severity": "error",
              "message": "Cannot access member 'x' for type 'None'",
              "range": { "start": { "line": 4, "character": 2 } },
              "rule": "reportGeneralTypeIssues"
            },
            {
              "file": "/abs/src/b.py",
              "severity": "warning",
              "message": "Unused variable 'y'",
              "range": { "start": { "line": 10, "character": 0 } },
              "rule": "reportUnusedVariable"
            }
          ]
        }"#;
        let findings = parse_pyright(json, "src/a.py");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "src/a.py");
        assert_eq!(findings[0].line, Some(5));
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].category, Category::Correctness);
        assert!(findings[0].title.starts_with("reportGeneralTypeIssues: "));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn information_severity_is_low() {
        let json = r#"{
          "generalDiagnostics": [
            {
              "file": "/abs/x.py",
              "severity": "information",
              "message": "Info message",
              "range": { "start": { "line": 1, "character": 0 } },
              "rule": ""
            }
          ]
        }"#;
        let findings = parse_pyright(json, "x.py");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[0].title, "Info message");
    }

    #[test]
    fn missing_range_is_handled() {
        let json = r#"{
          "generalDiagnostics": [
            {
              "file": "/abs/x.py",
              "severity": "error",
              "message": "Parsing error",
              "rule": ""
            }
          ]
        }"#;
        let findings = parse_pyright(json, "x.py");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, None);
    }

    #[test]
    fn malformed_yields_empty() {
        assert!(parse_pyright("not json", "x.py").is_empty());
        assert!(parse_pyright("", "x.py").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let json = r#"{
          "generalDiagnostics": [
            {
              "file": "/abs/x.py",
              "severity": "error",
              "message": "Found secret AKIAIOSFODNN7EXAMPLE in code",
              "range": { "start": { "line": 1, "character": 0 } },
              "rule": "reportGeneralTypeIssues"
            }
          ]
        }"#;
        let findings = parse_pyright(json, "x.py");
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
        assert!(f.body.contains("[redacted]"));
    }
}