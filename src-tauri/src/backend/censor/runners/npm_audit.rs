//! npm audit runner (Node.js dependency vulnerability scan).
//!
//! `npm audit --json` emits a JSON object with a `vulnerabilities` map.
//! Each key is a package name, value contains severity, range, via, etc.
//! Vulnerabilities are package-level, not file-anchored, so the finding's
//! `file` is set to `package.json` and `line` is `None`.
//!
//! Severity/category: a known vulnerability in a dependency is High Security.
//! Granularity is Coarse (project-level dependency scan).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Coarse
}

#[derive(Deserialize, Default)]
struct AuditReport {
    #[serde(default)]
    vulnerabilities: BTreeMap<String, VulnerabilityEntry>,
}

#[derive(Deserialize, Default)]
struct VulnerabilityEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    range: String,
    #[serde(default)]
    via: Vec<ViaEntry>,
    #[serde(default)]
    fix_available: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ViaEntry {
    Object { title: Option<String>, url: Option<String> },
    String(String),
}

/// Parse `npm audit --json` stdout into raw findings. PURE. A non-JSON / empty
/// stdout yields an empty Vec. Each vulnerability in the map becomes one finding
/// on `package.json`. The vulnerabilities map is iterated in sorted key order
/// (BTreeMap) for deterministic output.
///
/// Tolerant parsing: missing fields default to empty/None. An {"error":...} payload
/// or non-JSON yields empty findings.
pub fn parse_npm_audit(stdout: &str) -> Vec<RawFinding> {
    let report: AuditReport = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    report
        .vulnerabilities
        .into_iter()
        .filter_map(|(pkg_name, entry)| {
            if pkg_name.is_empty() {
                return None;
            }
            let name = if entry.name.is_empty() {
                pkg_name.clone()
            } else {
                entry.name.clone()
            };
            let range = if entry.range.is_empty() {
                "unknown".to_string()
            } else {
                entry.range.clone()
            };
            let (severity, category) = super::super::severity::severity_from_npm_audit(&entry.severity);

            // Determine title
            let title = format!("npm audit: {} {}", name, range);

            // Determine body: first via entry's title if it's an object with a title,
            // else generic message.
            let body = if let Some(via) = entry.via.first() {
                match via {
                    ViaEntry::Object { title: Some(t), .. } => {
                        let safe_t = redact_secrets(t);
                        if safe_t.is_empty() {
                            format!("Known vulnerability in {}", name)
                        } else {
                            safe_t
                        }
                    }
                    _ => format!("Known vulnerability in {}", name),
                }
            } else {
                format!("Known vulnerability in {}", name)
            };

            // Char-safe cap; the only tool-derived text (the via title) is
            // already redacted above, the fallbacks are our own fixed text.
            let body = cap(&body, 1000);

            Some(RawFinding {
                file: "package.json".to_string(),
                line: None,
                severity,
                category,
                source: "npm-audit".to_string(),
                title,
                body,
            })
        })
        .collect()
}

/// Run npm audit from the project root. Absent `npm` → empty.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("npm") {
        return Vec::new();
    }
    // npm audit can be slow; allow a generous budget.
    let stdout = run_capture_with_timeout(
        "npm",
        &["audit", "--json"],
        root,
        Duration::from_secs(300),
    );
    match stdout {
        Some(s) => parse_npm_audit(&s),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    #[test]
    fn parses_a_vulnerability() {
        let json = r#"{
            "vulnerabilities": {
                "bad-pkg": {
                    "name": "bad-pkg",
                    "severity": "high",
                    "range": "<1.2.3",
                    "via": [{"title": "Prototype Pollution in bad-pkg", "url": "https://example.com"}],
                    "fixAvailable": true
                }
            }
        }"#;
        let findings = parse_npm_audit(json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "package.json");
        assert_eq!(f.line, None);
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "npm-audit");
        assert!(f.title.contains("bad-pkg"));
        assert!(f.body.contains("Prototype Pollution"));
    }

    #[test]
    fn parses_moderate_severity() {
        let json = r#"{
            "vulnerabilities": {
                "mod-pkg": {
                    "name": "mod-pkg",
                    "severity": "moderate",
                    "range": ">=1.0.0 <2.0.0",
                    "via": ["Some issue"],
                    "fixAvailable": false
                }
            }
        }"#;
        let findings = parse_npm_audit(json);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Medium);
    }

    #[test]
    fn parses_low_severity() {
        let json = r#"{
            "vulnerabilities": {
                "low-pkg": {
                    "name": "low-pkg",
                    "severity": "low",
                    "range": "*",
                    "via": ["Low issue"],
                    "fixAvailable": false
                }
            }
        }"#;
        let findings = parse_npm_audit(json);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Low);
    }

    #[test]
    fn empty_vulnerabilities_yields_empty() {
        let json = r#"{"vulnerabilities": {}}"#;
        assert!(parse_npm_audit(json).is_empty());
    }

    #[test]
    fn malformed_yields_empty() {
        assert!(parse_npm_audit("not json").is_empty());
        assert!(parse_npm_audit("").is_empty());
    }

    #[test]
    fn error_payload_yields_empty() {
        let json = r#"{"error": "npm audit failed"}"#;
        assert!(parse_npm_audit(json).is_empty());
    }

    #[test]
    fn redacts_secret_in_via_title() {
        let json = r#"{
            "vulnerabilities": {
                "leak-pkg": {
                    "name": "leak-pkg",
                    "severity": "high",
                    "range": "<1.0.0",
                    "via": [{"title": "Leak AKIAIOSFODNN7EXAMPLE in logs", "url": "https://example.com"}],
                    "fixAvailable": true
                }
            }
        }"#;
        let findings = parse_npm_audit(json);
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

    #[test]
    fn string_via_entry_yields_generic_body() {
        let json = r#"{
            "vulnerabilities": {
                "str-pkg": {
                    "name": "str-pkg",
                    "severity": "high",
                    "range": "*",
                    "via": ["Some string description"],
                    "fixAvailable": false
                }
            }
        }"#;
        let findings = parse_npm_audit(json);
        assert_eq!(findings.len(), 1);
        // A bare-string via entry carries no structured title → generic body.
        assert!(findings[0].body.contains("Known vulnerability in str-pkg"));
    }

    #[test]
    fn missing_via_yields_generic_body() {
        let json = r#"{
            "vulnerabilities": {
                "no-via-pkg": {
                    "name": "no-via-pkg",
                    "severity": "high",
                    "range": "*",
                    "via": [],
                    "fixAvailable": false
                }
            }
        }"#;
        let findings = parse_npm_audit(json);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].body.contains("Known vulnerability in no-via-pkg"));
    }

    #[test]
    fn sorted_keys_deterministic() {
        let json = r#"{
            "vulnerabilities": {
                "z-pkg": {
                    "name": "z-pkg",
                    "severity": "high",
                    "range": "*",
                    "via": [{"title": "Z issue", "url": ""}],
                    "fixAvailable": false
                },
                "a-pkg": {
                    "name": "a-pkg",
                    "severity": "high",
                    "range": "*",
                    "via": [{"title": "A issue", "url": ""}],
                    "fixAvailable": false
                }
            }
        }"#;
        let findings = parse_npm_audit(json);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].title, "npm audit: a-pkg *");
        assert_eq!(findings[1].title, "npm audit: z-pkg *");
    }
}