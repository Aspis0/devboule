//! stylelint runner (CSS/SCSS/Sass/Less linter).
//!
//! `stylelint` is a STANDALONE stylesheet linter (a Node entry-point binary), cheap
//! enough to run per-file in the FINE (per-keystroke-settled) loop. We invoke it with the
//! machine JSON reporter:
//!
//! ```text
//! stylelint --formatter json <file>
//! ```
//!
//! The `--formatter json` reporter writes an ARRAY of per-source objects to STDOUT:
//!
//! ```json
//! [ { "source": "a.css",
//!     "warnings": [ { "line": 1, "column": 1, "rule": "block-no-empty",
//!                     "severity": "error", "text": "Unexpected empty block" } ] } ]
//! ```
//!
//! Parsed DEFENSIVELY with serde_json (tolerate schema variance — unknown fields ignored,
//! every field optional/defaulted, never panic). Advisory: `error` → Medium (capped, never
//! High) / Correctness, `warning` → Low / Style (see [`severity_from_stylelint`]). Absent
//! `stylelint` → empty Vec (never an error).
//!
//! CONFIG NOTE: stylelint REQUIRES a configuration (it has no built-in default ruleset). On
//! a project with NO stylelint config, the tool ERRORS OUT before producing any JSON — our
//! defensive parse then yields an empty Vec (graceful, expected). So stylelint only
//! surfaces findings on projects that have actually configured it; for everyone else it is
//! silently a no-op, exactly the advisory-first posture we want.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_stylelint;
use super::DEFAULT_RUNNER_TIMEOUT;
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding, RunTarget};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    // Single-binary, no-compile linter → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// One per-source entry in stylelint's `--formatter json` array. Defensive: every field is
/// optional/defaulted and unknown fields are ignored, so a schema drift across stylelint
/// versions never breaks the parse.
#[derive(Deserialize, Default)]
struct StylelintSource {
    #[serde(default)]
    source: String,
    #[serde(default)]
    warnings: Vec<StylelintWarning>,
}

/// One warning. `line`/`column` are integers; `rule` is the rule id (e.g.
/// `block-no-empty`); `text` is the human message; `severity` is `error`/`warning`. All
/// optional/defaulted.
#[derive(Deserialize, Default)]
struct StylelintWarning {
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    rule: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    text: String,
}

/// Parse `stylelint --formatter json` stdout. PURE. Tolerant: malformed/empty JSON → empty
/// Vec (never a panic). stylelint may prepend non-JSON chatter to stdout, so a failed
/// whole-string parse is retried from the first `[` (the JSON array start). The per-finding
/// `file` prefers `file_hint` (consistently project-relative + normalized); the JSON
/// `source` is the fallback when the hint is empty. The `severity` is mapped via
/// [`severity_from_stylelint`] (advisory: capped at Medium).
///
/// PRIVACY: a stylelint `text` can interpolate a literal/value from the stylesheet; the
/// message is run through `redact_secrets` before it lands in title/body.
pub fn parse_stylelint(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let sources = match parse_json_sources(stdout) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for src in sources {
        let resolved_file = if file_hint.is_empty() {
            src.source.replace('\\', "/")
        } else {
            file_hint.replace('\\', "/")
        };
        if resolved_file.is_empty() {
            continue;
        }
        for w in src.warnings {
            // Skip a wholly-empty warning (no rule AND no text → nothing to show).
            if w.rule.is_empty() && w.text.is_empty() {
                continue;
            }
            let (severity, category) = severity_from_stylelint(&w.severity);
            let safe_text = redact_secrets(&w.text);
            // Prefix the rule id when present (e.g. "block-no-empty: Unexpected empty block").
            let titled = if w.rule.is_empty() {
                cap(&safe_text, 200)
            } else {
                format!("{}: {}", w.rule, cap(&safe_text, 200))
            };
            // stylelint emits a positive line number; treat 0/absent defensively as no line.
            let line_field = w.line.filter(|n| *n != 0);
            out.push(RawFinding {
                file: resolved_file.clone(),
                line: line_field,
                severity,
                category,
                source: "stylelint".to_string(),
                title: format!("stylelint: {titled}"),
                body: cap(&safe_text, 1000),
            });
        }
    }
    out
}

/// Deserialize the stylelint source array from `stdout`, tolerating a non-JSON prefix.
/// Tries a whole-string parse first, then retries from the first `[`. `None` if neither
/// yields a valid array.
fn parse_json_sources(stdout: &str) -> Option<Vec<StylelintSource>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(sources) = serde_json::from_str::<Vec<StylelintSource>>(trimmed) {
        return Some(sources);
    }
    // Retry from the first '[' — drops any leading log/warning chatter.
    let start = trimmed.find('[')?;
    serde_json::from_str::<Vec<StylelintSource>>(&trimmed[start..]).ok()
}

/// Run stylelint on a single file from the project root. Absent `stylelint` → empty
/// (never an error). The JSON report goes to STDOUT, so we capture stdout with the
/// default per-file timeout. The file path is the orchestrator-validated project-relative
/// path (a leading-`-` component is rejected upstream by `validate_rel_path`, so it can't
/// be mistaken for a flag). A project with no stylelint config makes stylelint error out
/// with no JSON → empty Vec (see the CONFIG NOTE in the module header).
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("stylelint") {
        return Vec::new();
    }
    let stdout = run_capture_with_timeout(
        "stylelint",
        &["--formatter", "json", &target.file_rel_path],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stdout {
        Some(s) => parse_stylelint(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_warnings_from_json_array() {
        // Captured-sample `stylelint --formatter json` output shape.
        let json = r#"[
          {
            "source": "a.css",
            "warnings": [
              {"line": 1, "column": 1, "rule": "block-no-empty", "severity": "error", "text": "Unexpected empty block"},
              {"line": 4, "column": 3, "rule": "color-no-invalid-hex", "severity": "warning", "text": "Unexpected invalid hex color"}
            ]
          }
        ]"#;
        let findings = parse_stylelint(json, "a.css");
        assert_eq!(findings.len(), 2, "findings: {findings:?}");

        let e = &findings[0];
        assert_eq!(e.file, "a.css");
        assert_eq!(e.line, Some(1));
        // error → advisory Medium (never High), Correctness.
        assert_eq!(e.severity, Severity::Medium);
        assert_eq!(e.category, Category::Correctness);
        assert_eq!(e.source, "stylelint");
        assert!(e.title.starts_with("stylelint: block-no-empty: "));
        assert!(e.body.contains("Unexpected empty block"));

        let w = &findings[1];
        assert_eq!(w.line, Some(4));
        // warning → advisory Low, Style.
        assert_eq!(w.severity, Severity::Low);
        assert_eq!(w.category, Category::Style);
        assert!(w.title.contains("color-no-invalid-hex"));
    }

    #[test]
    fn file_hint_overrides_json_source() {
        // The orchestrator-supplied hint is the canonical project-relative path; it wins
        // over stylelint's reported (possibly absolute) source.
        let json = r#"[{"source":"/abs/tmp/x.css","warnings":[{"line":2,"rule":"indentation","severity":"warning","text":"bad indent"}]}]"#;
        let findings = parse_stylelint(json, "styles/x.scss");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "styles/x.scss");
    }

    #[test]
    fn tolerates_prefix_before_json() {
        // A non-JSON line can precede the array on stdout; retry from the first '['.
        let stdout = "Deprecation warning\n[{\"source\":\"a.css\",\"warnings\":[{\"line\":1,\"rule\":\"block-no-empty\",\"severity\":\"error\",\"text\":\"empty\"}]}]\n";
        let findings = parse_stylelint(stdout, "a.css");
        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert_eq!(findings[0].line, Some(1));
        assert_eq!(findings[0].severity, Severity::Medium);
    }

    #[test]
    fn tolerates_missing_and_extra_fields() {
        // No `rule`, no `line`, unknown severity, plus an UNKNOWN field — defensive parse
        // keeps the text-only finding (line None, severity defaults Low/Style) and ignores
        // the extra field.
        let json = r#"[{"source":"a.css","warnings":[{"text":"some issue","node":"unknown"}]}]"#;
        let findings = parse_stylelint(json, "a.css");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, None);
        // Unknown/empty severity → Low/Style.
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[0].category, Category::Style);
        // No rule → title is just the text (no `rule: ` prefix beyond `stylelint: `).
        assert!(findings[0].title.starts_with("stylelint: some issue"));
    }

    #[test]
    fn empty_warnings_and_no_sources_yield_no_findings() {
        assert!(parse_stylelint(r#"[]"#, "a.css").is_empty());
        assert!(parse_stylelint(r#"[{"source":"a.css","warnings":[]}]"#, "a.css").is_empty());
    }

    #[test]
    fn malformed_and_empty_yield_empty_no_panic() {
        // A project with no config makes stylelint error out with no JSON → empty.
        assert!(parse_stylelint("not json", "a.css").is_empty());
        assert!(parse_stylelint("", "a.css").is_empty());
        assert!(parse_stylelint("\n\n", "a.css").is_empty());
        // An object (not an array) is not the expected shape → empty, no panic.
        assert!(parse_stylelint(r#"{"warnings":[]}"#, "a.css").is_empty());
    }

    #[test]
    fn redacts_secret_in_text() {
        let json = r#"[{"source":"a.css","warnings":[{"line":1,"rule":"x","severity":"error","text":"leaked token AKIAIOSFODNN7EXAMPLE here"}]}]"#;
        let findings = parse_stylelint(json, "a.css");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when stylelint absent; with stylelint present
    //      but NO project config, stylelint errors out → empty Vec (graceful). We assert
    //      the absent-tool path and the no-panic present path. ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_does_not_panic() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent stylelint → empty Vec, no error (graceful absence). When stylelint IS
        // present but the temp project has NO config, stylelint errors out and we get an
        // empty Vec — the point is it never panics and every finding (if any) is advisory.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aspis-stylelint-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rel = "a.css";
        std::fs::write(dir.join(rel), "a {}\n").unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target);
        if crate::backend::projects::command_exists("stylelint") {
            // No config → stylelint errors → empty (expected). If a global config DID make
            // it produce findings, they must be advisory (never High) and correctly sourced.
            for f in &findings {
                assert_eq!(f.source, "stylelint");
                assert_ne!(f.severity, Severity::High);
            }
        } else {
            assert!(findings.is_empty(), "absent stylelint must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
