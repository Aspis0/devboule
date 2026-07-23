//! jscpd runner (copy/paste detector).
//!
//! jscpd's json reporter emits `{ "duplicates": [ { "firstFile": {name,start,end},
//! "secondFile": {name,start,end}, "lines", "fragment" } ], "statistics": {...} }`.
//! We surface one finding per duplicate, anchored to `firstFile`, and DROP the
//! `fragment` field (the duplicated source — not declared, so serde discards it;
//! avoids echoing arbitrary code into a shard). Severity/category via
//! `jscpd_category` (Medium Duplication). Granularity is Fine.
//!
//! Note: jscpd writes its JSON report to a file by default; A3 reads that file's
//! contents and passes the string here. `parse_jscpd` is the testable core.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::jscpd_category;
use super::{run_capture, Granularity, RawFinding, RunnerOutcome};
use serde::Deserialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub fn granularity() -> Granularity {
    Granularity::Fine
}

#[derive(Deserialize)]
struct JscpdReport {
    #[serde(default)]
    duplicates: Vec<JscpdDuplicate>,
}

#[derive(Deserialize)]
struct JscpdDuplicate {
    #[serde(default, rename = "firstFile")]
    first_file: Option<JscpdFileRef>,
    #[serde(default, rename = "secondFile")]
    second_file: Option<JscpdFileRef>,
    #[serde(default)]
    lines: Option<u32>,
    // `fragment` (the duplicated source) is deliberately NOT declared → dropped.
}

#[derive(Deserialize)]
struct JscpdFileRef {
    #[serde(default)]
    name: String,
    #[serde(default)]
    start: Option<u32>,
}

/// Parse jscpd json-report contents. PURE. One finding per duplicate, anchored to
/// the first file's start line. The duplicated source fragment is never read.
/// Tolerant: malformed JSON → empty.
pub fn parse_jscpd(report_json: &str) -> Vec<RawFinding> {
    let report: JscpdReport = match serde_json::from_str(report_json.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let (severity, category) = jscpd_category();
    let mut out = Vec::new();
    for dup in report.duplicates {
        let first = match dup.first_file {
            Some(f) if !f.name.is_empty() => f,
            _ => continue,
        };
        let file = first.name.replace('\\', "/");
        let lines = dup.lines.map(|n| n.to_string()).unwrap_or_default();
        let second_name = dup
            .second_file
            .as_ref()
            .map(|s| s.name.replace('\\', "/"))
            .unwrap_or_default();
        let title = if lines.is_empty() {
            "Duplicated code block".to_string()
        } else {
            format!("Duplicated code ({lines} lines)")
        };
        let body = if second_name.is_empty() {
            format!("Duplicate block at {file}")
        } else {
            format!("{file} duplicates {second_name}")
        };
        out.push(RawFinding {
            file,
            line: first.start,
            severity,
            category,
            source: "jscpd".to_string(),
            title,
            body,
        });
    }
    out
}

/// Per-process monotonic counter for unique output dirs (mirrors the ledger /
/// detect-test convention; collision-free with `process::id()`).
static JSCPD_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Run jscpd from the project root. Absent `jscpd` → empty.
///
/// CONTRACT: jscpd's `json` reporter writes its report to a FILE, not stdout —
/// `<output_dir>/jscpd-report.json`. (Without `--output` it defaults to
/// `<cwd>/report/jscpd-report.json`.) Relying on stdout silently produced ZERO
/// findings. We therefore point `--output` at a UNIQUE temp dir under the system
/// temp, run jscpd, read the produced `jscpd-report.json`, parse it, and clean the
/// temp dir up afterwards (best-effort). Using the system temp (not a dir under
/// the watched root) keeps the report out of the project tree so the file watcher
/// never re-triggers on it.
pub fn run(root: &Path) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("jscpd") {
        return RunnerOutcome::Skipped;
    }
    let n = JSCPD_COUNTER.fetch_add(1, Ordering::Relaxed);
    let out_dir =
        std::env::temp_dir().join(format!("aspis-censor-jscpd-{}-{n}", std::process::id()));
    // Fresh dir; ignore a pre-existing one (unique name makes this unlikely).
    let _ = std::fs::remove_dir_all(&out_dir);
    if std::fs::create_dir_all(&out_dir).is_err() {
        return RunnerOutcome::Failed;
    }
    let out_dir_str = out_dir.to_string_lossy().into_owned();
    // `--silent` suppresses the human report; `--reporters json` + `--output <dir>`
    // writes `<dir>/jscpd-report.json`. We scan the project root (`.`). stdout is
    // ignored — the report file is the source of truth.
    let _ = run_capture(
        "jscpd",
        &[
            "--silent",
            "--reporters",
            "json",
            "--output",
            &out_dir_str,
            ".",
        ],
        root,
    );
    let report_path = out_dir.join("jscpd-report.json");
    let findings = match std::fs::read_to_string(&report_path) {
        Ok(json) => parse_jscpd(&json),
        Err(_) => return { let _ = std::fs::remove_dir_all(&out_dir); RunnerOutcome::Failed },
    };
    // Clean up the temp report dir (best-effort; never fatal).
    let _ = std::fs::remove_dir_all(&out_dir);
    RunnerOutcome::Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_duplicate_and_drops_fragment() {
        let json = r#"{
          "duplicates": [
            {
              "firstFile": {"name":"src/a.ts","start":10,"end":25},
              "secondFile": {"name":"src/b.ts","start":40,"end":55},
              "lines": 15,
              "fragment": "const SECRET_LOOKING_CODE = 'do-not-leak';"
            }
          ],
          "statistics": {"total": {"duplicates": 1}}
        }"#;
        let findings = parse_jscpd(json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/a.ts");
        assert_eq!(f.line, Some(10));
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.category, Category::Duplication);
        assert_eq!(f.source, "jscpd");
        assert!(f.title.contains("15 lines"));
        assert!(f.body.contains("src/b.ts"));
        // The fragment source is never echoed.
        assert!(!f.body.contains("do-not-leak"));
        assert!(!f.title.contains("do-not-leak"));
    }

    #[test]
    fn skips_duplicate_without_first_file() {
        let json = r#"{"duplicates":[{"lines":5}]}"#;
        assert!(parse_jscpd(json).is_empty());
    }

    #[test]
    fn empty_and_malformed_yield_empty() {
        assert!(parse_jscpd(r#"{"duplicates":[]}"#).is_empty());
        assert!(parse_jscpd("not json").is_empty());
        assert!(parse_jscpd("").is_empty());
    }
}
