//! HTML Tidy runner (HTML validity checker).
//!
//! `tidy` is a STANDALONE HTML validator/cleaner (a single native binary — no compile,
//! no toolchain bootstrap), cheap enough to run per-file in the FINE (per-keystroke-
//! settled) loop. We invoke it in error-checking mode:
//!
//! ```text
//! tidy -q -e <file>
//! ```
//!
//! Flags:
//!   - `-q` (quiet) — suppress the version banner / non-diagnostic chatter.
//!   - `-e` (errors) — show ONLY errors and warnings; do NOT emit the cleaned markup on
//!     stdout (we never want tidy to print a rewritten document).
//!
//! tidy writes its diagnostics to STDERR in the form:
//!
//! ```text
//! line N column M - Warning: <message>
//! line N column M - Error: <message>
//! ```
//!
//! so the runner captures stderr and parses that shape. Advisory: Correctness, capped at
//! MEDIUM (see [`severity_from_tidy`] — even an `Error` is Medium, never High, until the
//! FP-rate on this repo is measured). Absent `tidy` → empty Vec (never an error).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_tidy;
use super::DEFAULT_RUNNER_TIMEOUT;
use super::{cap, redact_secrets, run_capture_stderr_with_timeout, Granularity, RawFinding, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    // Single-binary, no-compile validator → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// Parse HTML Tidy stderr (one diagnostic per line, of the form
/// `line N column M - Severity: message`). PURE. Lines that don't match the shape
/// (tidy's summary banners, `Info:`/`Document:` notes, blank lines) are IGNORED — never
/// a panic. The severity token (`Warning`/`Error`) is mapped via [`severity_from_tidy`]
/// (advisory: Correctness, `Error` capped at Medium, `Warning` → Low). Findings are
/// attributed to `file_hint` (the project-relative path we asked tidy to check — tidy
/// reports line/column, not a file path, in its per-diagnostic lines).
///
/// PRIVACY: a tidy message can interpolate text/attribute values from the source; the
/// message is run through `redact_secrets` before it lands in the title/body.
pub fn parse_tidy(stderr: &str, file_hint: &str) -> Vec<RawFinding> {
    let file = file_hint.replace('\\', "/");
    let mut out = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(finding) = parse_tidy_line(line, &file) {
            out.push(finding);
        }
    }
    out
}

/// Parse ONE tidy diagnostic line `line N column M - Severity: message` into a
/// [`RawFinding`], or `None` if the line does not match the shape (no panic). The
/// recognized severities are `Warning` and `Error` (case-insensitively); any other
/// leading token (e.g. tidy's `Info:` or a summary line) → `None`. An empty message → `None`.
fn parse_tidy_line(line: &str, file: &str) -> Option<RawFinding> {
    // Expected: "line N column M - Severity: message"
    let rest = line.strip_prefix("line ")?;
    // rest = "N column M - Severity: message"
    let (line_no_str, after_line) = rest.split_once(" column ")?;
    let line_no: u32 = line_no_str.trim().parse().ok()?;
    // after_line = "M - Severity: message"
    let (_col_str, after_col) = after_line.split_once(" - ")?;
    // after_col = "Severity: message"
    let (severity_tok, message) = after_col.split_once(':')?;
    let severity_tok = severity_tok.trim();
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    // Only Warning/Error diagnostics are findings; anything else (Info, etc.) is ignored.
    let sev_lower = severity_tok.to_ascii_lowercase();
    if sev_lower != "warning" && sev_lower != "error" {
        return None;
    }
    // tidy emits `0` as a line number for a document-level note; treat that as no line.
    let line_field = (line_no != 0).then_some(line_no);

    let (severity, category) = severity_from_tidy(severity_tok);
    let safe_message = redact_secrets(message);
    Some(RawFinding {
        file: file.to_string(),
        line: line_field,
        severity,
        category,
        source: "tidy".to_string(),
        title: format!("tidy: {}", cap(&safe_message, 200)),
        body: cap(&safe_message, 1000),
    })
}

/// Run tidy on a single file from the project root. Absent `tidy` → empty (never an
/// error). Diagnostics go to STDERR (stdout would carry the cleaned markup, which `-e`
/// suppresses), so we capture the stderr stream with the default per-file timeout. The
/// file path is the orchestrator-validated project-relative path (a leading-`-`
/// component is rejected upstream by `validate_rel_path`, so it can't be mistaken for a
/// flag).
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("tidy") {
        return Vec::new();
    }
    let stderr = run_capture_stderr_with_timeout(
        "tidy",
        &["-q", "-e", &target.file_rel_path],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stderr {
        Some(s) => parse_tidy(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_warning_and_an_error_line() {
        // A captured-sample stderr (the `line N column M - Severity: message` form).
        let stderr = "\
line 3 column 1 - Warning: missing <!DOCTYPE> declaration
line 8 column 5 - Error: <foo> is not recognized!
";
        let findings = parse_tidy(stderr, "index.html");
        assert_eq!(findings.len(), 2, "findings: {findings:?}");

        let w = &findings[0];
        assert_eq!(w.file, "index.html");
        assert_eq!(w.line, Some(3));
        // Warning → advisory Low, Correctness.
        assert_eq!(w.severity, Severity::Low);
        assert_eq!(w.category, Category::Correctness);
        assert_eq!(w.source, "tidy");
        assert!(w.title.starts_with("tidy: "));
        assert!(w.body.contains("missing <!DOCTYPE> declaration"));

        let e = &findings[1];
        assert_eq!(e.file, "index.html");
        assert_eq!(e.line, Some(8));
        // Error → advisory Medium (never High), Correctness.
        assert_eq!(e.severity, Severity::Medium);
        assert_eq!(e.category, Category::Correctness);
        assert!(e.body.contains("is not recognized"));
    }

    #[test]
    fn message_with_internal_colon_is_kept_whole() {
        // split_once(':') keeps the whole remainder as the message, so an internal colon
        // (and the words after it) survive.
        let stderr = "line 1 column 1 - Warning: trimming empty: element here\n";
        let findings = parse_tidy(stderr, "a.html");
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].body.contains("trimming empty: element here"),
            "message truncated at internal colon: {}",
            findings[0].body
        );
    }

    #[test]
    fn ignores_banner_summary_and_info_lines_without_panic() {
        let stderr = "\
Tidy for HTML5 (version 5.8.0)
line 3 column 1 - Warning: missing <!DOCTYPE> declaration
Info: Document content looks like HTML5
this is not a diagnostic
line notanumber column 1 - Warning: bad line number
line 9 column 2 - Warning:
line 12 column 4 - Error: unescaped & or unknown entity
3 warnings, 1 error were found!
";
        let findings = parse_tidy(stderr, "a.html");
        // Only the two well-formed Warning/Error lines with a non-empty message survive.
        // The banner, the `Info:` note, the prose line, the bad line number, the empty-
        // message line, and the summary line are all dropped.
        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[1].line, Some(12));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn empty_input_yields_no_findings() {
        assert!(parse_tidy("", "a.html").is_empty());
        assert!(parse_tidy("\n\n", "a.html").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let stderr = "line 1 column 1 - Warning: leaked token AKIAIOSFODNN7EXAMPLE here\n";
        let findings = parse_tidy(stderr, "a.html");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when tidy absent; ONE tiny run when present
    //      (single-binary validator, so a per-file invocation is cheap). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_invalid_html() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent tidy → empty Vec, no error (graceful absence). When tidy IS present, a
        // file with clearly invalid/incomplete markup is flagged (ONE tiny run).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aspis-tidy-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No DOCTYPE, no <title>, an unclosed tag — tidy reliably emits warnings/errors.
        let rel = "bad.html";
        std::fs::write(dir.join(rel), "<html><body><p>hi</body></html>\n").unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target);
        if crate::backend::projects::command_exists("tidy") {
            assert!(
                !findings.is_empty(),
                "tidy should flag the invalid markup in {rel}"
            );
            for f in &findings {
                assert_eq!(f.source, "tidy");
                assert_eq!(f.category, Category::Correctness);
                // Advisory cap: never High.
                assert_ne!(f.severity, Severity::High);
            }
        } else {
            assert!(findings.is_empty(), "absent tidy must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
