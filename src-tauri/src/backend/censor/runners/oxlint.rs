//! oxlint runner.
//!
//! `oxlint --format unix -- <file>` emits unix-format lines:
//! `path:LINE:COL: message text [rest]`. Severity/category via
//! `severity_from_oxlint`. Granularity is Fine (per-file).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_oxlint;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// Parse `oxlint --format unix` stdout. PURE. `file_hint` is the project-relative
/// path of the file we asked oxlint to lint; oxlint reports absolute paths in
/// the output lines, so we use `file_hint` for the finding's `file` (consistent,
/// already project-relative + normalized by the caller). Tolerant: malformed lines
/// are skipped.
pub fn parse_oxlint(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Split into path, LINE, COL, message by the first three ':' separators.
        // Format: path:LINE:COL: message
        let mut parts = line.splitn(4, ':');
        let _path = parts.next(); // path (absolute, ignored)
        let line_str = parts.next();
        let col_str = parts.next();
        let message_part = parts.next();

        let line_num = match line_str.and_then(|s| s.trim().parse::<u32>().ok()) {
            Some(n) => n,
            None => continue,
        };
        let _col = match col_str.and_then(|s| s.trim().parse::<u32>().ok()) {
            Some(c) => c,
            None => continue,
        };
        let message = match message_part {
            Some(m) => m.trim().to_string(),
            None => continue,
        };

        let (severity, category) = severity_from_oxlint(&message);
        let safe_message = redact_secrets(&message);
        let title = cap(&safe_message, 200);
        out.push(RawFinding {
            file: file_hint.to_string(),
            line: Some(line_num),
            severity,
            category,
            source: "oxlint".to_string(),
            title,
            body: cap(&safe_message, 1000),
        });
    }
    out
}

/// Run oxlint on a single file from the project root.
/// Absent `oxlint` → empty. `--` ends flag parsing so a file whose name
/// begins with `-` is never interpreted as an option.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("oxlint") {
        return Vec::new();
    }
    let stdout = run_capture(
        "oxlint",
        &["--format", "unix", "--", &target.file_rel_path],
        root,
    );
    match stdout {
        Some(s) => parse_oxlint(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_error_and_warning_lines() {
        let output = "src/a.ts:10:5: 'x' is defined but never used. [Error] [no-unused-vars]\nsrc/b.ts:20:1: Expected '===' and instead saw '=='. [Warning] [eqeqeq]";
        let findings = parse_oxlint(output, "src/a.ts");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "src/a.ts");
        assert_eq!(findings[0].line, Some(10));
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].category, Category::Correctness);
        assert!(findings[0].title.contains("'x' is defined"));
        assert_eq!(findings[1].severity, Severity::Medium);
    }

    #[test]
    fn skips_malformed_lines() {
        let output = "src/a.ts:10:5: Valid message\nbad line\nsrc/b.ts:notint:1: Bad line";
        let findings = parse_oxlint(output, "src/a.ts");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, Some(10));
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_oxlint("", "x.ts").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let output = "src/a.ts:1:1: Found secret AKIAIOSFODNN7EXAMPLE in code [Error]";
        let findings = parse_oxlint(output, "src/a.ts");
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