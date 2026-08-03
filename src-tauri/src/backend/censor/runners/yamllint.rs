//! yamllint runner (YAML linter).
//!
//! `yamllint` is a STANDALONE YAML linter (a single Python entry-point binary — no
//! compile, no toolchain bootstrap), cheap enough to run per-file in the FINE
//! (per-keystroke-settled) loop. We invoke it with the stable, line-oriented parsable
//! format:
//!
//! ```text
//! yamllint --format parsable <file>
//! ```
//!
//! The `--format parsable` reporter writes ONE diagnostic per line to STDOUT in the form:
//!
//! ```text
//! file:line:col: [level] message (rule)
//! ```
//!
//! so the runner captures stdout and parses that shape. Advisory: an `error` is a
//! structural problem (syntax error, duplicate key) → Correctness, capped at MEDIUM (see
//! [`severity_from_yamllint`] — even an `error` is Medium, never High, until the FP-rate
//! on this repo is measured); a `warning` is a style suggestion (line length,
//! indentation) → Style/Low. Absent `yamllint` → empty Vec (never an error).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_yamllint;
use super::DEFAULT_RUNNER_TIMEOUT;
use super::{
    cap, redact_secrets, run_capture_with_timeout, split_file_and_coord, Granularity, RawFinding, RunnerOutcome,
    RunTarget,
};
use std::path::Path;

pub fn granularity() -> Granularity {
    // Single-binary, no-compile linter → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// Parse yamllint `--format parsable` stdout (one diagnostic per line, of the form
/// `file:line:col: [level] message (rule)`). PURE. Lines that don't match the shape
/// (blank lines, a missing `[level]` bracket, a non-numeric line/col) are IGNORED —
/// never a panic. The level token (`error`/`warning`) is mapped via
/// [`severity_from_yamllint`] (advisory: capped at Medium).
///
/// PRIVACY: a yamllint message can interpolate a key/value from the source; the message
/// is run through `redact_secrets` before it lands in title/body.
pub fn parse_yamllint(stdout: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(finding) = parse_yamllint_line(line) {
            out.push(finding);
        }
    }
    out
}

/// Parse ONE yamllint parsable line `file:line:col: [level] message (rule)` into a
/// [`RawFinding`], or `None` if the line does not match the shape (no panic).
///
/// Like shellcheck, the `file` may contain a Windows drive colon, so we anchor on the
/// first `:<digits>:<digits>: ` coordinate triplet via the shared [`split_file_and_coord`]
/// rather than splitting on every `:`. The remainder after the triplet is `[level]
/// message`; we pull the bracketed level token, then take the rest as the message. An empty
/// file/message, a non-numeric coordinate, or a missing `[level]` bracket → `None`.
fn parse_yamllint_line(line: &str) -> Option<RawFinding> {
    let (file, line_no, _col, after_coord) = split_file_and_coord(line)?;
    if file.is_empty() {
        return None;
    }
    // after_coord = "[level] message (rule)"
    let after_coord = after_coord.trim_start();
    let inside = after_coord.strip_prefix('[')?;
    let (level_tok, after_level) = inside.split_once(']')?;
    let level_tok = level_tok.trim();
    let message = after_level.trim();
    if message.is_empty() {
        return None;
    }
    // yamllint always emits a positive line number; treat 0 defensively as no line.
    let line_field = (line_no != 0).then_some(line_no);

    let (severity, category) = severity_from_yamllint(level_tok);
    let safe_message = redact_secrets(message);
    Some(RawFinding {
        file: file.replace('\\', "/"),
        line: line_field,
        severity,
        category,
        source: "yamllint".to_string(),
        title: format!("yamllint: {}", cap(&safe_message, 200)),
        body: cap(&safe_message, 1000),
    })
}

/// Run yamllint on a single file from the project root. Absent `yamllint` → empty
/// (never an error). Diagnostics go to STDOUT in the parsable format, so we capture
/// stdout with the default per-file timeout. The file path is the orchestrator-validated
/// project-relative path (a leading-`-` component is rejected upstream by
/// `validate_rel_path`, so it can't be mistaken for a flag).
pub fn run(root: &Path, target: &RunTarget) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("yamllint") {
        return RunnerOutcome::Skipped;
    }
    let stdout = run_capture_with_timeout(
        "yamllint",
        &["--format", "parsable", &target.file_rel_path],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_yamllint(&s)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_warning_and_an_error_line() {
        // Captured-sample parsable stdout (`file:line:col: [level] message (rule)`).
        let stdout = "\
ci.yml:3:81: [warning] line too long (95 > 80 characters) (line-length)
ci.yml:8:1: [error] syntax error: could not find expected ':' (syntax)
";
        let findings = parse_yamllint(stdout);
        assert_eq!(findings.len(), 2, "findings: {findings:?}");

        let w = &findings[0];
        assert_eq!(w.file, "ci.yml");
        assert_eq!(w.line, Some(3));
        // warning → advisory Low, Style.
        assert_eq!(w.severity, Severity::Low);
        assert_eq!(w.category, Category::Style);
        assert_eq!(w.source, "yamllint");
        assert!(w.title.starts_with("yamllint: "));
        assert!(w.body.contains("line too long"));

        let e = &findings[1];
        assert_eq!(e.file, "ci.yml");
        assert_eq!(e.line, Some(8));
        // error → advisory Medium (never High), Correctness.
        assert_eq!(e.severity, Severity::Medium);
        assert_eq!(e.category, Category::Correctness);
        assert!(e.body.contains("syntax error"));
    }

    #[test]
    fn message_with_internal_colon_is_kept_whole() {
        // The message is everything after `]`, so an internal colon (the `syntax error:`
        // prefix and its remainder) survives.
        let stdout = "a.yaml:1:1: [error] syntax error: mapping values are not allowed (syntax)\n";
        let findings = parse_yamllint(stdout);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0]
                .body
                .contains("syntax error: mapping values are not allowed"),
            "message truncated at internal colon: {}",
            findings[0].body
        );
    }

    #[test]
    fn ignores_malformed_and_bracketless_lines_without_panic() {
        let stdout = "\
a.yaml:3:81: [warning] real diagnostic (line-length)
this is not a diagnostic
a.yaml:notanumber:1: [warning] bad line number (line-length)
a.yaml:9:2: no bracket level here
a.yaml:10:1: [warning]
a.yaml:12:1: [error] another real one (syntax)
";
        let findings = parse_yamllint(stdout);
        // Only the two well-formed lines with a bracketed level + non-empty message
        // survive. The prose line, the bad line number, the bracketless line, and the
        // empty-message line are dropped.
        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[1].line, Some(12));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn windows_drive_path_is_not_split_on_the_drive_colon() {
        let stdout = "C:\\conf\\a.yaml:4:2: [warning] trailing spaces (trailing-spaces)\n";
        let findings = parse_yamllint(stdout);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "C:/conf/a.yaml");
        assert_eq!(findings[0].line, Some(4));
        assert_eq!(findings[0].severity, Severity::Low);
    }

    #[test]
    fn empty_input_yields_no_findings() {
        assert!(parse_yamllint("").is_empty());
        assert!(parse_yamllint("\n\n").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let stdout = "a.yaml:1:1: [warning] leaked token AKIAIOSFODNN7EXAMPLE here (key)\n";
        let findings = parse_yamllint(stdout);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when yamllint absent; ONE tiny run when
    //      present (single-binary linter, so a per-file invocation is cheap). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_bad_yaml() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent yamllint → empty Vec, no error (graceful absence). When yamllint IS
        // present, a file with a YAML problem is flagged (ONE tiny run).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aspis-yamllint-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A duplicate mapping key — yamllint flags this with its default config.
        let rel = "bad.yaml";
        std::fs::write(dir.join(rel), "a: 1\na: 2\n").unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target).into_findings();
        if crate::backend::projects::command_exists("yamllint") {
            // On an ELEVATED Windows host the non-empty claim is skipped — see
            // `assert_flags_or_skip_if_elevated`'s doc comment (tauri#13926).
            super::super::assert_flags_or_skip_if_elevated(
                &findings,
                "yamllint",
                &format!("yamllint should flag the duplicate key in {rel}"),
                |f| {
                    // Advisory cap: never High.
                    assert_ne!(f.severity, Severity::High);
                },
            );
        } else {
            assert!(findings.is_empty(), "absent yamllint must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
