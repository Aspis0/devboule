//! semgrep runner (pattern-based static analysis). PRIVACY-SENSITIVE.
//!
//! `semgrep --json` emits `{ "results": [ { "check_id", "path", "start":{"line"},
//! "extra": { "message", "severity": "ERROR"|"WARNING"|"INFO", "lines": "<source>" } } ] }`.
//! The `extra.lines` field is the MATCHED SOURCE SNIPPET, which can contain a
//! secret or sensitive code — it is deliberately NOT declared, so serde drops it.
//! We keep only `check_id`, `path`, `start.line`, `extra.message`, `extra.severity`.
//! Severity/category via `severity_from_semgrep`. Granularity is Fine.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_semgrep;
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding, RunTarget};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

#[derive(Deserialize)]
struct SemgrepReport {
    #[serde(default)]
    results: Vec<SemgrepResult>,
}

#[derive(Deserialize)]
struct SemgrepResult {
    #[serde(default)]
    check_id: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    start: Option<SemgrepPos>,
    #[serde(default)]
    extra: Option<SemgrepExtra>,
}

#[derive(Deserialize)]
struct SemgrepPos {
    #[serde(default)]
    line: Option<u32>,
}

/// Only message + severity are declared. `lines` (matched source) and any
/// `metavars` (which can capture secret substrings) are NOT declared → dropped.
#[derive(Deserialize)]
struct SemgrepExtra {
    #[serde(default)]
    message: String,
    #[serde(default)]
    severity: String,
}

/// Parse `semgrep --json` stdout. PURE. The matched source snippet (`extra.lines`)
/// is never read. Title is `<check_id>: <message>`; body is the rule message +
/// location — never the matched code. `file_hint` is preferred over semgrep's
/// `path`. Tolerant: malformed JSON → empty.
pub fn parse_semgrep(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let report: SemgrepReport = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for r in report.results {
        let (message, sev_str) = match r.extra {
            Some(e) => (e.message, e.severity),
            None => (String::new(), String::new()),
        };
        let (severity, category) = severity_from_semgrep(&sev_str);
        let file = if file_hint.is_empty() {
            r.path.replace('\\', "/")
        } else {
            file_hint.to_string()
        };
        if file.is_empty() {
            continue;
        }
        let line = r.start.and_then(|s| s.line);
        // check_id is the rule identifier (e.g. "python.lang.security.audit.eval");
        // safe structured metadata, capped defensively.
        let rule = cap(r.check_id.trim(), 120);
        // PRIVACY: semgrep interpolates the MATCHED value into `extra.message`
        // (`$METAVAR` expansion), so the message can embed a secret. Redact secret-
        // shaped tokens BEFORE the message reaches title/body (the cap only
        // truncates). Redact first, then cap.
        let safe_message = cap(redact_secrets(message.trim()).trim(), 300);
        let title = if rule.is_empty() {
            if safe_message.is_empty() {
                "semgrep finding".to_string()
            } else {
                safe_message.clone()
            }
        } else {
            format!("{rule}: {safe_message}")
        };
        out.push(RawFinding {
            file,
            line,
            severity,
            category,
            source: "semgrep".to_string(),
            title,
            body: if safe_message.is_empty() {
                format!("Rule {rule} matched")
            } else {
                safe_message
            },
        });
    }
    out
}

/// Run semgrep on a single file from the project root using the project's semgrep
/// config (`--config auto` if none is present is intentionally NOT forced — we let
/// the project drive; if the project has no config semgrep emits no results).
/// Absent `semgrep` → empty.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("semgrep") {
        return Vec::new();
    }
    // `--` ends flag parsing so a `-`-leading file name is never read as an
    // option. semgrep rule evaluation can be slow; allow a generous budget.
    let stdout = run_capture_with_timeout(
        "semgrep",
        // PINNED 2026-06-12 (master plan P2): broad/auto rulesets are ~42% FP and
        // `--config auto` phones home. Pin the small curated `p/ci` pack and keep
        // metrics off (privacy + determinism).
        &[
            "--json",
            "--quiet",
            "--config",
            "p/ci",
            "--metrics",
            "off",
            "--",
            &target.file_rel_path,
        ],
        root,
        Duration::from_secs(300),
    );
    match stdout {
        Some(s) => parse_semgrep(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    /// SECURITY: the matched source line must never leak into the finding.
    #[test]
    fn drops_matched_source_lines() {
        let secret_line = "password = 'AKIAIOSFODNN7EXAMPLE'";
        let json = format!(
            r#"{{
              "results": [
                {{
                  "check_id":"python.lang.security.audit.hardcoded-password",
                  "path":"app.py",
                  "start":{{"line":7}},
                  "end":{{"line":7}},
                  "extra":{{
                    "message":"Hardcoded password detected",
                    "severity":"ERROR",
                    "lines":"{secret_line}"
                  }}
                }}
              ]
            }}"#
        );
        let findings = parse_semgrep(&json, "app.py");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "app.py");
        assert_eq!(f.line, Some(7));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "semgrep");
        // The matched source (and its secret) must not appear.
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!f.body.contains(secret_line));
        // Structured rule id + message are present.
        assert!(f.title.contains("hardcoded-password"));
        assert!(f.body.contains("Hardcoded password detected"));
    }

    /// SECURITY (WARNING 1): semgrep interpolates the matched value into
    /// `extra.message`. A message embedding a secret must NOT surface that secret
    /// in either title or body.
    #[test]
    fn redacts_secret_interpolated_into_message() {
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let json = format!(
            r#"{{"results":[
              {{"check_id":"generic.secrets.aws-key","path":"app.py","start":{{"line":3}},
                "extra":{{"message":"Key found: {secret}","severity":"ERROR"}}}}
            ]}}"#
        );
        let findings = parse_semgrep(&json, "app.py");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(
            !f.title.contains(secret),
            "secret leaked into title: {}",
            f.title
        );
        assert!(
            !f.body.contains(secret),
            "secret leaked into body: {}",
            f.body
        );
        // The rule id and the surrounding prose survive.
        assert!(f.title.contains("aws-key"));
        assert!(f.body.contains("Key found"));
        assert!(f.body.contains("[redacted]"));
    }

    #[test]
    fn maps_warning_and_info() {
        let json = r#"{"results":[
          {"check_id":"r1","path":"a.py","start":{"line":1},"extra":{"message":"m","severity":"WARNING"}},
          {"check_id":"r2","path":"a.py","start":{"line":2},"extra":{"message":"n","severity":"INFO"}}
        ]}"#;
        let findings = parse_semgrep(json, "a.py");
        assert_eq!(findings[0].severity, Severity::Medium);
        assert_eq!(findings[1].severity, Severity::Low);
    }

    #[test]
    fn empty_and_malformed_yield_empty() {
        assert!(parse_semgrep(r#"{"results":[]}"#, "a.py").is_empty());
        assert!(parse_semgrep("not json", "a.py").is_empty());
        assert!(parse_semgrep("", "a.py").is_empty());
    }
}
