//! ruff runner (Python linter).
//!
//! `ruff check --output-format json <target>` emits a JSON ARRAY of diagnostics,
//! each `{ "code", "message", "filename", "location": {"row","column"} }`.
//! ruff has no severity field; `severity_from_ruff` maps by rule-code prefix
//! (`S*`→security, `B*`/`F*`/`E9*`→correctness, else style). Granularity is Fine.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_ruff;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunTarget};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

#[derive(Deserialize)]
struct RuffDiagnostic {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    location: Option<RuffLocation>,
}

#[derive(Deserialize)]
struct RuffLocation {
    #[serde(default)]
    row: Option<u32>,
}

/// Parse `ruff check --output-format json` stdout. PURE. `file_hint` (the
/// project-relative path we asked ruff to check) is used for the finding's `file`
/// (ruff reports absolute `filename`). Tolerant: malformed JSON → empty.
pub fn parse_ruff(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let diags: Vec<RuffDiagnostic> = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for d in diags {
        let code = d.code.unwrap_or_default();
        let (severity, category) = severity_from_ruff(&code);
        let file = if file_hint.is_empty() {
            d.filename.replace('\\', "/")
        } else {
            file_hint.to_string()
        };
        let line = d.location.and_then(|l| l.row);
        // PRIVACY: ruff S-rules (flake8-bandit) can interpolate a matched literal
        // into `message`. Redact secret-shaped tokens BEFORE title/body. Redact
        // once, then cap.
        let safe_message = redact_secrets(&d.message);
        let title = if code.is_empty() {
            cap(&safe_message, 200)
        } else {
            format!("{code}: {}", cap(&safe_message, 200))
        };
        out.push(RawFinding {
            file,
            line,
            severity,
            category,
            source: "ruff".to_string(),
            title,
            body: cap(&safe_message, 1000),
        });
    }
    out
}

/// Run ruff on a single file from the project root using the project's
/// ruff.toml/pyproject.toml config. Absent `ruff` → empty. `--` ends flag parsing
/// so a `-`-leading file name is never read as an option.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("ruff") {
        return Vec::new();
    }
    let stdout = run_capture(
        "ruff",
        &[
            "check",
            "--output-format",
            "json",
            "--",
            &target.file_rel_path,
        ],
        root,
    );
    match stdout {
        Some(s) => parse_ruff(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_security_and_style_codes() {
        let json = r#"[
          {"code":"S105","message":"Possible hardcoded password","filename":"/abs/a.py","location":{"row":4,"column":1}},
          {"code":"E501","message":"Line too long","filename":"/abs/a.py","location":{"row":10,"column":80}}
        ]"#;
        let findings = parse_ruff(json, "a.py");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].category, Category::Security);
        assert_eq!(findings[0].line, Some(4));
        assert!(findings[0].title.starts_with("S105: "));
        assert_eq!(findings[1].severity, Severity::Low);
        assert_eq!(findings[1].category, Category::Style);
    }

    #[test]
    fn uses_file_hint_over_absolute_filename() {
        let json = r#"[{"code":"F401","message":"unused import","filename":"/abs/weird/path.py","location":{"row":1,"column":1}}]"#;
        let findings = parse_ruff(json, "pkg/mod.py");
        assert_eq!(findings[0].file, "pkg/mod.py");
        assert_eq!(findings[0].category, Category::Correctness);
    }

    #[test]
    fn malformed_and_empty_yield_empty() {
        assert!(parse_ruff("not json", "a.py").is_empty());
        assert!(parse_ruff("", "a.py").is_empty());
        assert!(parse_ruff("[]", "a.py").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let json = r#"[{"code":"S105","message":"Possible hardcoded password: AKIAIOSFODNN7EXAMPLE","filename":"a.py","location":{"row":4,"column":1}}]"#;
        let findings = parse_ruff(json, "a.py");
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
