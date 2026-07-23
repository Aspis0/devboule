//! cargo fmt runner.
//! Detects unformatted Rust files using `cargo fmt --check`.
//!
//! This runner is style-only (Severity::Low, Category::Style) and operates at
//! coarse granularity (whole project).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_cargo_fmt;
use super::{cap, run_capture_with_timeout, Granularity, RawFinding, RunnerOutcome};
use std::path::Path;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Coarse
}

/// Parse `cargo fmt --check` output.
///
/// The output format for each unformatted file is:
/// ```text
/// Diff in /abs/path/to/file.rs:LINE:
/// ... diff lines ...
/// ```
///
/// This parser extracts the file path and line number from the header line.
/// It skips malformed lines and diff body lines.
/// PRIVACY: Diff content is NOT included in findings to avoid leaking secrets.
pub fn parse_cargo_fmt(stdout: &str, root: &Path) -> Vec<RawFinding> {
    let mut findings = Vec::new();

    for line in stdout.lines() {
        // Header lines start with "Diff in " and end with ":"
        if line.starts_with("Diff in ") && line.ends_with(":") {
            // Extract path and line number
            // Format: "Diff in /path/to/file.rs:LINE:"
            let content = &line[8..line.len() - 1]; // Remove "Diff in " and trailing ":"

            // Find the last colon to separate path and line number
            if let Some(colon_pos) = content.rfind(':') {
                let path_str = &content[..colon_pos];
                let line_str = &content[colon_pos + 1..];

                if let Ok(line_num) = line_str.parse::<u32>() {
                    // Strip root prefix to make path relative
                    let relative_path = if let Some(root_str) = root.to_str() {
                        if let Some(stripped) = path_str.strip_prefix(root_str) {
                            // Normalize backslashes to forward slashes, then drop the
                            // separator left over from the root prefix.
                            stripped.replace('\\', "/").trim_start_matches('/').to_string()
                        } else {
                            path_str.replace('\\', "/")
                        }
                    } else {
                        path_str.replace('\\', "/")
                    };

                    let severity = severity_from_cargo_fmt().0;
                    let category = severity_from_cargo_fmt().1;
                    let source = "cargo-fmt";
                    let title = "rustfmt: file is not formatted";
                    let body = cap(
                        &format!(
                            "cargo fmt --check reports a formatting diff in {} at line {}. Run `cargo fmt` to fix.",
                            relative_path, line_num
                        ),
                        1000,
                    );

                    findings.push(RawFinding {
                        file: relative_path,
                        line: Some(line_num),
                        severity,
                        category,
                        source: source.to_string(),
                        title: title.to_string(),
                        body,
                    });
                }
            }
        }
    }

    findings
}

/// Run cargo fmt from the project root. Absent `cargo` → empty.
pub fn run(root: &Path) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("cargo") {
        return RunnerOutcome::Skipped;
    }
    let stdout = run_capture_with_timeout(
        "cargo",
        &["fmt", "--", "--check", "--color=never"],
        root,
        Duration::from_secs(120),
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_cargo_fmt(&s, root)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_single_header_line() {
        let stdout = "Diff in /proj/src/main.rs:3:\n@@ -1,2 +1,2 @@\n-let x = 1;\n+let x=1;\n";
        let root = Path::new("/proj");
        let findings = parse_cargo_fmt(stdout, root);

        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/main.rs");
        assert_eq!(f.line, Some(3));
        assert_eq!(f.severity, Severity::Low);
        assert_eq!(f.category, Category::Style);
        assert_eq!(f.source, "cargo-fmt");
        assert_eq!(f.title, "rustfmt: file is not formatted");
        assert!(f.body.contains("src/main.rs"));
        assert!(f.body.contains("3"));
    }

    #[test]
    fn strips_root_prefix() {
        let stdout = "Diff in /proj/src/lib.rs:10:\n";
        let root = Path::new("/proj");
        let findings = parse_cargo_fmt(stdout, root);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/lib.rs");
    }

    #[test]
    fn skips_malformed_and_diff_lines() {
        let stdout = "Some random line\nDiff in /proj/src/main.rs:5:\n@@ -1,1 +1,1 @@\n- old\n+ new\nDiff in /proj/src/main.rs:10:\n";
        let root = Path::new("/proj");
        let findings = parse_cargo_fmt(stdout, root);

        // Should only parse the two valid header lines
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].line, Some(5));
        assert_eq!(findings[1].line, Some(10));
    }

    #[test]
    fn empty_input_returns_empty() {
        let root = Path::new("/proj");
        let findings = parse_cargo_fmt("", root);
        assert!(findings.is_empty());
    }

    #[test]
    fn multiple_headers_yield_multiple_findings() {
        let stdout = "Diff in /proj/src/a.rs:1:\nDiff in /proj/src/b.rs:2:\n";
        let root = Path::new("/proj");
        let findings = parse_cargo_fmt(stdout, root);

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "src/a.rs");
        assert_eq!(findings[1].file, "src/b.rs");
    }
}