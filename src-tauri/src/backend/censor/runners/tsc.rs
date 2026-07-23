//! tsc (TypeScript compiler) runner.
//!
//! tsc has no stable JSON diagnostic output, so we run `tsc --noEmit --pretty
//! false` and parse the canonical one-line diagnostic format:
//!   `path/to/file.ts(LINE,COL): error TSxxxx: message`
//! The category token (`error`/`warning`/`suggestion`/`message`) maps via
//! `severity_from_tsc`. Granularity is Fine (per-file), though tsc itself compiles
//! the whole project — A3 still buckets it as fine for the JS/TS debounce.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_tsc;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding, RunnerOutcome};
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// Parse `tsc --pretty false` stdout. PURE. Each diagnostic line is
/// `file(line,col): category TScode: message`. Lines that don't match the shape
/// (continuation lines, blank lines, summary) are skipped. Tolerant.
pub fn parse_tsc(stdout: &str) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        if let Some(f) = parse_tsc_line(line) {
            out.push(f);
        }
    }
    out
}

/// Parse one tsc diagnostic line. Returns `None` for any line that isn't a
/// well-formed `file(line,col): category code: message`.
fn parse_tsc_line(line: &str) -> Option<RawFinding> {
    // Split off the "(line,col)" location. A path can itself contain parens
    // (e.g. `C:/Program Files (x86)/app/a.ts(12,5): ...`), so we must NOT take the
    // FIRST '(' — we take the LAST `(<digits>,<digits>)` group, which is the tsc
    // location marker. Scan '(' candidates right-to-left and accept the first that
    // holds a valid `line,col`.
    let (paren_open, paren_close, line_no) = find_location(line)?;

    let file = line[..paren_open].trim();
    if file.is_empty() {
        return None;
    }

    // After ")": expect ": <category> TScode: <message>".
    let after = line[paren_close + 1..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    // Category is the first word.
    let mut words = after.splitn(2, ' ');
    let category_word = words.next()?.trim();
    let remainder = words.next().unwrap_or("").trim();
    // remainder is "TSxxxx: message"; split on the first ": ".
    let (code, message) = match remainder.split_once(": ") {
        Some((c, m)) => (c.trim(), m.trim()),
        None => ("", remainder),
    };
    if message.is_empty() && code.is_empty() {
        return None;
    }

    let (severity, cat) = severity_from_tsc(category_word);
    // PRIVACY: a tsc diagnostic can echo a string literal (e.g. a type error on
    // an inline secret string). Redact secret-shaped tokens BEFORE title/body.
    let safe_message = redact_secrets(message);
    let title = if code.is_empty() {
        cap(&safe_message, 200)
    } else {
        format!("{code}: {}", cap(&safe_message, 200))
    };
    Some(RawFinding {
        file: file.replace('\\', "/"),
        line: Some(line_no),
        severity,
        category: cat,
        source: "tsc".to_string(),
        title,
        body: cap(&safe_message, 1000),
    })
}

/// Find the tsc location marker `(<line>,<col>)` in a diagnostic line, returning
/// `(open_paren_byte_index, close_paren_byte_index, line_number)`. Because a path
/// may contain parens, we scan `(` candidates from RIGHTMOST to leftmost and take
/// the first whose parenthesized content is exactly `<digits>,<digits>` — that is
/// the location group tsc appends after the (possibly paren-containing) path.
fn find_location(line: &str) -> Option<(usize, usize, u32)> {
    // Collect byte indices of every '(' then scan them in reverse.
    let opens: Vec<usize> = line.match_indices('(').map(|(i, _)| i).collect();
    for &open in opens.iter().rev() {
        let after_open = &line[open + 1..];
        let close_rel = match after_open.find(')') {
            Some(c) => c,
            None => continue,
        };
        let inner = &after_open[..close_rel];
        let mut parts = inner.split(',');
        let (Some(l), Some(c), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let line_no: u32 = match l.trim().parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        // column must be present + numeric to validate the shape (value unused).
        if c.trim().parse::<u32>().is_err() {
            continue;
        }
        let close = open + 1 + close_rel;
        return Some((open, close, line_no));
    }
    None
}

/// Run tsc from the project root using the project's tsconfig.json. Absent `tsc`
/// → empty.
pub fn run(root: &Path) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("tsc") {
        return RunnerOutcome::Skipped;
    }
    // `--incremental` caches the program graph (`.tsbuildinfo`) so re-runs on the
    // coarse debounce skip unchanged files — a large win for a project-wide tsc.
    // It is compatible with `--noEmit` on TypeScript >= 5.4 (the supported range);
    // on older tsc the combination errors and the runner degrades to empty (tsc
    // simply yields no findings, never a crash).
    let stdout = run_capture(
        "tsc",
        &["--noEmit", "--incremental", "--pretty", "false"],
        root,
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_tsc(&s)),
        None => RunnerOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_error_line() {
        let line = "src/app.ts(12,5): error TS2304: Cannot find name 'foo'.";
        let findings = parse_tsc(line);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "src/app.ts");
        assert_eq!(f.line, Some(12));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Correctness);
        assert_eq!(f.source, "tsc");
        assert!(f.title.starts_with("TS2304: "));
        assert!(f.title.contains("Cannot find name"));
    }

    #[test]
    fn parses_multiple_lines_and_skips_noise() {
        let stdout = "\
src/a.ts(1,1): error TS1005: ';' expected.
this is a continuation line
src/b.ts(40,9): error TS2322: Type 'string' is not assignable to type 'number'.
Found 2 errors.";
        let findings = parse_tsc(stdout);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "src/a.ts");
        assert_eq!(findings[1].file, "src/b.ts");
        assert_eq!(findings[1].line, Some(40));
    }

    #[test]
    fn parses_path_containing_parentheses() {
        // A path with parens (e.g. `Program Files (x86)`) must not be split on the
        // FIRST '(' — the LAST `(line,col)` group is the location marker.
        let line =
            "C:/Program Files (x86)/proj/src/app.ts(12,5): error TS2304: Cannot find name 'foo'.";
        let findings = parse_tsc(line);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "C:/Program Files (x86)/proj/src/app.ts");
        assert_eq!(f.line, Some(12));
        assert!(f.title.starts_with("TS2304: "));
    }

    #[test]
    fn normalizes_windows_path() {
        let line = "src\\nested\\c.ts(3,2): error TS2552: foo.";
        let findings = parse_tsc(line);
        assert_eq!(findings[0].file, "src/nested/c.ts");
    }

    #[test]
    fn empty_and_malformed_yield_empty() {
        assert!(parse_tsc("").is_empty());
        assert!(parse_tsc("random text\nno location here").is_empty());
    }

    #[test]
    fn redacts_secret_in_message() {
        let line =
            "src/a.ts(1,1): error TS2322: Type '\"AKIAIOSFODNN7EXAMPLE\"' is not assignable.";
        let findings = parse_tsc(line);
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
    }
}
