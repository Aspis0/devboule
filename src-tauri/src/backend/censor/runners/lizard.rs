//! lizard runner (cyclomatic complexity).
//!
//! lizard's `--csv` output is one row per function:
//!   `nloc,ccn,token_count,param_count,length,location,file,function,signature,start_line,end_line`
//! We read `ccn` (column 1), `file` (column 6), `function` (column 7), and
//! `start_line` (column 9). A function whose CCN exceeds the threshold becomes a
//! Medium/Complexity finding (via `lizard_complexity`). Granularity is Fine.
//!
//! The default threshold here is 15 (a common ceiling); A3 may make it
//! configurable later. The parser is parameterized on the threshold for testing.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::lizard_complexity;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// Default cyclomatic-complexity threshold (CCN). Functions strictly above this
/// are reported.
pub const DEFAULT_CCN_THRESHOLD: u32 = 15;

/// Parse lizard `--csv` output with the given CCN threshold. PURE. Each row is
/// split on commas; a quoted signature field may itself contain commas, so we take
/// fields by FIXED leading index (nloc, ccn, ...) and never rely on the trailing
/// columns. Rows that don't parse (header, blank, too few columns) are skipped.
pub fn parse_lizard(stdout: &str, threshold: u32) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Split into the leading fixed columns. We only need indices 1 (ccn),
        // 6 (file), 7 (function), 9 (start_line). A signature with embedded commas
        // would corrupt indices >=8, so we parse start_line defensively and
        // tolerate its absence.
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 8 {
            continue;
        }
        let ccn: u32 = match cols[1].trim().parse() {
            Ok(n) => n,
            Err(_) => continue, // header row ("CCN") or malformed → skip
        };
        let (severity, category) = match lizard_complexity(ccn, threshold) {
            Some(v) => v,
            None => continue, // below threshold → no finding
        };
        let file = cols[6].trim().trim_matches('"').replace('\\', "/");
        if file.is_empty() {
            continue;
        }
        // The function name is a structured source identifier, but per the
        // conservative fail-closed stance (MINOR 1, defense-in-depth) it still goes
        // through `redact_secrets` before `cap()`: a secret-shaped identifier (e.g. a
        // function literally named `AKIAIOSFODNN7EXAMPLE`) must never reach a shard
        // title/body. Ordinary identifiers are left intact by the redactor's
        // heuristics. (The numeric CCN is NOT redacted — it is not free text.)
        let function = cols[7].trim().trim_matches('"');
        // start_line / end_line are the LAST two columns. A quoted signature
        // (column 8) may contain commas that shift every index after it, so we
        // anchor start_line from the END (second-to-last column) rather than a
        // fixed forward index. `cols.len() >= 8` is guaranteed above, so the
        // second-to-last index is always valid; fall back to None if it doesn't
        // parse.
        let line_no: Option<u32> = cols[cols.len() - 2].trim().parse().ok();
        let func_label = if function.is_empty() {
            "function".to_string()
        } else {
            cap(&redact_secrets(function), 80)
        };
        out.push(RawFinding {
            file,
            line: line_no,
            severity,
            category,
            source: "lizard".to_string(),
            title: format!("High complexity (CCN {ccn}) in {func_label}"),
            body: format!(
                "Cyclomatic complexity {ccn} exceeds threshold {threshold} in {func_label}"
            ),
        });
    }
    out
}

/// Run lizard on a single file from the project root, CSV output, default
/// threshold. Absent `lizard` → empty. `--` ends flag parsing so a `-`-leading
/// file name is never read as an option.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("lizard") {
        return Vec::new();
    }
    let stdout = run_capture("lizard", &["--csv", "--", &target.file_rel_path], root);
    match stdout {
        Some(s) => parse_lizard(&s, DEFAULT_CCN_THRESHOLD),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn reports_function_over_threshold() {
        // nloc,ccn,token,param,length,location,file,function,signature,start,end
        let csv = "30,20,150,3,40,\"big@1-40@src/a.py\",src/a.py,big,\"big(a, b, c)\",1,40";
        let findings = parse_lizard(csv, 15);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/a.py");
        assert_eq!(f.line, Some(1));
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.category, Category::Complexity);
        assert_eq!(f.source, "lizard");
        assert!(f.title.contains("CCN 20"));
        assert!(f.title.contains("big"));
    }

    #[test]
    fn skips_function_below_threshold() {
        let csv = "10,5,40,1,12,\"small@1-12@a.py\",a.py,small,\"small(x)\",1,12";
        assert!(parse_lizard(csv, 15).is_empty());
    }

    #[test]
    fn skips_header_and_malformed_rows() {
        let csv = "\
NLOC,CCN,token,PARAM,length,location,file,function,signature,start,end
not,enough
20,18,100,2,30,\"f@1@a.rs\",a.rs,f,\"f()\",1,30";
        let findings = parse_lizard(csv, 15);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "a.rs");
    }

    #[test]
    fn empty_yields_empty() {
        assert!(parse_lizard("", 15).is_empty());
    }

    #[test]
    fn secret_shaped_function_name_is_redacted() {
        // MINOR 1 (defense-in-depth): a function literally named like an AWS access
        // key must be redacted in the title/body before it reaches a shard. The
        // numeric CCN is preserved (it is not free text).
        let csv = "30,20,150,3,40,\"x@1-40@a.py\",a.py,AKIAIOSFODNN7EXAMPLE,\"x()\",1,40";
        let findings = parse_lizard(csv, 15);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(
            !f.title.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret-shaped name leaked into title"
        );
        assert!(
            !f.body.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret-shaped name leaked into body"
        );
        assert!(
            f.title.contains("[redacted]"),
            "title carries the redaction marker"
        );
        assert!(
            f.title.contains("CCN 20"),
            "the numeric CCN is preserved (not redacted)"
        );
    }
}
