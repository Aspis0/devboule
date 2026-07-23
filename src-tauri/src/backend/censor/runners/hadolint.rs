//! hadolint runner (Dockerfile static analyzer).
//!
//! `hadolint` is a STANDALONE Dockerfile linter (a single native binary — no compile,
//! no toolchain bootstrap), cheap enough to run per-file in the FINE
//! (per-keystroke-settled) loop. We invoke it with the machine JSON reporter:
//!
//! ```text
//! hadolint --format json <file>
//! ```
//!
//! The `--format json` reporter writes an ARRAY of diagnostic objects to STDOUT:
//!
//! ```json
//! [ { "file": "Dockerfile", "line": 3, "column": 1, "level": "warning",
//!     "code": "DL3008", "message": "Pin versions in apt get install." } ]
//! ```
//!
//! Parsed DEFENSIVELY with serde_json (tolerate schema variance — unknown fields ignored,
//! every field optional/defaulted, never panic). hadolint can prepend non-JSON warning
//! lines to stdout, so if a whole-string parse fails we retry from the first `[`.
//! Advisory: `error` → Medium (capped, never High), `warning` → Low, `info`/`style` →
//! Style/Low (see [`severity_from_hadolint`]). Absent `hadolint` → empty Vec (never an
//! error).
//!
//! LICENSING INVARIANT (master plan): hadolint is GPL-3.0 → INVOKE-ONLY, NEVER BUNDLE.
//! We spawn the user's own installed `hadolint` binary as a subprocess (an arms-length
//! invocation of a separate program), exactly like every other runner; we do NOT vendor,
//! statically link, or redistribute it. The presence gate (`command_exists`) means an
//! absent hadolint simply yields no findings — the product never ships the GPL binary.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_hadolint;
use super::DEFAULT_RUNNER_TIMEOUT;
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding, RunnerOutcome, RunTarget};
use serde::Deserialize;
use std::path::Path;

pub fn granularity() -> Granularity {
    // Single-binary, no-compile analyzer → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// One diagnostic in hadolint's `--format json` array. Defensive: every field is
/// optional/defaulted and unknown fields are ignored, so a schema drift across hadolint
/// versions never breaks the parse.
#[derive(Deserialize, Default)]
struct HadolintDiag {
    #[serde(default)]
    file: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    level: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// Parse `hadolint --format json` stdout. PURE. Tolerant: malformed/empty JSON → empty
/// Vec (never a panic). hadolint may prepend non-JSON chatter to stdout, so a failed
/// whole-string parse is retried from the first `[` (the JSON array start). The
/// per-finding `file` prefers `file_hint` (consistently project-relative + normalized);
/// the JSON `file` is the fallback when the hint is empty. The `level` is mapped via
/// [`severity_from_hadolint`] (advisory: capped at Medium).
///
/// PRIVACY: a hadolint `message` can interpolate a literal from the Dockerfile; the
/// message is run through `redact_secrets` before it lands in title/body.
pub fn parse_hadolint(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let diags = match parse_json_diags(stdout) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for d in diags {
        let resolved_file = if file_hint.is_empty() {
            d.file.replace('\\', "/")
        } else {
            file_hint.replace('\\', "/")
        };
        if resolved_file.is_empty() {
            continue;
        }
        // Skip a wholly-empty diagnostic (no code AND no message → nothing to show).
        if d.code.is_empty() && d.message.is_empty() {
            continue;
        }
        let (severity, category) = severity_from_hadolint(&d.level);
        let safe_message = redact_secrets(&d.message);
        // Prefix the rule code when present (e.g. "DL3008: Pin versions ...").
        let titled = if d.code.is_empty() {
            cap(&safe_message, 200)
        } else {
            format!("{}: {}", d.code, cap(&safe_message, 200))
        };
        // hadolint emits a positive line number; treat 0/absent defensively as no line.
        let line_field = d.line.filter(|n| *n != 0);
        out.push(RawFinding {
            file: resolved_file,
            line: line_field,
            severity,
            category,
            source: "hadolint".to_string(),
            title: format!("hadolint: {titled}"),
            body: cap(&safe_message, 1000),
        });
    }
    out
}

/// Deserialize the hadolint diagnostic array from `stdout`, tolerating a non-JSON prefix.
/// Tries a whole-string parse first, then retries from the first `[`. `None` if neither
/// yields a valid array.
fn parse_json_diags(stdout: &str) -> Option<Vec<HadolintDiag>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(diags) = serde_json::from_str::<Vec<HadolintDiag>>(trimmed) {
        return Some(diags);
    }
    // Retry from the first '[' — drops any leading log/warning chatter.
    let start = trimmed.find('[')?;
    serde_json::from_str::<Vec<HadolintDiag>>(&trimmed[start..]).ok()
}

/// Run hadolint on a single file from the project root. Absent `hadolint` → empty
/// (never an error). The JSON report goes to STDOUT, so we capture stdout with the
/// default per-file timeout. The file path is the orchestrator-validated project-relative
/// path (a leading-`-` component is rejected upstream by `validate_rel_path`, so it can't
/// be mistaken for a flag).
pub fn run(root: &Path, target: &RunTarget) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("hadolint") {
        return RunnerOutcome::Skipped;
    }
    let stdout = run_capture_with_timeout(
        "hadolint",
        &["--format", "json", &target.file_rel_path],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_hadolint(&s, &target.file_rel_path)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_diags_from_json_array() {
        // Captured-sample `hadolint --format json` output shape.
        let json = r#"[
          {"file":"Dockerfile","line":3,"column":1,"level":"warning","code":"DL3008","message":"Pin versions in apt get install."},
          {"file":"Dockerfile","line":8,"column":1,"level":"error","code":"DL3003","message":"Use WORKDIR to switch to a directory."}
        ]"#;
        let findings = parse_hadolint(json, "Dockerfile");
        assert_eq!(findings.len(), 2, "findings: {findings:?}");

        let w = &findings[0];
        assert_eq!(w.file, "Dockerfile");
        assert_eq!(w.line, Some(3));
        // warning → advisory Low, Correctness.
        assert_eq!(w.severity, Severity::Low);
        assert_eq!(w.category, Category::Correctness);
        assert_eq!(w.source, "hadolint");
        assert!(w.title.starts_with("hadolint: DL3008: "));
        assert!(w.body.contains("Pin versions"));

        let e = &findings[1];
        assert_eq!(e.line, Some(8));
        // error → advisory Medium (never High), Correctness.
        assert_eq!(e.severity, Severity::Medium);
        assert_eq!(e.category, Category::Correctness);
        assert!(e.title.contains("DL3003"));
    }

    #[test]
    fn info_and_style_levels_map_to_style_low() {
        let json = r#"[
          {"file":"Dockerfile","line":1,"level":"info","code":"DL3059","message":"Multiple consecutive RUN."},
          {"file":"Dockerfile","line":2,"level":"style","code":"DL3042","message":"Avoid cache dir."}
        ]"#;
        let findings = parse_hadolint(json, "Dockerfile");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[0].category, Category::Style);
        assert_eq!(findings[1].severity, Severity::Low);
        assert_eq!(findings[1].category, Category::Style);
    }

    #[test]
    fn file_hint_overrides_json_file() {
        // The orchestrator-supplied hint is the canonical project-relative path; it wins
        // over hadolint's reported file.
        let json = r#"[{"file":"/abs/tmp/Dockerfile","line":2,"level":"warning","code":"DL3000","message":"x"}]"#;
        let findings = parse_hadolint(json, "docker/Dockerfile.prod");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "docker/Dockerfile.prod");
    }

    #[test]
    fn tolerates_prefix_before_json() {
        // A non-JSON line can precede the array on stdout; retry from the first '['.
        let stdout = "warning: could not load config\n[{\"file\":\"Dockerfile\",\"line\":1,\"level\":\"warning\",\"code\":\"DL3008\",\"message\":\"pin\"}]\n";
        let findings = parse_hadolint(stdout, "Dockerfile");
        assert_eq!(findings.len(), 1, "findings: {findings:?}");
        assert_eq!(findings[0].line, Some(1));
    }

    #[test]
    fn tolerates_missing_and_extra_fields() {
        // No `code`, no `line`, plus an UNKNOWN field — defensive parse keeps the
        // message-only finding (line None) and ignores the extra field.
        let json = r#"[{"file":"Dockerfile","level":"warning","message":"some issue","extra":"unknown"}]"#;
        let findings = parse_hadolint(json, "Dockerfile");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, None);
        // No code → title is just the message (no `CODE: ` prefix beyond `hadolint: `).
        assert!(findings[0].title.starts_with("hadolint: some issue"));
    }

    #[test]
    fn empty_array_and_no_diags_yield_no_findings() {
        assert!(parse_hadolint(r#"[]"#, "Dockerfile").is_empty());
    }

    #[test]
    fn malformed_and_empty_yield_empty_no_panic() {
        assert!(parse_hadolint("not json", "Dockerfile").is_empty());
        assert!(parse_hadolint("", "Dockerfile").is_empty());
        assert!(parse_hadolint("\n\n", "Dockerfile").is_empty());
        // An object (not an array) is not the expected shape → empty, no panic.
        assert!(parse_hadolint(r#"{"level":"error"}"#, "Dockerfile").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let json = r#"[{"file":"Dockerfile","line":1,"level":"warning","code":"DL3008","message":"leaked token AKIAIOSFODNN7EXAMPLE here"}]"#;
        let findings = parse_hadolint(json, "Dockerfile");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when hadolint absent; ONE tiny run when
    //      present (single-binary analyzer, so a per-file invocation is cheap). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_bad_dockerfile() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent hadolint → empty Vec, no error (graceful absence). When hadolint IS
        // present, a Dockerfile with an obvious smell is flagged (ONE tiny run).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aspis-hadolint-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // An unpinned `apt-get install` (DL3008) + `latest` tag (DL3007): hadolint flags this.
        let rel = "Dockerfile";
        std::fs::write(
            dir.join(rel),
            "FROM ubuntu:latest\nRUN apt-get install curl\n",
        )
        .unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target).into_findings();
        if crate::backend::projects::command_exists("hadolint") {
            assert!(
                !findings.is_empty(),
                "hadolint should flag the unpinned install / latest tag in {rel}"
            );
            for f in &findings {
                assert_eq!(f.source, "hadolint");
                // Advisory cap: never High.
                assert_ne!(f.severity, Severity::High);
            }
        } else {
            assert!(findings.is_empty(), "absent hadolint must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
