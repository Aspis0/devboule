//! sqlfluff runner (SQL linter/formatter).
//!
//! `sqlfluff` is a STANDALONE SQL linter (a single Python entry-point binary — no
//! compile, no toolchain bootstrap), cheap enough to run per-file in the FINE
//! (per-keystroke-settled) loop. We invoke it in lint mode with a fixed dialect and the
//! machine JSON reporter:
//!
//! ```text
//! sqlfluff lint --dialect ansi --format json <file>
//! ```
//!
//! `--dialect ansi` is the safe, universal default (sqlfluff REQUIRES a dialect and has
//! no auto-detect for a single file outside a project config); `ansi` parses the broadest
//! common SQL. The `--format json` reporter writes an ARRAY of per-file objects to STDOUT:
//!
//! ```json
//! [ { "filepath": "a.sql",
//!     "violations": [ { "line_no": 1, "line_pos": 1, "code": "LT01",
//!                       "description": "Expected single whitespace." } ] } ]
//! ```
//!
//! Parsed DEFENSIVELY with serde_json (tolerate schema variance — unknown fields ignored,
//! every field optional/defaulted, never panic). sqlfluff can prepend non-JSON `WARNING`
//! lines to stdout (sqlfluff#850), so if a whole-string parse fails we retry from the
//! first `[`. Advisory: sqlfluff is predominantly a STYLE/format tool, so every violation
//! is Style/Low (see [`severity_from_sqlfluff`]). Absent `sqlfluff` → empty Vec (never an
//! error).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_sqlfluff;
use super::DEFAULT_RUNNER_TIMEOUT;
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding, RunnerOutcome, RunTarget};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    // Single-binary, no-compile linter → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// One per-file entry in sqlfluff's `--format json` array. Defensive: every field is
/// optional/defaulted and unknown fields are ignored, so a schema drift across sqlfluff
/// versions never breaks the parse.
#[derive(Deserialize, Default)]
struct SqlfluffFile {
    #[serde(default)]
    filepath: String,
    #[serde(default)]
    violations: Vec<SqlfluffViolation>,
}

/// One violation. `line_no`/`line_pos` are integers; `code` is the rule id (e.g.
/// `LT01`); `description` is the human message. All optional/defaulted.
#[derive(Deserialize, Default)]
struct SqlfluffViolation {
    #[serde(default)]
    line_no: Option<u32>,
    #[serde(default)]
    code: String,
    #[serde(default)]
    description: String,
}

/// Parse `sqlfluff lint --format json` stdout. PURE. Tolerant: malformed/empty JSON →
/// empty Vec (never a panic). sqlfluff may prepend non-JSON `WARNING` lines to stdout, so
/// a failed whole-string parse is retried from the first `[` (the JSON array start). The
/// per-finding `file` prefers `file_hint` (consistently project-relative + normalized);
/// the JSON `filepath` is the fallback when the hint is empty. Every violation is
/// Style/Low (advisory — see [`severity_from_sqlfluff`]).
///
/// PRIVACY: a sqlfluff `description` can interpolate a literal/identifier from the source;
/// the message is run through `redact_secrets` before it lands in title/body.
pub fn parse_sqlfluff(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let files = match parse_json_files(stdout) {
        Some(f) => f,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for file in files {
        let resolved_file = if file_hint.is_empty() {
            file.filepath.replace('\\', "/")
        } else {
            file_hint.replace('\\', "/")
        };
        if resolved_file.is_empty() {
            continue;
        }
        for v in file.violations {
            // Skip a wholly-empty violation (no code AND no description → nothing to show).
            if v.code.is_empty() && v.description.is_empty() {
                continue;
            }
            let (severity, category) = severity_from_sqlfluff();
            let safe_desc = redact_secrets(&v.description);
            // Prefix the rule code when present (e.g. "LT01: Expected single whitespace.").
            let titled = if v.code.is_empty() {
                cap(&safe_desc, 200)
            } else {
                format!("{}: {}", v.code, cap(&safe_desc, 200))
            };
            // sqlfluff emits a positive line number; treat 0/absent defensively as no line.
            let line_field = v.line_no.filter(|n| *n != 0);
            out.push(RawFinding {
                file: resolved_file.clone(),
                line: line_field,
                severity,
                category,
                source: "sqlfluff".to_string(),
                title: format!("sqlfluff: {titled}"),
                body: cap(&safe_desc, 1000),
            });
        }
    }
    out
}

/// Deserialize the sqlfluff file array from `stdout`, tolerating a non-JSON prefix
/// (sqlfluff#850: `WARNING` lines can precede the JSON). Tries a whole-string parse
/// first, then retries from the first `[`. `None` if neither yields a valid array.
fn parse_json_files(stdout: &str) -> Option<Vec<SqlfluffFile>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(files) = serde_json::from_str::<Vec<SqlfluffFile>>(trimmed) {
        return Some(files);
    }
    // Retry from the first '[' — drops any leading WARNING/log chatter.
    let start = trimmed.find('[')?;
    serde_json::from_str::<Vec<SqlfluffFile>>(&trimmed[start..]).ok()
}

/// Run sqlfluff on a single file from the project root. Absent `sqlfluff` → empty
/// (never an error). The JSON report goes to STDOUT, so we capture stdout with the
/// default per-file timeout. `--dialect ansi` is the safe default dialect (sqlfluff
/// requires one). The file path is the orchestrator-validated project-relative path (a
/// leading-`-` component is rejected upstream by `validate_rel_path`, so it can't be
/// mistaken for a flag).
pub fn run(root: &Path, target: &RunTarget) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("sqlfluff") {
        return RunnerOutcome::Skipped;
    }
    let stdout = run_capture_with_timeout(
        "sqlfluff",
        &[
            "lint",
            "--dialect",
            "ansi",
            "--format",
            "json",
            &target.file_rel_path,
        ],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_sqlfluff(&s, &target.file_rel_path)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_violations_from_json_array() {
        // Captured-sample `sqlfluff lint --format json` output shape.
        let json = r#"[
          {
            "filepath": "schema.sql",
            "violations": [
              {"line_no": 1, "line_pos": 1, "code": "LT01", "description": "Expected single whitespace."},
              {"line_no": 3, "line_pos": 5, "code": "CP01", "description": "Keywords must be consistently upper case."}
            ]
          }
        ]"#;
        let findings = parse_sqlfluff(json, "schema.sql");
        assert_eq!(findings.len(), 2, "findings: {findings:?}");

        let a = &findings[0];
        assert_eq!(a.file, "schema.sql");
        assert_eq!(a.line, Some(1));
        // sqlfluff is style/format → Low/Style advisory.
        assert_eq!(a.severity, Severity::Low);
        assert_eq!(a.category, Category::Style);
        assert_eq!(a.source, "sqlfluff");
        assert!(a.title.starts_with("sqlfluff: LT01: "));
        assert!(a.body.contains("Expected single whitespace"));

        let b = &findings[1];
        assert_eq!(b.line, Some(3));
        assert!(b.title.contains("CP01"));
        assert!(b.body.contains("consistently upper case"));
    }

    #[test]
    fn file_hint_overrides_json_filepath() {
        // The orchestrator-supplied hint is the canonical project-relative path; it wins
        // over sqlfluff's reported (possibly absolute) filepath.
        let json = r#"[{"filepath":"/abs/tmp/x.sql","violations":[{"line_no":2,"code":"LT02","description":"indent"}]}]"#;
        let findings = parse_sqlfluff(json, "db/x.sql");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "db/x.sql");
    }

    #[test]
    fn tolerates_warning_prefix_before_json() {
        // sqlfluff#850: a WARNING line can precede the JSON on stdout. We retry from
        // the first '[' so the array still parses.
        let stdout = "WARNING  Parse error in template.\n[{\"filepath\":\"a.sql\",\"violations\":[{\"line_no\":1,\"code\":\"LT01\",\"description\":\"ws\"}]}]\n";
        let findings = parse_sqlfluff(stdout, "a.sql");
        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert_eq!(findings[0].line, Some(1));
        assert_eq!(findings[0].category, Category::Style);
    }

    #[test]
    fn tolerates_missing_and_extra_fields() {
        // No `code`, no `line_no`, plus an UNKNOWN field — defensive parse keeps the
        // description-only finding (line None) and ignores the extra field.
        let json = r#"[{"filepath":"a.sql","violations":[{"description":"some issue","name":"unknown_extra"}]}]"#;
        let findings = parse_sqlfluff(json, "a.sql");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, None);
        // No code → title is just the description (no `CODE: ` prefix beyond `sqlfluff: `).
        assert!(findings[0].title.starts_with("sqlfluff: some issue"));
    }

    #[test]
    fn empty_violations_and_no_files_yield_no_findings() {
        assert!(parse_sqlfluff(r#"[]"#, "a.sql").is_empty());
        assert!(parse_sqlfluff(r#"[{"filepath":"a.sql","violations":[]}]"#, "a.sql").is_empty());
    }

    #[test]
    fn malformed_and_empty_yield_empty_no_panic() {
        assert!(parse_sqlfluff("not json", "a.sql").is_empty());
        assert!(parse_sqlfluff("", "a.sql").is_empty());
        assert!(parse_sqlfluff("\n\n", "a.sql").is_empty());
        // An object (not an array) is not the expected shape → empty, no panic.
        assert!(parse_sqlfluff(r#"{"violations":[]}"#, "a.sql").is_empty());
    }

    #[test]
    fn redacts_secret_in_description() {
        let json = r#"[{"filepath":"a.sql","violations":[{"line_no":1,"code":"LT01","description":"leaked token AKIAIOSFODNN7EXAMPLE here"}]}]"#;
        let findings = parse_sqlfluff(json, "a.sql");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when sqlfluff absent; ONE tiny run when
    //      present (single-binary linter, so a per-file invocation is cheap). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_messy_sql() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent sqlfluff → empty Vec, no error (graceful absence). When sqlfluff IS
        // present, a deliberately mis-formatted query is flagged (ONE tiny run).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aspis-sqlfluff-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Lowercase keywords + irregular whitespace: sqlfluff's ansi rules flag this.
        let rel = "bad.sql";
        std::fs::write(dir.join(rel), "select  a,b   from t\n").unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target).into_findings();
        if crate::backend::projects::command_exists("sqlfluff") {
            assert!(
                !findings.is_empty(),
                "sqlfluff should flag the messy query in {rel}"
            );
            for f in &findings {
                assert_eq!(f.source, "sqlfluff");
                assert_eq!(f.category, Category::Style);
                // Advisory cap: never High.
                assert_ne!(f.severity, Severity::High);
            }
        } else {
            assert!(findings.is_empty(), "absent sqlfluff must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
