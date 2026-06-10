//! knip runner (unused files/exports/dependencies in JS/TS projects).
//!
//! `knip --reporter json` emits `{ "issues": [ { "file", <type>: [{name,line,...}] } ] }`.
//! Each named entry in a recognized issue-type array (exports/types/dependencies/
//! devDependencies/unlisted/enumMembers/duplicates/unresolved/...) becomes one
//! dead-code finding. Severity/category via `knip_category` (Low DeadCode).
//! Granularity is Fine (project-level run, but bucketed fine for the JS debounce).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::knip_category;
use super::{cap, redact_secrets, run_capture, Granularity, RawFinding};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

#[derive(Deserialize)]
struct KnipReport {
    #[serde(default)]
    issues: Vec<KnipIssue>,
}

#[derive(Deserialize)]
struct KnipIssue {
    #[serde(default)]
    file: String,
    /// All type-specific arrays (exports, types, dependencies, …) are captured
    /// generically so we don't have to enumerate knip's full, evolving taxonomy.
    /// `owners` is a string array and is excluded by the entry shape (it has no
    /// object entries with a `name`).
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct KnipEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    line: Option<u32>,
}

/// Type keys we surface as findings, with a human label. Limiting to a known set
/// avoids turning unrelated metadata arrays into findings.
const ISSUE_TYPES: &[(&str, &str)] = &[
    ("files", "Unused file"),
    ("dependencies", "Unused dependency"),
    ("devDependencies", "Unused devDependency"),
    (
        "optionalPeerDependencies",
        "Unused optional peer dependency",
    ),
    ("unlisted", "Unlisted dependency"),
    ("binaries", "Unlisted binary"),
    ("unresolved", "Unresolved import"),
    ("exports", "Unused export"),
    ("nsExports", "Unused namespace export"),
    ("types", "Unused exported type"),
    ("nsTypes", "Unused namespace type"),
    ("enumMembers", "Unused enum member"),
    ("classMembers", "Unused class member"),
    ("duplicates", "Duplicate export"),
];

/// Parse `knip --reporter json` stdout. PURE. Malformed JSON → empty. Each named
/// entry in a recognized issue-type array becomes a finding anchored to the
/// issue's `file`. Entries without a usable name are skipped. Tolerant.
pub fn parse_knip(stdout: &str) -> Vec<RawFinding> {
    let report: KnipReport = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let (severity, category) = knip_category();
    let mut out = Vec::new();
    for issue in report.issues {
        let file = issue.file.replace('\\', "/");
        if file.is_empty() {
            continue;
        }
        for (key, label) in ISSUE_TYPES {
            let value = match issue.extra.get(*key) {
                Some(v) => v,
                None => continue,
            };
            let arr = match value.as_array() {
                Some(a) => a,
                None => continue,
            };
            for raw in arr {
                let entry: KnipEntry = match serde_json::from_value(raw.clone()) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if entry.name.is_empty() {
                    // For "files" the file itself is the subject; name may be absent.
                    if *key == "files" {
                        out.push(RawFinding {
                            file: file.clone(),
                            line: None,
                            severity,
                            category,
                            source: "knip".to_string(),
                            title: (*label).to_string(),
                            body: format!("{label}: {file}"),
                        });
                    }
                    continue;
                }
                // The symbol/dependency name is a structured source identifier, but
                // per the conservative fail-closed stance (MINOR 1, defense-in-depth)
                // it still goes through `redact_secrets` before `cap()`: a secret-
                // shaped name (e.g. an export named `AKIAIOSFODNN7EXAMPLE`) must never
                // reach a shard title/body. Ordinary camelCase identifiers are left
                // intact by the redactor's heuristics.
                let name = cap(&redact_secrets(&entry.name), 120);
                out.push(RawFinding {
                    file: file.clone(),
                    line: entry.line,
                    severity,
                    category,
                    source: "knip".to_string(),
                    title: format!("{label}: {name}"),
                    body: format!("{label} '{name}' in {file}"),
                });
            }
        }
    }
    out
}

/// Run knip from the project root using the project's knip config. Absent `knip`
/// → empty. knip is project-wide; A3 may dedupe/limit by the changed file.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("knip") {
        return Vec::new();
    }
    let stdout = run_capture("knip", &["--reporter", "json", "--no-progress"], root);
    match stdout {
        Some(s) => parse_knip(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_unused_export_and_type() {
        // NOTE: identifiers are kept SHORT here on purpose. MINOR 1 runs each name
        // through `redact_secrets` before `cap()`, and that shared (Python-mirrored)
        // redactor flags any 12+ char token that mixes upper+lower as secret-shaped —
        // so a long camelCase identifier (e.g. `unusedExport`, exactly 12 chars) would
        // be reported as `[redacted]`. That false-positive trade-off is accepted for
        // the conservative fail-closed stance; these names stay under the threshold so
        // the test asserts the normal (non-redacted) shape.
        let json = r#"{
          "issues": [
            {
              "file": "src/Registration.tsx",
              "owners": ["@org/owner"],
              "exports": [{"name":"oldFn","line":1,"col":14,"pos":13}],
              "types": [{"name":"OldType","line":3,"col":13,"pos":71}]
            }
          ]
        }"#;
        let findings = parse_knip(json);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.file == "src/Registration.tsx"));
        assert!(findings.iter().all(|f| f.source == "knip"));
        assert!(findings.iter().all(|f| f.severity == Severity::Low));
        assert!(findings.iter().all(|f| f.category == Category::DeadCode));
        let export = findings.iter().find(|f| f.title.contains("oldFn")).unwrap();
        assert_eq!(export.line, Some(1));
        assert!(export.title.starts_with("Unused export: "));
    }

    #[test]
    fn parses_unused_dependency() {
        let json = r#"{"issues":[{"file":"package.json","dependencies":[{"name":"jquery","line":5,"col":6}]}]}"#;
        let findings = parse_knip(json);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "package.json");
        assert!(findings[0].title.starts_with("Unused dependency: jquery"));
        assert_eq!(findings[0].line, Some(5));
    }

    #[test]
    fn owners_array_is_not_treated_as_finding() {
        let json = r#"{"issues":[{"file":"a.ts","owners":["@x"],"exports":[]}]}"#;
        assert!(parse_knip(json).is_empty());
    }

    #[test]
    fn malformed_and_empty_yield_empty() {
        assert!(parse_knip("not json").is_empty());
        assert!(parse_knip("").is_empty());
        assert!(parse_knip(r#"{"issues":[]}"#).is_empty());
    }

    #[test]
    fn secret_shaped_symbol_name_is_redacted() {
        // MINOR 1 (defense-in-depth): an export/dependency literally named like an
        // AWS access key must be redacted in the title/body before it reaches a shard.
        let json =
            r#"{"issues":[{"file":"a.ts","exports":[{"name":"AKIAIOSFODNN7EXAMPLE","line":1}]}]}"#;
        let findings = parse_knip(json);
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
    }
}
