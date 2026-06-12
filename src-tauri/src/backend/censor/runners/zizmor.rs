//! zizmor runner (GitHub Actions workflow security audit).
//!
//! `zizmor --format json .` emits a top-level ARRAY of findings. Each finding
//! has an `ident`, `desc`, `url`, `determinations` (confidence, severity), and
//! `locations` (symbolic file path, concrete line/column). zizmor exits non-zero
//! when findings exist — stdout is still the JSON.
//!
//! Severity/category: CI hardening findings are Security. Granularity is Coarse
//! (project-level workflow scan).

#![allow(dead_code)]

use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Coarse
}

#[derive(Deserialize, Default)]
struct ZizmorFinding {
    #[serde(default)]
    ident: String,
    #[serde(default)]
    desc: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    determinations: Determinations,
    #[serde(default)]
    locations: Vec<Location>,
}

#[derive(Deserialize, Default)]
struct Determinations {
    #[serde(default)]
    confidence: String,
    #[serde(default)]
    severity: String,
}

#[derive(Deserialize, Default)]
struct Location {
    #[serde(default)]
    symbolic: SymbolicLocation,
    #[serde(default)]
    concrete: ConcreteLocation,
}

#[derive(Deserialize, Default)]
struct SymbolicLocation {
    #[serde(default)]
    annotation: String,
    #[serde(default)]
    key: LocationKey,
}

#[derive(Deserialize, Default)]
struct LocationKey {
    // zizmor serializes the Rust enum variant name verbatim: `"key": {"Local":
    // {"given_path": "..."}}` — capitalized, nested object.
    #[serde(default, rename = "Local")]
    local: Option<LocalKey>,
}

#[derive(Deserialize, Default)]
struct LocalKey {
    #[serde(default)]
    given_path: String,
}

#[derive(Deserialize, Default)]
struct ConcreteLocation {
    #[serde(default)]
    location: ConcretePoint,
}

#[derive(Deserialize, Default)]
struct ConcretePoint {
    #[serde(default)]
    start_point: Option<Point>,
}

#[derive(Deserialize, Default)]
struct Point {
    #[serde(default)]
    row: usize,
    #[serde(default)]
    column: usize,
}

/// Parse `zizmor --format json .` stdout into raw findings. PURE. A non-JSON /
/// empty stdout yields an empty Vec. Each finding becomes one file-anchored
/// finding. The ident/desc are structured metadata (no secret risk), but the
/// description is redacted for secrets before reaching title/body.
pub fn parse_zizmor(stdout: &str) -> Vec<RawFinding> {
    let findings: Vec<ZizmorFinding> = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    findings
        .into_iter()
        .filter_map(|f| {
            // Determine file: first location's symbolic.key.Local.given_path,
            // fallback to the workflows dir.
            let file = f
                .locations
                .first()
                .and_then(|l| l.symbolic.key.local.as_ref())
                .map(|k| k.given_path.replace('\\', "/"))
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| ".github/workflows".to_string());

            // Determine line: first location's concrete start_point.row + 1 (0-based).
            let line = f
                .locations
                .first()
                .and_then(|l| l.concrete.location.start_point.as_ref())
                .map(|p| p.row.saturating_add(1) as u32);

            // Determine severity: map zizmor severity string.
            let (severity, category) = super::super::severity::severity_from_zizmor(&f.determinations.severity);

            // Title: "{ident}: {desc}" cap 200. Empty ident → just desc.
            let safe_desc = redact_secrets(&f.desc);
            let title = if f.ident.is_empty() {
                safe_desc.clone()
            } else {
                format!("{}: {}", f.ident, safe_desc)
            };
            let title = cap(&title, 200);

            // Body: redact_secrets(desc) cap 1000.
            let body = cap(&safe_desc, 1000);

            Some(RawFinding {
                file,
                line,
                severity,
                category,
                source: "zizmor".to_string(),
                title,
                body,
            })
        })
        .collect()
}

/// Run zizmor from the project root. Absent `zizmor` → empty.
///
/// zizmor exits non-zero when findings exist — that is fine, stdout is still
/// the JSON. The scan can be slow on large repos, so the timeout is generous.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("zizmor") {
        return Vec::new();
    }
    let stdout = run_capture_with_timeout(
        "zizmor",
        &["--format", "json", "."],
        root,
        Duration::from_secs(120),
    );
    match stdout {
        Some(s) => parse_zizmor(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_finding() {
        let json = r#"[
            {
                "ident": "artipacked",
                "desc": "credential persistence through GitHub Actions artifacts",
                "url": "https://example.com/artipacked",
                "determinations": {"confidence": "high", "severity": "medium"},
                "locations": [
                    {
                        "symbolic": {
                            "annotation": "workflow file",
                            "key": {"Local": {"given_path": ".github/workflows/ci.yml"}}
                        },
                        "concrete": {
                            "location": {"start_point": {"row": 4, "column": 0}}
                        }
                    }
                ]
            }
        ]"#;
        let findings = parse_zizmor(json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, ".github/workflows/ci.yml");
        assert_eq!(f.line, Some(5)); // 0-based row 4 → 1-based line 5
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "zizmor");
        assert!(f.title.contains("artipacked"));
        assert!(f.title.contains("credential persistence"));
        assert!(f.body.contains("credential persistence"));
    }

    #[test]
    fn missing_location_fallback() {
        let json = r#"[
            {
                "ident": "test",
                "desc": "test desc",
                "determinations": {"severity": "high"},
                "locations": []
            }
        ]"#;
        let findings = parse_zizmor(json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, ".github/workflows");
        assert_eq!(f.line, None);
        assert_eq!(f.severity, Severity::High);
    }

    #[test]
    fn severity_variants() {
        let json = r#"[
            {"ident":"a","desc":"d","determinations":{"severity":"high"},"locations":[]},
            {"ident":"b","desc":"d","determinations":{"severity":"medium"},"locations":[]},
            {"ident":"c","desc":"d","determinations":{"severity":"low"},"locations":[]},
            {"ident":"d","desc":"d","determinations":{"severity":"informational"},"locations":[]},
            {"ident":"e","desc":"d","determinations":{"severity":"unknown"},"locations":[]}
        ]"#;
        let findings = parse_zizmor(json);
        assert_eq!(findings.len(), 5);
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[1].severity, Severity::Medium);
        assert_eq!(findings[2].severity, Severity::Low);
        assert_eq!(findings[3].severity, Severity::Low);
        assert_eq!(findings[4].severity, Severity::Medium);
    }

    #[test]
    fn malformed_yields_empty() {
        assert!(parse_zizmor("not json").is_empty());
        assert!(parse_zizmor("").is_empty());
    }

    #[test]
    fn redacts_secret_in_desc() {
        let json = r#"[
            {
                "ident": "leak",
                "desc": "leak AKIAIOSFODNN7EXAMPLE in workflow",
                "determinations": {"severity": "high"},
                "locations": [
                    {
                        "symbolic": {"key": {"Local": {"given_path": ".github/workflows/ci.yml"}}},
                        "concrete": {"location": {"start_point": {"row": 1, "column": 0}}}
                    }
                ]
            }
        ]"#;
        let findings = parse_zizmor(json);
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