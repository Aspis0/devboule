//! vulture runner (Python dead-code detector).
//!
//! vulture has no JSON output; it prints one diagnostic per line:
//!   `path/to/file.py:LINE: <description> (NN% confidence)`
//! We parse `file:line: message`. Severity/category via `severity_from_vulture`
//! (Low DeadCode). Granularity is Fine.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_vulture;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunnerOutcome, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// Parse vulture stdout. PURE. Each line is `file:line: message`. Lines that don't
/// match (blank, no colon-delimited line number) are skipped. Tolerant.
pub fn parse_vulture(stdout: &str) -> Vec<RawFinding> {
    let (severity, category) = severity_from_vulture();
    let mut out = Vec::new();
    for line in stdout.lines() {
        if let Some((file, line_no, message)) = parse_vulture_line(line) {
            // PRIVACY: vulture echoes the unused symbol name into its message;
            // a secret-named identifier would otherwise reach the shard. Redact
            // secret-shaped tokens BEFORE title/body. Redact once, then cap.
            let safe_message = redact_secrets(&message);
            out.push(RawFinding {
                file: file.replace('\\', "/"),
                line: Some(line_no),
                severity,
                category,
                source: "vulture".to_string(),
                title: cap(&safe_message, 200),
                body: cap(&safe_message, 1000),
            });
        }
    }
    out
}

/// Parse one vulture line into (file, line, message). The format is
/// `file:line: message` — but Windows paths contain a drive-letter colon
/// (`C:\...`), so we locate the `:<digits>:` line marker rather than splitting on
/// the first colon.
fn parse_vulture_line(line: &str) -> Option<(String, u32, String)> {
    let line = line.trim_end();
    if line.is_empty() {
        return None;
    }
    // Find the LAST occurrence of ":<digits>: " which delimits file / line / msg.
    // Scan colons; for each, check that the chars after it up to the next colon
    // are all ASCII digits.
    let bytes = line.as_bytes();
    let mut search_from = 0usize;
    while let Some(rel) = line[search_from..].find(':') {
        let colon = search_from + rel;
        let after = &line[colon + 1..];
        // Read the run of digits immediately after this colon.
        let digit_len = after.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digit_len > 0 {
            let next = colon + 1 + digit_len;
            // Must be followed by another ':' to be the line-number delimiter.
            if next < bytes.len() && bytes[next] == b':' {
                let file = line[..colon].trim();
                let line_no: u32 = line[colon + 1..next].parse().ok()?;
                let message = line[next + 1..].trim().to_string();
                if !file.is_empty() && !message.is_empty() {
                    return Some((file.to_string(), line_no, message));
                }
            }
        }
        search_from = colon + 1;
    }
    None
}

/// Run vulture on a single file from the project root. Absent `vulture` → empty.
/// `--` ends flag parsing so a `-`-leading file name is never read as an option.
pub fn run(root: &Path, target: &RunTarget) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("vulture") {
        return RunnerOutcome::Skipped;
    }
    // DEMOTED 2026-06-12 (master plan P2): vulture is FP-prone on dynamic Python;
    // only 100%-confidence findings are objective enough for the gate.
    let stdout = run_capture(
        "vulture",
        &["--min-confidence", "100", "--", &target.file_rel_path],
        root,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_vulture(&s)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_unused_variable_line() {
        let line = "src/a.py:12: unused variable 'x' (60% confidence)";
        let findings = parse_vulture(line);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/a.py");
        assert_eq!(f.line, Some(12));
        assert_eq!(f.severity, Severity::Low);
        assert_eq!(f.category, Category::DeadCode);
        assert_eq!(f.source, "vulture");
        assert!(f.title.contains("unused variable"));
    }

    #[test]
    fn handles_windows_drive_letter_path() {
        let line = "C:\\proj\\a.py:5: unused function 'foo' (90% confidence)";
        let findings = parse_vulture(line);
        assert_eq!(findings.len(), 1);
        // Drive-letter colon preserved within the (normalized) path; line parsed.
        assert_eq!(findings[0].line, Some(5));
        assert!(findings[0].file.ends_with("a.py"));
    }

    #[test]
    fn skips_non_matching_lines() {
        let stdout = "no line number here\n\nsrc/b.py:3: unused import 'os' (90% confidence)";
        let findings = parse_vulture(stdout);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/b.py");
    }

    #[test]
    fn empty_yields_empty() {
        assert!(parse_vulture("").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let line = "a.py:5: unused variable 'AKIAIOSFODNN7EXAMPLE' (60% confidence)";
        let findings = parse_vulture(line);
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
