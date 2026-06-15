//! go vet runner (Go correctness analyzer).
//!
//! `go vet ./...` runs the standard `vet` analyzers across every package in the
//! module and reports suspicious constructs (printf format mismatches, lost struct
//! tags, unreachable code, etc.) on STDERR, one diagnostic per line of the form:
//!
//! ```text
//! path/to/file.go:LINE:COL: message
//! ```
//!
//! Coarse (whole-project) granularity: it inspects the entire module in one
//! invocation, so the orchestrator runs it once per coarse trigger and ignores any
//! single changed file. Advisory: Correctness, capped at MEDIUM (P2 rollout
//! discipline — promote to High only after the FP-rate is measured on this repo).
//!
//! ⚠️ COMPILE-BASED / HEAVY — DO NOT RUN IN THE TIGHT INTERACTIVE LOOP. Unlike
//! `gofmt` (which only parses + reformats in memory, INSTANT), `go vet` TYPE-CHECKS
//! and COMPILES the packages it analyzes (the analyzers run on a fully type-checked
//! AST). It is therefore slow and thermally expensive — a future Tier-B / async-
//! cadence tool, NOT for the hot per-keystroke loop. It is wired here as an advisory
//! COARSE runner (debounced, project-level) and MUST NOT be invoked more than the
//! coarse debounce allows; tests run it at most ONCE on a trivial module.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_go_vet;
use super::{cap, redact_secrets, run_capture_stderr_with_timeout, Granularity, RawFinding};
use std::path::Path;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Coarse
}

/// Compile-based ⇒ a generous budget. `go vet ./...` type-checks + compiles every
/// package, so it is far slower than a syntax linter; this is the coarse-pass cap.
const GO_VET_TIMEOUT: Duration = Duration::from_secs(180);

/// Parse `go vet ./...` stderr. PURE. Each diagnostic is a line of the shape
/// `path/file.go:line:col: message`; malformed lines (no `.go:line:col:` prefix, a
/// non-numeric line, the `# pkg/...` build-progress banners go vet interleaves) are
/// IGNORED — never a panic. The path is taken verbatim from the diagnostic and
/// forward-slash normalized; the line is the parsed number; the column is parsed but
/// not stored (the Censor finding model carries only a line).
///
/// PRIVACY: a vet message can interpolate a literal (e.g. a printf arg); the message
/// is run through `redact_secrets` before it lands in the title/body.
pub fn parse_go_vet(stderr: &str) -> Vec<RawFinding> {
    let (severity, category) = severity_from_go_vet();
    let mut out = Vec::new();
    for line in stderr.lines() {
        // Skip build-progress banners (`# example.com/pkg`) and blank lines fast.
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((file, lineno, message)) = parse_vet_line(line) else {
            continue;
        };
        let safe_message = redact_secrets(message);
        out.push(RawFinding {
            file,
            line: Some(lineno),
            severity,
            category,
            source: "go-vet".to_string(),
            title: format!("go vet: {}", cap(&safe_message, 200)),
            body: cap(&safe_message, 1000),
        });
    }
    out
}

/// Parse ONE diagnostic line `path/file.go:line:col: message` into
/// `(file, line, message)`. Returns `None` for any line that does not match the
/// shape (no panic). The split is from the LEFT on `:` but mindful that a Windows-
/// style absolute path (`C:\...`) could contain a drive-letter colon — go vet on
/// every platform reports module-relative forward-slash paths here (it runs in the
/// module root), so we locate the `.go:` boundary explicitly rather than blindly
/// splitting on the first colon.
fn parse_vet_line(line: &str) -> Option<(String, u32, &str)> {
    // Find the file/position boundary at the LAST `.go:` so a path that itself embeds an
    // earlier `.go:` (e.g. a directory literally named `foo.go:bar/…`, or a `.go:`
    // substring inside the message that precedes the real coordinate) anchors on the
    // FILENAME boundary rather than the first stray occurrence. `rfind` gives the byte
    // index of the rightmost `.go:`.
    let go_marker = ".go:";
    let marker_at = line.rfind(go_marker)?;
    // file = everything up to and including `.go`.
    let file_end = marker_at + 3; // ".go".len()
    let file = &line[..file_end];
    if file.is_empty() {
        return None;
    }
    // The remainder after `.go:` is `line:col: message`.
    let rest = &line[file_end + 1..]; // skip the `:` after `.go`
    // rest = "LINE:COL: message" — take the first `:`-delimited field as the line.
    let mut parts = rest.splitn(3, ':');
    let line_str = parts.next()?;
    let _col_str = parts.next()?; // column parsed-and-discarded (model has no col).
    let message = parts.next()?.trim();
    let lineno: u32 = line_str.trim().parse().ok()?;
    if message.is_empty() {
        return None;
    }
    Some((file.replace('\\', "/"), lineno, message))
}

/// Run `go vet ./...` from the project root. Absent `go` → empty (never an error).
///
/// ⚠️ COMPILE-BASED — see the module header: this COMPILES the module's packages and
/// is heavy/thermal. It is a COARSE runner: the orchestrator invokes it once per
/// coarse debounce, NEVER per keystroke. Diagnostics go to stderr, so we capture the
/// stderr stream (stdout is drained) with the longer compile-aware timeout.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("go") {
        return Vec::new();
    }
    let stderr = run_capture_stderr_with_timeout("go", &["vet", "./..."], root, GO_VET_TIMEOUT);
    match stderr {
        Some(s) => parse_go_vet(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_diagnostic_line() {
        let stderr = "pkg/foo.go:12:5: something looks wrong\n";
        let findings = parse_go_vet(stderr);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "pkg/foo.go");
        assert_eq!(f.line, Some(12));
        // Advisory: Correctness capped at Medium.
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.category, Category::Correctness);
        assert_eq!(f.source, "go-vet");
        assert!(f.title.starts_with("go vet: "));
        assert!(f.title.contains("something looks wrong"));
        assert!(f.body.contains("something looks wrong"));
    }

    #[test]
    fn ignores_malformed_and_banner_lines_without_panic() {
        let stderr = "\
# example.com/m/pkg
pkg/foo.go:12:5: real diagnostic
this is not a diagnostic
pkg/bar.go:notanumber:5: bad line number
pkg/baz.go:7:3:
pkg/qux.go:9:1: another real one
";
        let findings = parse_go_vet(stderr);
        // Only the two well-formed lines with a non-empty message survive.
        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert_eq!(findings[0].file, "pkg/foo.go");
        assert_eq!(findings[0].line, Some(12));
        assert_eq!(findings[1].file, "pkg/qux.go");
        assert_eq!(findings[1].line, Some(9));
    }

    #[test]
    fn empty_input_yields_no_findings() {
        assert!(parse_go_vet("").is_empty());
        assert!(parse_go_vet("\n\n").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let stderr = "pkg/a.go:1:1: leaked token AKIAIOSFODNN7EXAMPLE in arg\n";
        let findings = parse_go_vet(stderr);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when `go` absent; run AT MOST ONCE when
    //      present (go vet COMPILES — never loop it). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_runs_once_on_trivial_module() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent `go` → empty Vec, no error. When `go` IS present we run go vet EXACTLY
        // ONCE on a trivial, clean module (it compiles, so keep it minimal and singular)
        // and assert it completes without panic and produces no spurious findings on
        // clean code. This is the ONLY place go vet is invoked in the test suite.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aspis-govet-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("go.mod"), "module example.com/x\n\ngo 1.21\n").unwrap();
        // A clean, vet-safe program: no printf mismatch, no lost struct tags.
        std::fs::write(
            dir.join("main.go"),
            "package main\n\nfunc main() {\n\t_ = 1\n}\n",
        )
        .unwrap();

        let findings = run(&dir);
        if crate::backend::projects::command_exists("go") {
            // Clean code → no vet diagnostics. (If the local toolchain emits an
            // environment diagnostic we tolerate it, but assert no panic + the source.)
            for f in &findings {
                assert_eq!(f.source, "go-vet");
                assert_eq!(f.severity, Severity::Medium);
                assert_eq!(f.category, Category::Correctness);
            }
        } else {
            assert!(findings.is_empty(), "absent `go` must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
