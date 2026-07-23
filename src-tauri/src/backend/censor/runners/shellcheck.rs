//! shellcheck runner (shell-script static analyzer).
//!
//! `shellcheck` is a STANDALONE static analyzer for shell scripts (a single native
//! binary — no compile, no toolchain bootstrap), cheap enough to run per-file in the
//! FINE (per-keystroke-settled) loop. We invoke it with the stable, line-oriented gcc
//! format:
//!
//! ```text
//! shellcheck --format=gcc <file>
//! ```
//!
//! The `--format=gcc` reporter writes ONE diagnostic per line to STDOUT in the form:
//!
//! ```text
//! file:line:col: severity: message [SC####]
//! ```
//!
//! so the runner captures stdout and parses that shape. Advisory: a `warning`/`error`
//! is a likely bug (Correctness), an `info`/`style` is a stylistic suggestion (Style);
//! every level is CAPPED at MEDIUM (see [`severity_from_shellcheck`] — even an `error`
//! is Medium, never High, until the FP-rate on this repo is measured). Absent
//! `shellcheck` → empty Vec (never an error).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_shellcheck;
use super::DEFAULT_RUNNER_TIMEOUT;
use super::{
    cap, redact_secrets, run_capture_with_timeout, split_file_and_coord, Granularity, RawFinding, RunnerOutcome,
    RunTarget,
};
use std::path::Path;

pub fn granularity() -> Granularity {
    // Single-binary, no-compile analyzer → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// Parse shellcheck `--format=gcc` stdout (one diagnostic per line, of the form
/// `file:line:col: severity: message [SC####]`). PURE. Lines that don't match the shape
/// (shellcheck banners, blank lines, a non-numeric line/col) are IGNORED — never a panic.
/// The level token (`error`/`warning`/`info`/`style`) is mapped via
/// [`severity_from_shellcheck`] (advisory: capped at Medium).
///
/// PRIVACY: a shellcheck message can interpolate a variable name / literal from the
/// source; the message is run through `redact_secrets` before it lands in title/body.
pub fn parse_shellcheck(stdout: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(finding) = parse_shellcheck_line(line) {
            out.push(finding);
        }
    }
    out
}

/// Parse ONE shellcheck gcc-format line `file:line:col: severity: message [SC####]` into
/// a [`RawFinding`], or `None` if the line does not match the shape (no panic).
///
/// The split is deliberate: `file` may itself contain a Windows drive colon
/// (`C:\path\a.sh`), so we DON'T split blindly on `:`. Instead we anchor on the
/// `:<line>:<col>: ` numeric coordinate triplet via the shared [`split_file_and_coord`] —
/// everything before the triplet is the file, the two digit runs are line/col (we keep
/// only the line), and the remainder is `severity: message`. The severity is the token
/// before the first `:` of the remainder; the message is the rest (a message containing a
/// colon is preserved intact). An empty file/message, or a non-numeric coordinate, → `None`.
fn parse_shellcheck_line(line: &str) -> Option<RawFinding> {
    let (file, line_no, _col, after_coord) = split_file_and_coord(line)?;
    if file.is_empty() {
        return None;
    }
    // after_coord = "severity: message [SC####]"
    let (severity_tok, message) = after_coord.split_once(':')?;
    let severity_tok = severity_tok.trim();
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    // shellcheck always emits a positive line number; treat 0 defensively as no line.
    let line_field = (line_no != 0).then_some(line_no);

    let (severity, category) = severity_from_shellcheck(severity_tok);
    let safe_message = redact_secrets(message);
    Some(RawFinding {
        file: file.replace('\\', "/"),
        line: line_field,
        severity,
        category,
        source: "shellcheck".to_string(),
        title: format!("shellcheck: {}", cap(&safe_message, 200)),
        body: cap(&safe_message, 1000),
    })
}

/// Run shellcheck on a single file from the project root. Absent `shellcheck` → empty
/// (never an error). Diagnostics go to STDOUT in the gcc format, so we capture stdout
/// with the default per-file timeout. The file path is the orchestrator-validated
/// project-relative path (a leading-`-` component is rejected upstream by
/// `validate_rel_path`, so it can't be mistaken for a flag).
pub fn run(root: &Path, target: &RunTarget) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("shellcheck") {
        return RunnerOutcome::Skipped;
    }
    let stdout = run_capture_with_timeout(
        "shellcheck",
        &["--format=gcc", &target.file_rel_path],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_shellcheck(&s)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_warning_and_an_error_line() {
        // Captured-sample gcc-format stdout (`file:line:col: severity: message [SC####]`).
        let stdout = "\
deploy.sh:3:5: warning: var is referenced but not assigned [SC2154]
deploy.sh:8:1: error: Couldn't parse this command [SC1073]
";
        let findings = parse_shellcheck(stdout);
        assert_eq!(findings.len(), 2, "findings: {findings:?}");

        let w = &findings[0];
        assert_eq!(w.file, "deploy.sh");
        assert_eq!(w.line, Some(3));
        // warning → advisory Low, Correctness.
        assert_eq!(w.severity, Severity::Low);
        assert_eq!(w.category, Category::Correctness);
        assert_eq!(w.source, "shellcheck");
        assert!(w.title.starts_with("shellcheck: "));
        assert!(w.body.contains("referenced but not assigned"));

        let e = &findings[1];
        assert_eq!(e.file, "deploy.sh");
        assert_eq!(e.line, Some(8));
        // error → advisory Medium (never High), Correctness.
        assert_eq!(e.severity, Severity::Medium);
        assert_eq!(e.category, Category::Correctness);
        assert!(e.body.contains("Couldn't parse this command"));
    }

    #[test]
    fn info_and_style_levels_map_to_style_low() {
        let stdout = "\
a.sh:1:1: info: Use ./*glob* or -- *glob* so names with dashes won't be options [SC2035]
a.sh:2:1: style: Use $(...) notation instead of legacy backticks [SC2006]
";
        let findings = parse_shellcheck(stdout);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[0].category, Category::Style);
        assert_eq!(findings[1].severity, Severity::Low);
        assert_eq!(findings[1].category, Category::Style);
    }

    #[test]
    fn message_with_internal_colon_is_kept_whole() {
        // The severity is the token before the FIRST colon of the remainder; the message
        // is everything after, so an internal colon (and the words after it) survive.
        let stdout = "a.sh:1:1: warning: note: this line has an internal colon [SC9999]\n";
        let findings = parse_shellcheck(stdout);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0]
                .body
                .contains("note: this line has an internal colon"),
            "message truncated at internal colon: {}",
            findings[0].body
        );
    }

    #[test]
    fn ignores_banner_and_malformed_lines_without_panic() {
        let stdout = "\
In deploy.sh line 3:
a.sh:3:5: warning: real diagnostic [SC2154]
this is not a diagnostic
a.sh:notanumber:1: warning: bad line number [SC0000]
a.sh:9:2: warning:
a.sh:12:4: error: another real one [SC1073]
";
        let findings = parse_shellcheck(stdout);
        // Only the two well-formed lines with a non-empty message survive. The prose
        // banner, the bad line number, and the empty-message line are dropped.
        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[1].line, Some(12));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn windows_drive_path_is_not_split_on_the_drive_colon() {
        // A file with a Windows drive colon must still parse — we anchor on the numeric
        // line:col triplet, not the drive colon.
        let stdout = "C:\\scripts\\a.sh:4:2: warning: unquoted variable [SC2086]\n";
        let findings = parse_shellcheck(stdout);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "C:/scripts/a.sh");
        assert_eq!(findings[0].line, Some(4));
        assert_eq!(findings[0].severity, Severity::Low);
    }

    #[test]
    fn empty_input_yields_no_findings() {
        assert!(parse_shellcheck("").is_empty());
        assert!(parse_shellcheck("\n\n").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let stdout = "a.sh:1:1: warning: leaked token AKIAIOSFODNN7EXAMPLE here [SC2154]\n";
        let findings = parse_shellcheck(stdout);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when shellcheck absent; ONE tiny run when
    //      present (single-binary analyzer, so a per-file invocation is cheap). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_buggy_script() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent shellcheck → empty Vec, no error (graceful absence). When shellcheck IS
        // present, a script with an obvious quoting bug is flagged (ONE tiny run).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aspis-shellcheck-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // An unquoted `$VAR` in `rm` is a classic SC2086 — shellcheck reliably flags it.
        let rel = "bad.sh";
        std::fs::write(
            dir.join(rel),
            "#!/bin/sh\nVAR=\"a b\"\nrm $VAR\n",
        )
        .unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target).into_findings();
        if crate::backend::projects::command_exists("shellcheck") {
            assert!(
                !findings.is_empty(),
                "shellcheck should flag the unquoted variable in {rel}"
            );
            for f in &findings {
                assert_eq!(f.source, "shellcheck");
                // Advisory cap: never High.
                assert_ne!(f.severity, Severity::High);
            }
        } else {
            assert!(findings.is_empty(), "absent shellcheck must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
