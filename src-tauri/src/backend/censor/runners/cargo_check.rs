//! cargo check runner.
//!
//! `cargo check --message-format=json` emits the same rustc JSON diagnostic stream
//! as clippy (one object per line). We parse `"reason":"compiler-message"` objects
//! and pull `level` + the PRIMARY span. Severity/category via
//! `severity_from_cargo_check`. Source is "cargo-check".
//!
//! Granularity is Coarse (crate-level compile).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_cargo_check;
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding, RunnerOutcome};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

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

/// Parse `cargo check --message-format=json` stdout into raw findings. PURE.
/// Tolerant: malformed lines / non-compiler-message objects / span-less summaries
/// are skipped without aborting.
pub fn parse_cargo_check(stdout: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: CargoLine = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if parsed.reason != "compiler-message" {
            continue;
        }
        let msg = match parsed.message {
            Some(m) => m,
            None => continue,
        };
        let span = msg
            .spans
            .iter()
            .find(|s| s.is_primary)
            .or_else(|| msg.spans.first());
        let span = match span {
            Some(s) if !s.file_name.is_empty() => s,
            _ => continue,
        };
        let (severity, category) = severity_from_cargo_check(&msg.level);
        let code = msg.code.as_ref().map(|c| c.code.as_str()).unwrap_or("");
        // PRIVACY: a rustc diagnostic can echo a string literal (e.g. a type error
        // mentioning an inline secret). Redact secret-shaped tokens BEFORE
        // title/body. Redact the full message once, then slice/cap.
        let safe_message = redact_secrets(msg.message.trim());
        let first_line = safe_message.lines().next().unwrap_or("").trim();
        let title = if code.is_empty() {
            cap(first_line, 200)
        } else {
            format!("{code}: {}", cap(first_line, 200))
        };
        out.push(RawFinding {
            file: span.file_name.replace('\\', "/"),
            line: Some(span.line_start),
            severity,
            category,
            source: "cargo-check".to_string(),
            title,
            body: cap(&safe_message, 1000),
        });
    }
    out
}

/// Run cargo check from the project root. Absent `cargo` → empty.
pub fn run(root: &Path) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("cargo") {
        return RunnerOutcome::Skipped;
    }
    // A full crate compile is slow on a cold target; allow a generous budget.
    let stdout = run_capture_with_timeout(
        "cargo",
        &["check", "--message-format=json", "--quiet"],
        root,
        Duration::from_secs(300),
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_cargo_check(&s)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_error_diagnostic() {
        let line = r#"{"reason":"compiler-message","message":{"message":"cannot find value `foo`","level":"error","code":{"code":"E0425"},"spans":[{"file_name":"src/main.rs","line_start":7,"is_primary":true}]}}"#;
        let findings = parse_cargo_check(line);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/main.rs");
        assert_eq!(f.line, Some(7));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Correctness);
        assert_eq!(f.source, "cargo-check");
        assert!(f.title.starts_with("E0425: "));
    }

    #[test]
    fn parses_warning_as_medium() {
        let line = r#"{"reason":"compiler-message","message":{"message":"unused import","level":"warning","spans":[{"file_name":"src/lib.rs","line_start":1,"is_primary":true}]}}"#;
        let findings = parse_cargo_check(line);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Medium);
    }

    #[test]
    fn skips_summary_and_malformed() {
        let lines = "garbage\n{\"reason\":\"build-finished\",\"success\":false}\n";
        assert!(parse_cargo_check(lines).is_empty());
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_cargo_check("").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let line = r#"{"reason":"compiler-message","message":{"message":"mismatched types: expected &str, found AKIAIOSFODNN7EXAMPLE","level":"error","spans":[{"file_name":"src/main.rs","line_start":7,"is_primary":true}]}}"#;
        let findings = parse_cargo_check(line);
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
}
