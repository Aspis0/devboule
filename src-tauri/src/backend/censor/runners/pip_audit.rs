//! pip-audit runner (Python dependency vulnerability scan).
//!
//! `pip-audit --format json --progress-spinner off` emits a JSON object with a
//! `dependencies` array. Each entry has a `name`, `version`, and `vulns` array.
//! Vulnerabilities are dependency-level, not file-anchored, so the finding's
//! `file` is set to `requirements.txt` and `line` is `None`.
//!
//! Severity/category: a known vulnerability in a dependency is High Security.
//! Granularity is Coarse (project-level dependency scan).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::schema::{Category, Severity};
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Coarse
}

#[derive(Deserialize, Default)]
struct AuditReport {
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize, Default)]
struct Dependency {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    vulns: Vec<Vulnerability>,
}

#[derive(Deserialize, Default)]
struct Vulnerability {
    #[serde(default)]
    id: String,
    #[serde(default)]
    fix_versions: Vec<String>,
    #[serde(default)]
    description: String,
}

/// Parse `pip-audit --format json --progress-spinner off` stdout into raw findings.
/// PURE. A non-JSON / empty stdout yields an empty Vec. Each (dependency, vuln)
/// pair becomes one finding on `requirements.txt`.
///
/// Tolerant parsing: missing fields default to empty/None. An {"error":...} payload
/// or non-JSON yields empty findings.
pub fn parse_pip_audit(stdout: &str) -> Vec<RawFinding> {
    let report: AuditReport = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    report
        .dependencies
        .into_iter()
        .flat_map(|dep| {
            dep.vulns.into_iter().filter_map(move |vuln| {
                if dep.name.is_empty() {
                    return None;
                }
                let pkg = if dep.version.is_empty() {
                    dep.name.clone()
                } else {
                    format!("{} {}", dep.name, dep.version)
                };
                let id = if vuln.id.is_empty() {
                    "UNKNOWN".to_string()
                } else {
                    vuln.id.clone()
                };
                let title = format!("{}: {} {}", id, dep.name, dep.version);

                // Determine body
                let body = if vuln.description.is_empty() {
                    let fix_versions_str = vuln.fix_versions.join(", ");
                    format!(
                        "Known vulnerability in {}; fix versions: {}",
                        pkg,
                        if fix_versions_str.is_empty() {
                            "unknown".to_string()
                        } else {
                            fix_versions_str
                        }
                    )
                } else {
                    vuln.description.clone()
                };

                // Redact secrets (advisory text can echo values), then char-safe cap.
                let body = cap(&redact_secrets(&body), 1000);

                Some(RawFinding {
                    file: "requirements.txt".to_string(),
                    line: None,
                    severity: Severity::High,
                    category: Category::Security,
                    source: "pip-audit".to_string(),
                    title,
                    body,
                })
            })
        })
        .collect()
}

/// Run pip-audit from the project root. Absent `pip-audit` → empty.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("pip-audit") {
        return Vec::new();
    }
    // pip-audit can be slow; allow a generous budget.
    let stdout = run_capture_with_timeout(
        "pip-audit",
        &["--format", "json", "--progress-spinner", "off"],
        root,
        Duration::from_secs(300),
    );
    match stdout {
        Some(s) => parse_pip_audit(&s),
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
            "dependencies": [
                {
                    "name": "badpkg",
                    "version": "1.0.0",
                    "vulns": [
                        {
                            "id": "PYSEC-2024-1",
                            "fix_versions": ["1.2.0"],
                            "description": "Prototype Pollution in badpkg"
                        }
                    ]
                }
            ]
        }"#;
        let findings = parse_pip_audit(json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "requirements.txt");
        assert_eq!(f.line, None);
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "pip-audit");
        assert!(f.title.contains("PYSEC-2024-1"));
        assert!(f.title.contains("badpkg 1.0.0"));
        assert!(f.body.contains("Prototype Pollution"));
    }

    #[test]
    fn parses_multiple_vulns() {
        let json = r#"{
            "dependencies": [
                {
                    "name": "badpkg",
                    "version": "1.0.0",
                    "vulns": [
                        {
                            "id": "PYSEC-2024-1",
                            "fix_versions": ["1.2.0"],
                            "description": "Vuln 1"
                        },
                        {
                            "id": "PYSEC-2024-2",
                            "fix_versions": ["1.3.0"],
                            "description": "Vuln 2"
                        }
                    ]
                }
            ]
        }"#;
        let findings = parse_pip_audit(json);
        assert_eq!(findings.len(), 2);
        assert!(findings[0].title.contains("PYSEC-2024-1"));
        assert!(findings[1].title.contains("PYSEC-2024-2"));
    }

    #[test]
    fn empty_dependencies_yields_empty() {
        let json = r#"{"dependencies": []}"#;
        assert!(parse_pip_audit(json).is_empty());
    }

    #[test]
    fn malformed_yields_empty() {
        assert!(parse_pip_audit("not json").is_empty());
        assert!(parse_pip_audit("").is_empty());
    }

    #[test]
    fn error_payload_yields_empty() {
        let json = r#"{"error": "pip-audit failed"}"#;
        assert!(parse_pip_audit(json).is_empty());
    }

    #[test]
    fn redacts_secret_in_description() {
        let json = r#"{
            "dependencies": [
                {
                    "name": "leakpkg",
                    "version": "1.0.0",
                    "vulns": [
                        {
                            "id": "PYSEC-2024-1",
                            "fix_versions": ["1.2.0"],
                            "description": "Leak AKIAIOSFODNN7EXAMPLE in logs"
                        }
                    ]
                }
            ]
        }"#;
        let findings = parse_pip_audit(json);
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
    fn empty_description_yields_generic_body() {
        let json = r#"{
            "dependencies": [
                {
                    "name": "nopkg",
                    "version": "1.0.0",
                    "vulns": [
                        {
                            "id": "PYSEC-2024-1",
                            "fix_versions": ["1.2.0"],
                            "description": ""
                        }
                    ]
                }
            ]
        }"#;
        let findings = parse_pip_audit(json);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].body.contains("Known vulnerability in nopkg 1.0.0"));
        assert!(findings[0].body.contains("fix versions: 1.2.0"));
    }

    #[test]
    fn empty_fix_versions_yields_unknown() {
        let json = r#"{
            "dependencies": [
                {
                    "name": "nopkg",
                    "version": "1.0.0",
                    "vulns": [
                        {
                            "id": "PYSEC-2024-1",
                            "fix_versions": [],
                            "description": ""
                        }
                    ]
                }
            ]
        }"#;
        let findings = parse_pip_audit(json);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].body.contains("fix versions: unknown"));
    }
}