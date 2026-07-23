//! ruff format runner.
//!
//! Runs `ruff format --check` on a single Python file. An unformatted file
//! prints a `Would reformat: <path>` line; a formatted file prints nothing
//! relevant. Style-only: Severity::Low, Category::Style (shared
//! `severity_from_format_checker`). At most ONE finding per run (the runner is
//! invoked per file). PRIVACY: no tool text is echoed into the finding.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_format_checker;
use super::{cap, run_capture, Granularity, RawFinding, RunnerOutcome, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// Parse `ruff format --check` output. PURE. Emits AT MOST ONE finding: the
/// runner targets a single file, so any `Would reformat: ` line means that
/// file needs formatting.
pub fn parse_ruff_format(stdout: &str, file_rel: &str) -> Vec<RawFinding> {
    let mut findings = Vec::new();
    for line in stdout.lines() {
        if line.starts_with("Would reformat: ") {
            let (severity, category) = severity_from_format_checker();
            let body = format!(
                "ruff format --check would reformat {file_rel}. Run `ruff format` to fix."
            );
            findings.push(RawFinding {
                file: file_rel.to_string(),
                line: None,
                severity,
                category,
                source: "ruff-format".to_string(),
                title: "ruff format: file is not formatted".to_string(),
                body: cap(&body, 1000),
            });
            // One finding per run: the target is a single file.
            break;
        }
    }
    findings
}

/// Run ruff format --check on a single file from the project root. Absent
/// `ruff` → empty. `--` ends flag parsing so a file whose name begins with `-`
/// is never interpreted as an option.
pub fn run(root: &Path, target: &RunTarget) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("ruff") {
        return RunnerOutcome::Skipped;
    }
    let stdout = run_capture(
        "ruff",
        &["format", "--check", "--", &target.file_rel_path],
        root,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_ruff_format(&s, &target.file_rel_path)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_would_reformat_line() {
        let stdout = "Would reformat: src/main.py\n1 file would be reformatted\n";
        let findings = parse_ruff_format(stdout, "src/main.py");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/main.py");
        assert_eq!(f.line, None);
        assert_eq!(f.severity, Severity::Low);
        assert_eq!(f.category, Category::Style);
        assert_eq!(f.source, "ruff-format");
        assert_eq!(f.title, "ruff format: file is not formatted");
        assert!(f.body.contains("src/main.py"));
    }

    #[test]
    fn at_most_one_finding() {
        let stdout = "Would reformat: src/main.py\nWould reformat: src/other.py\n";
        let findings = parse_ruff_format(stdout, "src/main.py");
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn formatted_file_yields_empty() {
        let stdout = "1 file already formatted\n";
        assert!(parse_ruff_format(stdout, "src/main.py").is_empty());
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_ruff_format("", "src/main.py").is_empty());
    }
}
