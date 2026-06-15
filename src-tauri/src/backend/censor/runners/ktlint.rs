//! ktlint runner (Kotlin formatter/linter).
//!
//! `ktlint` is a Kotlin STYLE/format linter (a JVM tool, but it analyzes the source
//! directly — it does NOT compile the target), cheap enough to run per-file in the FINE
//! (per-keystroke-settled) loop. We invoke it with the project-relative path FROM the
//! project root (cwd=root), so its default reporter echoes that same relative path:
//!
//! ```text
//! ktlint <file>      # spawned with cwd = project root
//! ```
//!
//! We deliberately do NOT pass `--relative`: that flag only exists on ktlint 0.48+. On an
//! older install it is an UNKNOWN argument → ktlint errors out with empty stdout and a
//! non-zero exit, which our capture helper turns into `None` → an empty `Vec`. That would
//! make a WHOLE Kotlin project silently report zero findings (a false all-clear). Instead
//! we stay version-independent: invoking with the relative path under cwd=root already
//! makes ktlint echo a project-relative path on every version, and [`relativize_file`]
//! defensively strips a `root` prefix should a version emit an absolute path anyway. The
//! parser is tolerant of BOTH absolute and relative `file` fields.
//!
//! The default reporter prints one diagnostic per line on STDOUT, in the form:
//!
//! ```text
//! file:line:col: message (rule-id)
//! ```
//!
//! ktlint EXITS NON-ZERO when it finds style issues, which is NORMAL — our capture helper
//! returns stdout regardless of exit code, so a non-zero exit with diagnostics is parsed
//! normally. Advisory: Style, always LOW (see [`severity_from_ktlint`]) — ktlint is a
//! formatting/style tool, like gofmt / cargo fmt / prettier. Absent `ktlint` → empty Vec
//! (never an error).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_ktlint;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunTarget};
use std::path::Path;

pub fn granularity() -> Granularity {
    // Style linter that analyzes the source (no compile-of-target) → FINE.
    Granularity::Fine
}

/// Parse ktlint stdout (one diagnostic per line, of the form
/// `file:line:col: message (rule-id)`). PURE. Lines that don't match the shape (a
/// summary line, blank lines, an info banner) are IGNORED — never a panic. Every ktlint
/// finding is Style/Low (see [`severity_from_ktlint`]). The message remainder is kept
/// intact (a message containing a colon survives — we split only the leading
/// `file:line:col:` fields).
///
/// Each finding's `file` is made PROJECT-RELATIVE via [`relativize_file`] against `root`:
/// an absolute path ktlint may emit (on a version that ignores or lacks the relative
/// reporter behavior) has the `root` prefix stripped; an already-relative path is left
/// unchanged. This keeps the output version-independent without relying on `--relative`.
///
/// PRIVACY: a ktlint message can interpolate an identifier from the source; the message
/// is run through `redact_secrets` before it lands in the title/body.
pub fn parse_ktlint(stdout: &str, root: &Path) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(mut finding) = parse_ktlint_line(line) {
            finding.file = relativize_file(&finding.file, root);
            out.push(finding);
        }
    }
    out
}

/// Make a ktlint-reported `file` PROJECT-RELATIVE against `root`, forward-slash
/// normalized. If `file` is absolute and lives under `root`, the `root` prefix is
/// stripped (`Path::strip_prefix` matches on OS path components, so it is correct on
/// both `/` and `\` platforms); an already-relative path — the common case when ktlint
/// is invoked with the relative path under cwd=root — is returned unchanged. A path we
/// cannot relativize (absolute but outside `root`, e.g. a symlinked checkout) is left as
/// ktlint reported it rather than guessed at. The result is always forward-slash
/// normalized so it matches the rest of the Censor's path convention.
fn relativize_file(file: &str, root: &Path) -> String {
    let path = Path::new(file);
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Parse ONE ktlint diagnostic line `file:line:col: message (rule-id)` into a
/// [`RawFinding`], or `None` if the line does not match the shape (no panic). We split
/// off exactly the three leading colon-delimited fields (`file`, `line`, `col`) and keep
/// the WHOLE remainder as the message (so an internal colon in the message survives). A
/// non-numeric line/col, an empty file, or an empty message → `None`. The `file` is kept
/// VERBATIM here (it can be absolute OR relative depending on the ktlint version); the
/// caller [`parse_ktlint`] relativizes and forward-slash normalizes it via
/// [`relativize_file`], so the raw OS separators are preserved for `strip_prefix` to
/// match `root`.
///
/// NOTE: a Windows absolute path has a drive-letter colon (`C:\dir\A.kt:3:1: msg`) which
/// `splitn(4, ':')` would split inside the drive letter, corrupting the path. ktlint is
/// invoked with the project-RELATIVE path under cwd=root, so it echoes a relative path
/// (no drive colon) on every platform — this parser is reached with a drive-letter path
/// only if a future version emits one, an accepted best-effort limitation documented here.
fn parse_ktlint_line(line: &str) -> Option<RawFinding> {
    // splitn(4, ':') → [file, line, col, " message (rule-id)"]
    let mut parts = line.splitn(4, ':');
    let file = parts.next()?.trim();
    let line_str = parts.next()?.trim();
    let col_str = parts.next()?.trim();
    let message = parts.next()?.trim();

    if file.is_empty() || message.is_empty() {
        return None;
    }
    let lineno: u32 = line_str.parse().ok()?;
    // The column field must be numeric too (guards against matching an unrelated line
    // that merely happens to contain colons). We don't store the column.
    let _col: u32 = col_str.parse().ok()?;
    let line_field = (lineno != 0).then_some(lineno);

    let (severity, category) = severity_from_ktlint();
    let safe_message = redact_secrets(message);
    Some(RawFinding {
        // Kept VERBATIM (OS separators intact) so `parse_ktlint` can `strip_prefix(root)`;
        // forward-slash normalization happens there in `relativize_file`.
        file: file.to_string(),
        line: line_field,
        severity,
        category,
        source: "ktlint".to_string(),
        title: format!("ktlint: {}", cap(&safe_message, 200)),
        body: cap(&safe_message, 1000),
    })
}

/// Run ktlint on a single file from the project root. Absent `ktlint` → empty (never an
/// error). The default reporter writes to STDOUT; ktlint exits non-zero when it finds
/// issues, but [`run_capture`] returns stdout regardless of exit code, so the diagnostics
/// are still captured.
///
/// We pass ONLY the project-relative path (no `--relative` flag — see the module doc:
/// that flag is 0.48+ and would make an older ktlint error out to an empty Vec, silently
/// zeroing a whole project's findings). Spawning from `root` makes ktlint echo a
/// project-relative path on every version, and [`parse_ktlint`] relativizes against `root`
/// defensively. The file path is the orchestrator-validated project-relative path (a
/// leading-`-` component is rejected upstream by `validate_rel_path`, so it can't be
/// mistaken for a flag).
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("ktlint") {
        return Vec::new();
    }
    let stdout = run_capture("ktlint", &[&target.file_rel_path], root);
    match stdout {
        Some(s) => parse_ktlint(&s, root),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_style_diagnostic_line() {
        // The captured-sample line from the spec.
        let stdout = "Main.kt:3:1: Unexpected indentation (indent)\n";
        let findings = parse_ktlint(stdout, Path::new("/proj"));
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "Main.kt");
        assert_eq!(f.line, Some(3));
        // ktlint is a style/formatting tool → Style/Low (advisory).
        assert_eq!(f.severity, Severity::Low);
        assert_eq!(f.category, Category::Style);
        assert_eq!(f.source, "ktlint");
        assert!(f.title.starts_with("ktlint: "));
        assert!(f.title.contains("Unexpected indentation"));
        assert!(f.body.contains("Unexpected indentation (indent)"));
    }

    #[test]
    fn message_with_internal_colon_is_kept_whole() {
        // splitn(4, ':') keeps everything after the 3rd colon as the message, so an
        // internal colon (and the words after it) survive.
        let stdout = "src/A.kt:10:5: needs newline before: brace (curly-spacing)\n";
        let findings = parse_ktlint(stdout, Path::new("/proj"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/A.kt");
        assert_eq!(findings[0].line, Some(10));
        assert!(
            findings[0].body.contains("needs newline before: brace"),
            "message truncated at internal colon: {}",
            findings[0].body
        );
    }

    #[test]
    fn ignores_malformed_and_summary_lines_without_panic() {
        let stdout = "\
Main.kt:3:1: Unexpected indentation (indent)
this is not a diagnostic
Main.kt:notanumber:1: bad line number
Main.kt:5:notacol: bad column
A.kt:7:2:
B.kt:9:3: another real one (no-wildcard-imports)
Summary error count (descending) by rule:
";
        let findings = parse_ktlint(stdout, Path::new("/proj"));
        // Only the two well-formed lines with numeric line+col and a non-empty message
        // survive. The prose line, the bad line number, the bad column, the empty-message
        // line, and the summary line are dropped.
        assert_eq!(findings.len(), 2, "findings: {findings:?}");
        assert_eq!(findings[0].file, "Main.kt");
        assert_eq!(findings[0].line, Some(3));
        assert_eq!(findings[1].file, "B.kt");
        assert_eq!(findings[1].line, Some(9));
    }

    #[test]
    fn empty_input_yields_no_findings() {
        assert!(parse_ktlint("", Path::new("/proj")).is_empty());
        assert!(parse_ktlint("\n\n", Path::new("/proj")).is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let stdout = "A.kt:1:1: leaked token AKIAIOSFODNN7EXAMPLE here (rule)\n";
        let findings = parse_ktlint(stdout, Path::new("/proj"));
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.title);
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"), "leaked: {}", f.body);
        assert!(f.body.contains("[redacted]"));
    }

    #[test]
    fn absolute_path_line_is_relativized_against_root() {
        // A ktlint version that ignores/lacks the relative-reporter behavior can echo an
        // ABSOLUTE path. We strip the `root` prefix so the finding's `file` is still
        // project-relative (and forward-slash normalized). Use a Unix-style absolute path
        // with a matching root so the test is deterministic across platforms.
        let stdout = "/home/me/proj/app/src/Main.kt:3:1: Unexpected indentation (indent)\n";
        let findings = parse_ktlint(stdout, Path::new("/home/me/proj"));
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].file, "app/src/Main.kt",
            "absolute path not relativized against root: {}",
            findings[0].file
        );
    }

    #[test]
    fn already_relative_path_is_left_unchanged() {
        // The common case: invoked with the relative path under cwd=root, ktlint echoes a
        // relative path. `strip_prefix(root)` does not match a relative path, so it is
        // returned unchanged (only forward-slash normalized).
        let stdout = "app/src/Main.kt:3:1: Unexpected indentation (indent)\n";
        let findings = parse_ktlint(stdout, Path::new("/home/me/proj"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "app/src/Main.kt");
    }

    // ---- presence-gated integration: skip when ktlint absent; ONE tiny run when present.

    #[test]
    fn run_absent_tool_is_empty_present_tool_flags_unstyled_file() {
        use std::sync::atomic::{AtomicU64, Ordering};

        // Absent ktlint → empty Vec, no error (graceful absence). When ktlint IS present,
        // a deliberately mis-styled file is flagged (ONE tiny run). We assert only that
        // the runner does not error and that any findings are well-formed Style/Low
        // (the exact rule set varies by ktlint version, so we don't assert a count).
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aspis-ktlint-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Bad indentation + a wildcard import + trailing whitespace: ktlint-unclean.
        let rel = "Bad.kt";
        std::fs::write(
            dir.join(rel),
            "import a.*\nfun  main() {\n      val x=1 \n}\n",
        )
        .unwrap();

        let target = RunTarget {
            file_rel_path: rel.to_string(),
        };
        let findings = run(&dir, &target);
        if crate::backend::projects::command_exists("ktlint") {
            for f in &findings {
                assert_eq!(f.source, "ktlint");
                assert_eq!(f.severity, Severity::Low);
                assert_eq!(f.category, Category::Style);
            }
        } else {
            assert!(findings.is_empty(), "absent ktlint must yield an empty Vec");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
