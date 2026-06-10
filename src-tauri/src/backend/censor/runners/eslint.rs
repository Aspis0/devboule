//! eslint runner.
//!
//! `eslint --format json <target>` emits a JSON ARRAY of file results, each with a
//! `filePath` and a `messages` array (`ruleId`, numeric `severity` 1|2, `line`,
//! `message`). Severity/category via `severity_from_eslint` (stringified integer).
//! Granularity is Fine (per-file).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_eslint;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunTarget};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

#[derive(Deserialize)]
struct EslintFileResult {
    #[serde(default, rename = "filePath")]
    file_path: String,
    #[serde(default)]
    messages: Vec<EslintMessage>,
}

#[derive(Deserialize)]
struct EslintMessage {
    #[serde(default, rename = "ruleId")]
    rule_id: Option<String>,
    #[serde(default)]
    severity: i64,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    message: String,
}

/// Parse `eslint --format json` stdout. PURE. `file_hint` is the project-relative
/// path of the file we asked eslint to lint; eslint reports absolute paths in
/// `filePath`, so we use `file_hint` for the finding's `file` (consistent,
/// already project-relative + normalized by the caller). Tolerant: malformed JSON
/// → empty; a message missing a line is still kept as a file-level finding.
pub fn parse_eslint(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let results: Vec<EslintFileResult> = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for file_result in results {
        // Prefer the hint (project-relative); fall back to eslint's path only if
        // the hint is empty.
        let file = if file_hint.is_empty() {
            file_result.file_path.replace('\\', "/")
        } else {
            file_hint.to_string()
        };
        for m in file_result.messages {
            let (severity, category) = severity_from_eslint(&m.severity.to_string());
            let rule = m.rule_id.unwrap_or_default();
            // PRIVACY: eslint security plugins can interpolate a matched literal
            // into `message`. Redact secret-shaped tokens BEFORE title/body (cap
            // only truncates). Redact once, then cap.
            let safe_message = redact_secrets(&m.message);
            let title = if rule.is_empty() {
                cap(&safe_message, 200)
            } else {
                format!("{rule}: {}", cap(&safe_message, 200))
            };
            out.push(RawFinding {
                file: file.clone(),
                line: m.line,
                severity,
                category,
                source: "eslint".to_string(),
                title,
                body: cap(&safe_message, 1000),
            });
        }
    }
    out
}

/// Run eslint on a single file from the project root, using the project's eslint
/// config. Absent `eslint` → empty. `--` ends flag parsing so a file whose name
/// begins with `-` is never interpreted as an option.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("eslint") {
        return Vec::new();
    }
    let stdout = run_capture(
        "eslint",
        &["--format", "json", "--", &target.file_rel_path],
        root,
    );
    match stdout {
        Some(s) => parse_eslint(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_error_and_warning() {
        let json = r#"[
          {
            "filePath": "/abs/src/a.ts",
            "messages": [
              {"ruleId":"no-unused-vars","severity":2,"line":3,"column":7,"message":"'x' is defined but never used."},
              {"ruleId":"eqeqeq","severity":1,"line":10,"column":1,"message":"Expected '===' and instead saw '=='."}
            ]
          }
        ]"#;
        let findings = parse_eslint(json, "src/a.ts");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "src/a.ts");
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].category, Category::Correctness);
        assert!(findings[0].title.starts_with("no-unused-vars: "));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn null_rule_id_is_handled() {
        let json = r#"[{"filePath":"/x.ts","messages":[{"ruleId":null,"severity":2,"line":1,"message":"Parsing error"}]}]"#;
        let findings = parse_eslint(json, "x.ts");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Parsing error");
    }

    #[test]
    fn empty_messages_yields_empty() {
        let json = r#"[{"filePath":"/x.ts","messages":[]}]"#;
        assert!(parse_eslint(json, "x.ts").is_empty());
    }

    #[test]
    fn malformed_yields_empty() {
        assert!(parse_eslint("not json", "x.ts").is_empty());
        assert!(parse_eslint("", "x.ts").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let json = r#"[{"filePath":"/x.ts","messages":[{"ruleId":"no-secrets/no-secrets","severity":2,"line":1,"message":"Found secret AKIAIOSFODNN7EXAMPLE in code"}]}]"#;
        let findings = parse_eslint(json, "x.ts");
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
