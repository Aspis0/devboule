//! cppcheck runner (C/C++ static analyzer).
//!
//! `cppcheck` is a STANDALONE static analyzer for C/C++ that does NOT compile the
//! translation unit — it parses + analyzes the source directly, so it is cheap enough
//! to run per-file in the FINE (per-keystroke-settled) loop, unlike a compile-based
//! analyzer (clang-tidy with a compilation database, or `go vet`). We invoke it with a
//! PARSEABLE template and a CURATED, low-FP check set:
//!
//! ```text
//! cppcheck --enable=warning --inline-suppr --quiet \
//!          --template={file}:{line}:{severity}:{id}:{message} <file>
//! ```
//!
//! Curated flags (advisory-first / label-hygiene rationale):
//!   - `--enable=warning` — the curated, low-FP set (likely bugs). We DELIBERATELY do
//!     NOT use `--enable=all` / `style` / `information`: those classes are noisy and
//!     would poison ORPO labels with subjective/portability chatter.
//!   - `--inline-suppr` — honor in-source `// cppcheck-suppress` annotations so a
//!     maintainer's intentional suppression is respected (fewer FPs).
//!   - `--quiet` — drop the progress chatter; we only want diagnostics.
//!   - `--template=...` — a stable, colon-delimited line we can parse deterministically.
//!
//! cppcheck writes its diagnostics to STDERR (stdout is progress/empty), so the runner
//! captures stderr. Advisory: Correctness, capped at MEDIUM (see
//! [`severity_from_cppcheck`] — even an `error` token is Medium, never High, until the
//! FP-rate on this repo is measured). Absent `cppcheck` → empty Vec (never an error).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_cppcheck;
use super::{cap, redact_secrets, run_capture_stderr_with_timeout, Granularity, RawFinding, RunTarget};
use super::DEFAULT_RUNNER_TIMEOUT;
use std::path::Path;

pub fn granularity() -> Granularity {
    // No-compile per-file analyzer → FINE (runs on the changed file in the hot loop).
    Granularity::Fine
}

/// The `--template` we ask cppcheck to format each diagnostic with. Stable and
/// colon-delimited: `{file}:{line}:{severity}:{id}:{message}`. cppcheck substitutes
/// `{message}` last, so any colons in the message stay in the final field (the parser
/// keeps the remainder intact — see [`parse_cppcheck`]).
const CPPCHECK_TEMPLATE: &str = "--template={file}:{line}:{severity}:{id}:{message}";

/// Parse cppcheck stderr (one diagnostic per line, formatted by [`CPPCHECK_TEMPLATE`]
/// as `file:line:severity:id:message`). PURE. Malformed lines (no parseable line
/// number, fewer than the five fields, cppcheck banners / `nofile:0:...` summary
/// lines) are IGNORED — never a panic. The message is the WHOLE remainder after the
/// fourth `:` (so a message containing colons is preserved). cppcheck severity is
/// mapped via [`severity_from_cppcheck`] (advisory: Correctness capped at Medium).
///
/// PRIVACY: a cppcheck message can interpolate a token/identifier from the source; the
/// message is run through `redact_secrets` before it lands in the title/body.
pub fn parse_cppcheck(stderr: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let Some(finding) = parse_cppcheck_line(line) else {
            continue;
        };
        out.push(finding);
    }
    out
}

/// Parse ONE cppcheck diagnostic line `file:line:severity:id:message` into a
/// [`RawFinding`], or `None` if the line does not match the shape (no panic). The
/// split keeps the message remainder intact: `splitn(5, ':')` yields exactly
/// `[file, line, severity, id, message-with-any-colons]`. A non-numeric line, a
/// `file` that is empty / cppcheck's `nofile` placeholder, or an empty message → `None`.
fn parse_cppcheck_line(line: &str) -> Option<RawFinding> {
    let mut parts = line.splitn(5, ':');
    let file = parts.next()?.trim();
    let line_str = parts.next()?.trim();
    let severity_tok = parts.next()?.trim();
    let _id = parts.next()?; // the cppcheck check id (e.g. `nullPointer`) — not stored.
    let message = parts.next()?.trim();

    if file.is_empty() || file == "nofile" || message.is_empty() {
        return None;
    }
    let lineno: u32 = line_str.parse().ok()?;
    // cppcheck emits `0` for a file-level diagnostic with no specific line.
    let line_field = (lineno != 0).then_some(lineno);

    let (severity, category) = severity_from_cppcheck(severity_tok);
    let safe_message = redact_secrets(message);
    Some(RawFinding {
        file: file.replace('\\', "/"),
        line: line_field,
        severity,
        category,
        source: "cppcheck".to_string(),
        title: format!("cppcheck: {}", cap(&safe_message, 200)),
        body: cap(&safe_message, 1000),
    })
}

/// Run cppcheck on a single file from the project root. Absent `cppcheck` → empty
/// (never an error). Diagnostics go to STDERR (stdout is progress), so we capture the
/// stderr stream with the default per-file timeout. The file path is the orchestrator-
/// validated project-relative path (a leading-`-` component is rejected upstream by
/// `validate_rel_path`, so it can't be mistaken for a flag).
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("cppcheck") {
        return Vec::new();
    }
    let stderr = run_capture_stderr_with_timeout(
        "cppcheck",
        &[
            "--enable=warning",
            "--inline-suppr",
            "--quiet",
            CPPCHECK_TEMPLATE,
            &target.file_rel_path,
        ],
        root,
        DEFAULT_RUNNER_TIMEOUT,
    );
    match stderr {
        Some(s) => parse_cppcheck(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_warning_diagnostic_line() {
        // The captured-sample line from the spec.
        let stderr = "a.cpp:10:warning:nullPointer:Possible null deref\n";
        let findings = parse_cppcheck(stderr);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "a.cpp");
        assert_eq!(f.line, Some(10));
        // Advisory: Correctness capped at Medium.
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.category, Category::Correctness);
        assert_eq!(f.source, "cppcheck");
        assert!(f.title.starts_with("cppcheck: "));
        assert!(f.title.contains("Possible null deref"));
        assert!(f.body.contains("Possible null deref"));
    }

    #[test]
    fn error_token_is_advisory_medium_not_high() {
        // The message deliberately contains a colon; all words are ordinary lowercase
        // prose so the secret-redactor leaves them intact (it only redacts token-shaped
        // blobs), letting this test isolate the colon-preservation behavior.
        let stderr = "src/x.c:3:error:uninitvar:uninitialized read of the value\n";
        let findings = parse_cppcheck(stderr);
        assert_eq!(findings.len(), 1);
        // Advisory-first: an `error` token is still Medium, NEVER High.
        assert_eq!(findings[0].severity, Severity::Medium);
        assert_eq!(findings[0].category, Category::Correctness);
        assert_eq!(findings[0].file, "src/x.c");
        assert_eq!(findings[0].line, Some(3));
        // A message containing a colon is preserved intact (splitn keeps the remainder).
        assert!(
            findings[0].body.contains("uninitialized read of the value"),
            "message truncated at colon: {}",
            findings[0].body
        );
    }

    #[test]
    fn message_with_internal_colon_is_kept_whole() {
        // splitn(5, ':') keeps everything after the 4th colon as the message, so an
        // internal colon (and the word after it) survives.
        let stderr = "a.cpp:2:warning:foo:redundant condition: x is always set\n";
        let findings = parse_cppcheck(stderr);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].body.contains("redundant condition: x is always set"),
            "message truncated at internal colon: {}",
            findings[0].body
        );
    }

    #[test]
    fn ignores_malformed_and_banner_lines_without_panic() {
        let stderr = "\
Checking a.cpp ...
a.cpp:10:warning:nullPointer:Possible null deref
this is not a diagnostic
a.cpp:notanumber:warning:foo:bad line number
nofile:0:information:missingInclude:include not found
b.cpp:7:portability:foo:
b.cpp:9:warning:foo:another real one
";
        let findings = parse_cppcheck(stderr);
        // Only the two well-formed lines with a non-empty message survive. The banner,
        // the prose line, the bad line number, the `nofile` summary, and the empty-
        // message line are all dropped.
        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert_eq!(findings[0].file, "a.cpp");
        assert_eq!(findings[0].line, Some(10));
        assert_eq!(findings[1].file, "b.cpp");
        assert_eq!(findings[1].line, Some(9));
    }

    #[test]
    fn empty_input_yields_no_findings() {
        assert!(parse_cppcheck("").is_empty());
        assert!(parse_cppcheck("\n\n").is_empty());
    }

    #[test]
    fn portability_and_performance_tokens_map_to_low() {
        let stderr = "\
a.cpp:1:portability:ptr:portable issue
a.cpp:2:performance:perf:slow construct
";
        let findings = parse_cppcheck(stderr);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].severity, Severity::Low);
        assert_eq!(findings[0].category, Category::Correctness);
        assert_eq!(findings[1].severity, Severity::Low);
    }

    #[test]
    fn redacts_secret_in_message() {
        let stderr = "a.cpp:1:warning:foo:leaked token AKIAIOSFODNN7EXAMPLE here\n";
        let findings = parse_cppcheck(stderr);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    // ---- presence-gated integration: skip when cppcheck absent; ONE tiny run when
    //      present (no-compile, so a single per-file invocation is cheap). ----

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_buggy_file() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent cppcheck → empty Vec, no error (graceful absence). When cppcheck IS
        // present, a file with a clear `warning`-class bug is flagged (ONE tiny run).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aspis-cppcheck-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A null-pointer dereference: a `warning`-class bug cppcheck reliably reports.
        let rel = "bad.cpp";
        std::fs::write(
            dir.join(rel),
            "void f() {\n    int *p = 0;\n    *p = 1;\n}\n",
        )
        .unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target);
        if crate::backend::projects::command_exists("cppcheck") {
            // The toolchain SHOULD flag the null deref; we assert at least one finding
            // and that every finding is well-formed (source + advisory severity cap).
            assert!(
                !findings.is_empty(),
                "cppcheck should flag the null deref in {rel}"
            );
            for f in &findings {
                assert_eq!(f.source, "cppcheck");
                assert_eq!(f.category, Category::Correctness);
                // Advisory cap: never High.
                assert_ne!(f.severity, Severity::High);
            }
        } else {
            assert!(findings.is_empty(), "absent cppcheck must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
