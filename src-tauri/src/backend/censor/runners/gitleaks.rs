//! gitleaks runner (secret scanner). HIGH PRIVACY RISK.
//!
//! gitleaks JSON output is an ARRAY of findings; each carries the ACTUAL secret in
//! `Secret`, the matched substring in `Match`, and the full source line in `Line`.
//! NONE of those fields may ever reach a `RawFinding` — we extract ONLY structured
//! metadata (`RuleID`, `Description`, `File`, `StartLine`) and build a redacted
//! title/body. The secret/match/line fields are read into `serde_json::Value`'s
//! ignored-by-omission void: we simply do not declare them, so serde drops them.
//!
//! Severity/category via `gitleaks_category` (High Security). Granularity is Fine.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::gitleaks_category;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// ONLY the non-secret structured fields are declared. `Secret`, `Match`, and
/// `Line` are deliberately NOT fields here, so serde discards them — they never
/// enter the process's owned data, let alone a shard.
#[derive(Deserialize)]
struct GitleaksFinding {
    #[serde(default, rename = "RuleID")]
    rule_id: String,
    #[serde(default, rename = "Description")]
    description: String,
    #[serde(default, rename = "File")]
    file: String,
    #[serde(default, rename = "StartLine")]
    start_line: Option<u32>,
}

/// Parse gitleaks JSON stdout. PURE. Builds findings from RuleID/Description/File/
/// StartLine ONLY. The secret value (`Secret`/`Match`/`Line`) is NEVER read into
/// the title or body — `title = "Secret detected: <ruleID>"`,
/// `body = "<ruleID> at <file>:<line>"`. Tolerant: malformed JSON → empty.
pub fn parse_gitleaks(stdout: &str) -> Vec<RawFinding> {
    let findings: Vec<GitleaksFinding> = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let (severity, category) = gitleaks_category();
    findings
        .into_iter()
        .filter_map(|gf| {
            // Identify the rule; fall back to a generic label if absent. Never use
            // the secret/match value for identification.
            let rule = if gf.rule_id.is_empty() {
                "unknown-rule".to_string()
            } else {
                gf.rule_id.clone()
            };
            let file = gf.file.replace('\\', "/");
            if file.is_empty() {
                return None;
            }
            // Description is gitleaks' rule description (e.g. "AWS Access Key"),
            // NOT the matched value — safe to include. Still cap defensively.
            let rule_label = sanitize_rule_label(&rule);
            let line_token = match gf.start_line {
                Some(n) => n.to_string(),
                None => "?".to_string(),
            };
            let title = format!("Secret detected: {rule_label}");
            let body = if gf.description.is_empty() {
                format!("{rule_label} at {file}:{line_token}")
            } else {
                // PRIVACY: gitleaks' `Description` is normally a static rule label
                // ("AWS Access Key"), but a custom rule can interpolate the matched
                // value into it. Redact secret-shaped tokens before it reaches the
                // body (redact first, then cap).
                format!(
                    "{} ({rule_label}) at {file}:{line_token}",
                    cap(&redact_secrets(&gf.description), 160)
                )
            };
            Some(RawFinding {
                file,
                line: gf.start_line,
                severity,
                category,
                source: "gitleaks".to_string(),
                title,
                body,
            })
        })
        .collect()
}

/// Defensive: a RuleID is normally a short slug, but cap it so a hostile/odd rule
/// id can't bloat a title.
fn sanitize_rule_label(rule: &str) -> String {
    cap(rule.trim(), 80)
}

/// Run gitleaks on the project root (it scans the working tree / git history). We
/// report to stdout in JSON. Absent `gitleaks` → empty. gitleaks exits non-zero
/// when leaks are found — that is parsed normally by `run_capture`.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("gitleaks") {
        return Vec::new();
    }
    // `detect --report-format json --report-path -` streams JSON to stdout (`-`).
    // `--no-banner` keeps stdout clean. We run from the project root.
    let stdout = run_capture(
        "gitleaks",
        &[
            "detect",
            "--no-banner",
            "--report-format",
            "json",
            "--report-path",
            "-",
        ],
        root,
    );
    match stdout {
        Some(s) => parse_gitleaks(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    /// SECURITY: a synthetic gitleaks finding carrying a fake secret must NEVER
    /// surface that value (or the matched line) in the title or body.
    #[test]
    fn redacts_secret_match_and_line() {
        let secret = "AKIAIOSFODNN7EXAMPLE";
        // The matched line as gitleaks would emit it (quotes escaped for valid JSON).
        let matched = "aws_key = AKIAIOSFODNN7EXAMPLE-token";
        let json = format!(
            r#"[
              {{
                "RuleID":"aws-access-token",
                "Description":"AWS Access Key",
                "File":"src/config.py",
                "StartLine":42,
                "Secret":"{secret}",
                "Match":"{matched}",
                "Line":"{matched}"
              }}
            ]"#
        );
        let findings = parse_gitleaks(&json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/config.py");
        assert_eq!(f.line, Some(42));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "gitleaks");
        // The secret value and the matched line must appear NOWHERE.
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
        assert!(
            !f.title.contains(matched),
            "match leaked into title: {}",
            f.title
        );
        assert!(
            !f.body.contains(matched),
            "match leaked into body: {}",
            f.body
        );
        // The structured rule id / description are present.
        assert!(f.title.contains("aws-access-token"));
        assert!(f.body.contains("AWS Access Key"));
        assert!(f.body.contains("src/config.py:42"));
    }

    /// PRIVACY: a custom gitleaks rule that interpolates the matched value into
    /// `Description` must not leak that value into the finding body.
    #[test]
    fn redacts_secret_in_description() {
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let json = format!(
            r#"[{{"RuleID":"custom","Description":"Found key {secret}","File":"a.py","StartLine":1}}]"#
        );
        let findings = parse_gitleaks(&json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(
            !f.body.contains(secret),
            "secret leaked into body: {}",
            f.body
        );
        assert!(f.body.contains("[redacted]"));
    }

    #[test]
    fn handles_missing_rule_and_line() {
        let json = r#"[{"File":"a.ts","Secret":"deadbeef"}]"#;
        let findings = parse_gitleaks(json);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "a.ts");
        assert_eq!(findings[0].line, None);
        assert!(!findings[0].body.contains("deadbeef"));
        assert!(findings[0].title.contains("unknown-rule"));
    }

    #[test]
    fn empty_and_malformed_yield_empty() {
        assert!(parse_gitleaks("[]").is_empty());
        assert!(parse_gitleaks("not json").is_empty());
        assert!(parse_gitleaks("").is_empty());
    }
}
