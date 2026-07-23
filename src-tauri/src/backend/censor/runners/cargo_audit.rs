//! cargo audit runner (RustSec advisory DB).
//!
//! `cargo audit --json` emits a SINGLE JSON object (not line-delimited) with a
//! `vulnerabilities.list` array. Each entry has an `advisory` (id, title) and a
//! `package` (name, version). Vulnerabilities are crate-level, not file-anchored,
//! so the finding's `file` is set to `Cargo.toml` and `line` is `None`.
//!
//! Severity/category: a known vulnerability in a dependency is High Security.
//! Granularity is Coarse (project-level dependency scan).

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::schema::{Category, Severity};
use super::{redact_secrets, run_capture, run_capture_with_timeout, Granularity, RawFinding, RunnerOutcome};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Coarse
}

#[derive(Deserialize)]
struct AuditReport {
    #[serde(default)]
    vulnerabilities: Vulnerabilities,
}

#[derive(Deserialize, Default)]
struct Vulnerabilities {
    #[serde(default)]
    list: Vec<Vulnerability>,
}

#[derive(Deserialize)]
struct Vulnerability {
    #[serde(default)]
    advisory: Advisory,
    #[serde(default)]
    package: PackageRef,
}

#[derive(Deserialize, Default)]
struct Advisory {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize, Default)]
struct PackageRef {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
}

/// Parse `cargo audit --json` stdout into raw findings. PURE. A non-JSON / empty
/// stdout yields an empty Vec (cargo-audit prints nothing parseable when the DB
/// can't be fetched). Each vulnerability becomes one file-level finding on
/// `Cargo.toml`. The advisory id/title and package name/version are structured
/// metadata (no secret risk).
pub fn parse_cargo_audit(stdout: &str) -> Vec<RawFinding> {
    let report: AuditReport = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    report
        .vulnerabilities
        .list
        .into_iter()
        .filter_map(|v| {
            if v.advisory.id.is_empty() && v.package.name.is_empty() {
                return None;
            }
            // The advisory id (RUSTSEC-YYYY-NNNN) and package name/version are
            // STRUCTURED metadata from the RustSec DB / Cargo.toml — not free text,
            // so they are left intact (the id is shaped like a token but is a known
            // identifier, never a secret). PRIVACY: the advisory TITLE is free prose,
            // so redact secret-shaped tokens there before it reaches title/body.
            let pkg = if v.package.version.is_empty() {
                v.package.name.clone()
            } else {
                format!("{} {}", v.package.name, v.package.version)
            };
            let safe_title = redact_secrets(&v.advisory.title);
            let title = format!(
                "{}: {}",
                v.advisory.id,
                if safe_title.is_empty() {
                    "known vulnerability".to_string()
                } else {
                    safe_title.clone()
                }
            );
            Some(RawFinding {
                file: "Cargo.toml".to_string(),
                line: None,
                severity: Severity::High,
                category: Category::Security,
                source: "cargo-audit".to_string(),
                title,
                body: format!(
                    "{} affects dependency {} (advisory {})",
                    if safe_title.is_empty() {
                        "Known vulnerability"
                    } else {
                        &safe_title
                    },
                    pkg,
                    v.advisory.id
                ),
            })
        })
        .collect()
}

/// Run cargo audit from the project root. Absent `cargo` → empty.
///
/// A missing `cargo-audit` SUBCOMMAND is distinguished from "no vulnerabilities":
/// `cargo audit --version` is probed first; if that fails (no such subcommand),
/// we log a SPECIFIC "cargo-audit not installed" message and return empty, so a
/// missing security scanner is never mistaken for a clean all-clear. The probe is
/// cheap and offline (it does not touch the advisory DB).
///
/// The advisory-DB fetch + scan can be slow on a cold cache, so the actual audit
/// runs under a longer timeout than the per-file default.
pub fn run(root: &Path) -> RunnerOutcome {
    if !crate::backend::projects::command_exists("cargo") {
        return RunnerOutcome::Skipped;
    }
    // Detect a missing `cargo-audit` subcommand. `--version` is fast and offline;
    // its absence (None) means the subcommand isn't installed.
    if run_capture("cargo", &["audit", "--version"], root).is_none() {
        eprintln!(
            "censor: cargo-audit not installed at {} (dependency vulnerability scan skipped)",
            root.display()
        );
        return RunnerOutcome::Skipped;
    }
    // Cold advisory-DB fetch + scan can take a while; allow a generous budget.
    let stdout = run_capture_with_timeout(
        "cargo",
        &["audit", "--json"],
        root,
        Duration::from_secs(300),
    );
    match stdout {
        Some(s) => RunnerOutcome::Ok(parse_cargo_audit(&s)),
        None => RunnerOutcome::Failed,
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
                "found": true,
                "count": 1,
                "list": [
                    {
                        "advisory": {"id":"RUSTSEC-2021-0001","title":"smol vuln"},
                        "package": {"name":"badcrate","version":"1.2.3"}
                    }
                ]
            }
        }"#;
        let findings = parse_cargo_audit(json);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "Cargo.toml");
        assert_eq!(f.line, None);
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "cargo-audit");
        assert!(f.title.contains("RUSTSEC-2021-0001"));
        assert!(f.body.contains("badcrate 1.2.3"));
    }

    #[test]
    fn empty_list_yields_empty() {
        let json = r#"{"vulnerabilities":{"found":false,"count":0,"list":[]}}"#;
        assert!(parse_cargo_audit(json).is_empty());
    }

    #[test]
    fn malformed_yields_empty() {
        assert!(parse_cargo_audit("not json").is_empty());
        assert!(parse_cargo_audit("").is_empty());
    }

    #[test]
    fn redacts_secret_in_advisory_title() {
        let json = r#"{"vulnerabilities":{"list":[{"advisory":{"id":"RUSTSEC-2021-0001","title":"leak AKIAIOSFODNN7EXAMPLE in logs"},"package":{"name":"badcrate","version":"1.2.3"}}]}}"#;
        let findings = parse_cargo_audit(json);
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
