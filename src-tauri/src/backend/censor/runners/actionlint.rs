//! actionlint runner (GitHub Actions workflow linter).
//!
//! `actionlint` is a STANDALONE static checker for GitHub Actions workflow files (a
//! single native Go binary — no compile, no toolchain bootstrap), cheap enough to run
//! per-file in the FINE (per-keystroke-settled) loop. We invoke it with its default,
//! line-oriented reporter:
//!
//! ```text
//! actionlint <file>
//! ```
//!
//! actionlint's DEFAULT format writes ONE diagnostic per line to STDOUT in the form:
//!
//! ```text
//! file:line:col: message [rule]
//! ```
//!
//! (followed by a colorful source snippet + an `^` indicator on subsequent lines, which
//! do NOT match the `:line:col:` shape and are therefore ignored). We anchor on the first
//! `:<line>:<col>: ` numeric triplet exactly like the shellcheck parser; everything before
//! it is the file, the remainder (message, including any internal colons AND the trailing
//! `[rule]`) is kept whole. actionlint has no severity tier — every finding is a workflow
//! CORRECTNESS issue, capped at Medium (advisory; see [`severity_from_actionlint`]).
//! Absent `actionlint` → empty Vec (never an error).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_actionlint;
use super::DEFAULT_RUNNER_TIMEOUT;
use super::{
    cap, redact_secrets, run_capture_with_timeout, split_file_and_coord, Granularity, RawFinding,
    RunTarget,
};
use std::path::Path;

pub fn granularity() -> Granularity {
    // Single-binary, no-compile checker → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// Parse actionlint default-format stdout (one diagnostic per line, of the form
/// `file:line:col: message [rule]`, interleaved with non-matching snippet/indicator
/// lines). PURE. Lines that don't match the `:line:col:` shape (the source snippet, the
/// `^` indicator, blank lines) are IGNORED — never a panic. Every finding is mapped via
/// [`severity_from_actionlint`] (advisory Medium / Correctness).
///
/// PRIVACY: an actionlint message can echo an expression / literal from the workflow; the
/// message is run through `redact_secrets` before it lands in title/body.
pub fn parse_actionlint(stdout: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(finding) = parse_actionlint_line(line) {
            out.push(finding);
        }
    }
    out
}

/// Parse ONE actionlint line `file:line:col: message [rule]` into a [`RawFinding`], or
/// `None` if the line does not match the shape (no panic). We anchor on the first
/// `:<line>:<col>: ` numeric coordinate triplet via the shared [`split_file_and_coord`]
/// (tolerating a Windows drive colon in the file portion), treat everything before it as
/// the file, and keep the whole remainder as the message. An empty file/message, or a
/// non-numeric coordinate, → `None`.
fn parse_actionlint_line(line: &str) -> Option<RawFinding> {
    let (file, line_no, _col, message) = split_file_and_coord(line)?;
    if file.is_empty() {
        return None;
    }
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    // actionlint emits a positive line number; treat 0 defensively as no line.
    let line_field = (line_no != 0).then_some(line_no);

    let (severity, category) = severity_from_actionlint();
    let safe_message = redact_secrets(message);
    Some(RawFinding {
        file: file.replace('\\', "/"),
        line: line_field,
        severity,
        category,
        source: "actionlint".to_string(),
        title: format!("actionlint: {}", cap(&safe_message, 200)),
        body: cap(&safe_message, 1000),
    })
}

/// Run actionlint on a single file from the project root. Absent `actionlint` → empty
/// (never an error). Diagnostics go to STDOUT in the default format, so we capture stdout
/// with the default per-file timeout. The file path is the orchestrator-validated
/// project-relative path (a leading-`-` component is rejected upstream by
/// `validate_rel_path`, so it can't be mistaken for a flag).
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("actionlint") {
        return Vec::new();
    }
    let stdout = run_capture_with_timeout(
        "actionlint",
        &[&target.file_rel_path],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stdout {
        Some(s) => parse_actionlint(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_default_format_lines() {
        // Captured-sample default-format stdout (`file:line:col: message [rule]`),
        // interleaved with the snippet + indicator lines actionlint also prints.
        let stdout = "\
.github/workflows/ci.yml:3:1: \"on\" section is missing in workflow [syntax-check]
  |
3 | jobs:
  | ^~~~~
.github/workflows/ci.yml:9:9: property \"runs-on\" is not defined [expression]
";
        let findings = parse_actionlint(stdout);
        assert_eq!(findings.len(), 2, "findings: {findings:?}");

        let a = &findings[0];
        assert_eq!(a.file, ".github/workflows/ci.yml");
        assert_eq!(a.line, Some(3));
        // actionlint findings are correctness issues, capped at Medium (advisory).
        assert_eq!(a.severity, Severity::Medium);
        assert_eq!(a.category, Category::Correctness);
        assert_eq!(a.source, "actionlint");
        assert!(a.title.starts_with("actionlint: "));
        assert!(a.body.contains("section is missing"));
        // The trailing `[rule]` is kept as part of the message.
        assert!(a.body.contains("[syntax-check]"));

        let b = &findings[1];
        assert_eq!(b.line, Some(9));
        assert!(b.body.contains("runs-on"));
    }

    #[test]
    fn message_with_internal_colon_is_kept_whole() {
        // The message (everything after the coordinate triplet) is kept intact, including
        // internal colons.
        let stdout =
            "f.yml:3:1: input \"node:version\" is not defined in this context [expression]\n";
        let findings = parse_actionlint(stdout);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].body.contains("input \"node:version\" is not defined"),
            "message truncated at internal colon: {}",
            findings[0].body
        );
    }

    #[test]
    fn ignores_snippet_and_malformed_lines_without_panic() {
        let stdout = "\
  |
f.yml:3:1: real diagnostic [rule]
this is not a diagnostic
f.yml:notanumber:1: bad line number [rule]
f.yml:9:2:
f.yml:12:4: another real one [rule]
";
        let findings = parse_actionlint(stdout);
        // Only the two well-formed lines with a non-empty message survive.
        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[1].line, Some(12));
        for f in &findings {
            assert_eq!(f.severity, Severity::Medium);
            assert_eq!(f.category, Category::Correctness);
        }
    }

    #[test]
    fn windows_drive_path_is_not_split_on_the_drive_colon() {
        // A file with a Windows drive colon must still parse — we anchor on the numeric
        // line:col triplet, not the drive colon.
        let stdout = "C:\\repo\\ci.yml:4:2: invalid expression [expression]\n";
        let findings = parse_actionlint(stdout);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "C:/repo/ci.yml");
        assert_eq!(findings[0].line, Some(4));
    }

    #[test]
    fn empty_input_yields_no_findings() {
        assert!(parse_actionlint("").is_empty());
        assert!(parse_actionlint("\n\n").is_empty());
        // A snippet-only output (no diagnostic line) yields nothing.
        assert!(parse_actionlint("  |\n3 | jobs:\n  | ^~~~~\n").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let stdout = "ci.yml:1:1: leaked token AKIAIOSFODNN7EXAMPLE here [rule]\n";
        let findings = parse_actionlint(stdout);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when actionlint absent; ONE tiny run when
    //      present (single-binary checker, so a per-file invocation is cheap). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_bad_workflow() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent actionlint → empty Vec, no error (graceful absence). When actionlint IS
        // present, a workflow with an obvious error is flagged (ONE tiny run).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aspis-actionlint-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The runner is invoked with the project-relative path; actionlint resolves it
        // from `root`, so the directory layout under root doesn't matter for the spawn.
        let wf_dir = dir.join(".github").join("workflows");
        std::fs::create_dir_all(&wf_dir).unwrap();
        let rel = ".github/workflows/ci.yml";
        // `runs-on` references an undefined matrix value → actionlint flags an expression
        // / runner-label error reliably.
        std::fs::write(
            dir.join(".github/workflows/ci.yml"),
            "on: push\njobs:\n  build:\n    runs-on: ${{ matrix.os }}\n    steps:\n      - run: echo hi\n",
        )
        .unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target);
        if crate::backend::projects::command_exists("actionlint") {
            assert!(
                !findings.is_empty(),
                "actionlint should flag the bad workflow in {rel}"
            );
            for f in &findings {
                assert_eq!(f.source, "actionlint");
                assert_eq!(f.category, Category::Correctness);
                // Advisory cap: never High.
                assert_ne!(f.severity, Severity::High);
            }
        } else {
            assert!(findings.is_empty(), "absent actionlint must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
