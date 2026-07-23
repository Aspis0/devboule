//! prettier --check runner.
//!
//! Runs `prettier --check` on a single TS/JS file. An unformatted file prints
//! a `[warn] <path>` line; prettier ALSO prints a `[warn] Code style issues
//! found…` summary sentence, so the parser only accepts the `[warn]` line whose
//! path (backslash-normalized) equals the target file — never the summary.
//! Style-only: Severity::Low, Category::Style (shared
//! `severity_from_format_checker`). At most ONE finding per run. PRIVACY: no
//! tool text is echoed into the finding.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_format_checker;
use super::{cap, run_capture, Granularity, RawFinding, RunnerOutcome, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// Parse `prettier --check` output. PURE. A `[warn] ` line counts only when its
/// remainder (backslash-normalized) equals the target file's rel path — this
/// rejects prettier's `[warn] Code style issues found…` summary sentence.
pub fn parse_prettier(stdout: &str, file_rel: &str) -> Vec<RawFinding> {
    let mut findings = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("[warn] ") {
            if rest.trim().replace('\\', "/") != file_rel {
                continue;
            }
            let (severity, category) = severity_from_format_checker();
            let body = format!(
                "prettier --check reports {file_rel} as unformatted. Run `prettier --write` to fix."
            );
            findings.push(RawFinding {
                file: file_rel.to_string(),
                line: None,
                severity,
                category,
                source: "prettier".to_string(),
                title: "prettier: file is not formatted".to_string(),
                body: cap(&body, 1000),
            });
            // One finding per run: the target is a single file.
            break;
        }
    }
    findings
}

/// Run prettier --check on a single file from the project root. Absent
/// `prettier` → empty. `--` ends flag parsing so a file whose name begins with
/// `-` is never interpreted as an option.
pub fn run(root: &Path, target: &RunTarget) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("prettier") {
        return RunnerOutcome::Skipped;
    }
    let stdout = run_capture(
        "prettier",
        &["--check", "--", &target.file_rel_path],
        root,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_prettier(&s, &target.file_rel_path)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_warn_line_for_target_file() {
        let stdout = "Checking formatting...\n[warn] src/index.ts\n[warn] Code style issues found in the above file. Run Prettier with --write to fix.\n";
        let findings = parse_prettier(stdout, "src/index.ts");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/index.ts");
        assert_eq!(f.line, None);
        assert_eq!(f.severity, Severity::Low);
        assert_eq!(f.category, Category::Style);
        assert_eq!(f.source, "prettier");
        assert_eq!(f.title, "prettier: file is not formatted");
        assert!(f.body.contains("src/index.ts"));
    }

    #[test]
    fn summary_sentence_is_never_a_finding() {
        let stdout = "[warn] Code style issues found in the above file. Run Prettier with --write to fix.\n";
        assert!(parse_prettier(stdout, "src/index.ts").is_empty());
    }

    #[test]
    fn other_files_warn_lines_are_ignored() {
        let stdout = "[warn] src/a.ts\n[warn] src/b.js\n";
        assert!(parse_prettier(stdout, "src/index.ts").is_empty());
    }

    #[test]
    fn backslash_paths_are_normalized() {
        let stdout = "[warn] src\\index.ts\n";
        let findings = parse_prettier(stdout, "src/index.ts");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/index.ts");
    }

    #[test]
    fn formatted_and_empty_yield_empty() {
        let ok = "Checking formatting...\nAll matched files use Prettier code style!\n";
        assert!(parse_prettier(ok, "src/index.ts").is_empty());
        assert!(parse_prettier("", "src/index.ts").is_empty());
    }
}
