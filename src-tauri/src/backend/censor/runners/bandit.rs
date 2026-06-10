//! bandit runner (Python security scanner).
//!
//! `bandit -f json <target>` emits `{ "results": [ { "filename", "line_number",
//! "issue_severity": "HIGH"|"MEDIUM"|"LOW", "issue_text", "test_id", "test_name" } ] }`.
//! Severity/category via `severity_from_bandit` (always Security). Granularity Fine.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_bandit;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunTarget};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

#[derive(Deserialize)]
struct BanditReport {
    #[serde(default)]
    results: Vec<BanditResult>,
}

#[derive(Deserialize)]
struct BanditResult {
    #[serde(default)]
    filename: String,
    #[serde(default)]
    line_number: Option<u32>,
    #[serde(default)]
    issue_severity: String,
    #[serde(default)]
    issue_text: String,
    #[serde(default)]
    test_id: String,
}

/// Parse `bandit -f json` stdout. PURE. `file_hint` is preferred for the finding's
/// `file` (bandit reports the path it was given, but the hint is consistently
/// project-relative + normalized). Tolerant: malformed JSON → empty.
pub fn parse_bandit(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let report: BanditReport = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for r in report.results {
        let (severity, category) = severity_from_bandit(&r.issue_severity);
        let file = if file_hint.is_empty() {
            r.filename.replace('\\', "/")
        } else {
            file_hint.to_string()
        };
        // PRIVACY: bandit interpolates the MATCHED value into `issue_text`
        // (e.g. B105/B106 "Hardcoded password string found: 'AKIA...'"). Redact
        // secret-shaped tokens BEFORE the text reaches title/body (cap only
        // truncates). Redact once, then cap.
        let safe_text = redact_secrets(&r.issue_text);
        let title = if r.test_id.is_empty() {
            cap(&safe_text, 200)
        } else {
            format!("{}: {}", r.test_id, cap(&safe_text, 200))
        };
        out.push(RawFinding {
            file,
            line: r.line_number,
            severity,
            category,
            source: "bandit".to_string(),
            title,
            body: cap(&safe_text, 1000),
        });
    }
    out
}

/// Run bandit on a single file from the project root. Absent `bandit` → empty.
/// `--` ends flag parsing so a `-`-leading file name is never read as an option.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("bandit") {
        return Vec::new();
    }
    let stdout = run_capture("bandit", &["-f", "json", "--", &target.file_rel_path], root);
    match stdout {
        Some(s) => parse_bandit(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_high_severity_result() {
        let json = r#"{
          "results": [
            {"filename":"/abs/a.py","line_number":12,"issue_severity":"HIGH","issue_text":"Use of exec detected.","test_id":"B102","test_name":"exec_used"}
          ]
        }"#;
        let findings = parse_bandit(json, "a.py");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "a.py");
        assert_eq!(f.line, Some(12));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "bandit");
        assert!(f.title.starts_with("B102: "));
    }

    #[test]
    fn maps_medium_low() {
        let json = r#"{"results":[
          {"filename":"a.py","line_number":1,"issue_severity":"MEDIUM","issue_text":"m","test_id":"B1"},
          {"filename":"a.py","line_number":2,"issue_severity":"LOW","issue_text":"l","test_id":"B2"}
        ]}"#;
        let findings = parse_bandit(json, "a.py");
        assert_eq!(findings[0].severity, Severity::Medium);
        assert_eq!(findings[1].severity, Severity::Low);
    }

    #[test]
    fn empty_and_malformed_yield_empty() {
        assert!(parse_bandit(r#"{"results":[]}"#, "a.py").is_empty());
        assert!(parse_bandit("not json", "a.py").is_empty());
        assert!(parse_bandit("", "a.py").is_empty());
    }

    #[test]
    fn redacts_secret_in_issue_text() {
        // B105: bandit embeds the matched secret literal in issue_text.
        let json = r#"{"results":[{"filename":"a.py","line_number":3,"issue_severity":"HIGH","issue_text":"Hardcoded password string found: 'AKIAIOSFODNN7EXAMPLE'","test_id":"B105"}]}"#;
        let findings = parse_bandit(json, "a.py");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(
            !f.title.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret leaked in title: {}",
            f.title
        );
        assert!(
            !f.body.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret leaked in body: {}",
            f.body
        );
        assert!(f.body.contains("[redacted]"));
    }
}
