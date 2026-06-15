//! gofmt runner (Go formatter check).
//!
//! `gofmt -l <file>` lists the files that are NOT gofmt-formatted: it PRINTS the
//! file path (one per line) when the file would be reformatted, and prints NOTHING
//! when the file is already formatted. We run it per-file, so a non-empty stdout
//! means THIS file is unformatted → ONE advisory Style/Low finding; an empty stdout
//! means no finding.
//!
//! Tier-A-safe: `gofmt` is INSTANT (it parses + reformats in memory; it does NOT
//! compile or type-check), so it is cheap enough for the tight interactive FINE
//! loop. Style-only (Severity::Low, Category::Style) at Fine (per-file) granularity.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_gofmt;
use super::{cap, run_capture, Granularity, RawFinding, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// Parse `gofmt -l <file>` stdout. PURE. `gofmt -l` prints the path of every file
/// it would reformat; since we invoke it on a SINGLE file, ANY non-empty output
/// means that file is unformatted. We therefore emit exactly ONE finding when the
/// (trimmed) stdout is non-empty, attributed to `file_hint` (the project-relative
/// path we asked gofmt to check — we never trust the printed path, which gofmt
/// reports verbatim as we passed it). Empty/whitespace-only stdout → no finding.
///
/// PRIVACY: the diff/content is never requested (`-l` lists names only) and never
/// surfaced; the finding body is a fixed advisory message.
pub fn parse_gofmt(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    if stdout.trim().is_empty() {
        return Vec::new();
    }
    let (severity, category) = severity_from_gofmt();
    let file = file_hint.replace('\\', "/");
    let body = cap(
        &format!(
            "gofmt reports that {file} is not gofmt-formatted. Run `gofmt -w` to fix."
        ),
        1000,
    );
    vec![RawFinding {
        file,
        // gofmt -l reports at the file level (no line); a formatting diff spans the
        // whole file, so a single file-level finding is correct.
        line: None,
        severity,
        category,
        source: "gofmt".to_string(),
        title: "gofmt: file is not formatted".to_string(),
        body,
    }]
}

/// Run gofmt on a single file from the project root. Absent `gofmt` → empty (never
/// an error). `--` is NOT used (gofmt has no `--` end-of-flags marker); instead the
/// path is validated/normalized by the orchestrator before it reaches here, and a
/// leading-`-` component is rejected upstream by `validate_rel_path`.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("gofmt") {
        return Vec::new();
    }
    let stdout = run_capture("gofmt", &["-l", &target.file_rel_path], root);
    match stdout {
        Some(s) => parse_gofmt(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn non_empty_output_yields_one_style_low_finding() {
        // gofmt -l prints the path of the unformatted file.
        let findings = parse_gofmt("main.go\n", "main.go");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "main.go");
        assert_eq!(f.line, None);
        assert_eq!(f.severity, Severity::Low);
        assert_eq!(f.category, Category::Style);
        assert_eq!(f.source, "gofmt");
        assert_eq!(f.title, "gofmt: file is not formatted");
        assert!(f.body.contains("main.go"));
    }

    #[test]
    fn uses_file_hint_for_attribution() {
        // We attribute to the path we asked about, not the verbatim printed path.
        let findings = parse_gofmt("pkg/weird.go\n", "pkg/svc/handler.go");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "pkg/svc/handler.go");
    }

    #[test]
    fn empty_and_whitespace_output_yields_no_finding() {
        assert!(parse_gofmt("", "main.go").is_empty());
        assert!(parse_gofmt("   \n\t\n", "main.go").is_empty());
    }

    // ---- presence-gated integration: skip when gofmt absent; tiny run when present ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_unformatted_file() {
        use crate::backend::censor::runners::RunTarget;
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent gofmt → empty Vec, no error (graceful absence). When gofmt IS present,
        // a deliberately mis-indented file is reported as unformatted (ONE finding).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aspis-gofmt-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // gofmt wants tab indentation; spaces + extra blank lines are not gofmt-clean.
        let rel = "bad.go";
        std::fs::write(
            dir.join(rel),
            "package main\nfunc  main()  {\n        x := 1\n_ = x\n}\n",
        )
        .unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target);
        if crate::backend::projects::command_exists("gofmt") {
            assert_eq!(findings.len(), 1, "gofmt should flag the unformatted file");
            assert_eq!(findings[0].source, "gofmt");
            assert_eq!(findings[0].severity, Severity::Low);
            assert_eq!(findings[0].category, Category::Style);
        } else {
            assert!(findings.is_empty(), "absent gofmt must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
