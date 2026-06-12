//! cargo-deny runner (advisories, bans, sources over the dependency graph).
//!
//! `cargo-deny --format json check advisories bans sources` emits LINE-DELIMITED
//! JSON diagnostics on STDERR (stdout stays human/empty) — this runner is the
//! reason `run_capture_stderr_with_timeout` exists. Each diagnostic line:
//! {"type":"diagnostic","fields":{"severity":"error|warning|note|help",
//!  "code":"vulnerability","message":"...","graphs":[{"Krate":{"name":..}}]}}.
//! `licenses` is NOT checked: it requires a curated deny.toml (owner decision).
//!
//! P2 ROLLOUT DISCIPLINE: ADVISORY-FIRST severities (error→Medium, the rest→
//! Low) — promotion to a blocking High happens only after the FP-rate on this
//! repo is measured (owner call). Findings are crate-level → `Cargo.lock`.

#![allow(dead_code)]

use super::super::severity::severity_from_cargo_deny;
use super::{cap, redact_secrets, run_capture, run_capture_stderr_with_timeout, Granularity, RawFinding};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

/// Dependency-graph scan on a cold advisory DB can be slow.
const CARGO_DENY_TIMEOUT: Duration = Duration::from_secs(120);

pub fn granularity() -> Granularity {
    Granularity::Coarse
}

#[derive(Deserialize, Default)]
struct DenyLine {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    fields: DenyFields,
}

#[derive(Deserialize, Default)]
struct DenyFields {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    graphs: Vec<DenyGraph>,
}

#[derive(Deserialize, Default)]
struct DenyGraph {
    #[serde(default, rename = "Krate")]
    krate: DenyKrate,
}

#[derive(Deserialize, Default)]
struct DenyKrate {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
}

/// Parse cargo-deny's line-delimited STDERR diagnostics into raw findings.
/// PURE. Non-diagnostic lines (summaries) and malformed lines are skipped —
/// cargo-deny mixes record types on one stream by design.
pub fn parse_cargo_deny(stderr: &str) -> Vec<RawFinding> {
    stderr
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let parsed: DenyLine = serde_json::from_str(line).ok()?;
            if parsed.kind != "diagnostic" {
                return None;
            }
            let f = parsed.fields;
            if f.message.is_empty() && f.code.is_empty() {
                return None;
            }
            let (severity, category) = severity_from_cargo_deny(&f.severity);
            // PRIVACY: the message is free prose from the tool — redact
            // secret-shaped tokens, then cap.
            let safe_message = redact_secrets(&f.message);
            let krate = f
                .graphs
                .first()
                .map(|g| {
                    if g.krate.version.is_empty() {
                        g.krate.name.clone()
                    } else {
                        format!("{} {}", g.krate.name, g.krate.version)
                    }
                })
                .filter(|k| !k.trim().is_empty());
            // The code is normally a short identifier (vulnerability/duplicate/…)
            // but it arrives from untrusted JSON — redact like the message.
            let safe_code = redact_secrets(&f.code);
            let title = if safe_code.is_empty() {
                "cargo-deny finding".to_string()
            } else {
                format!("cargo-deny: {safe_code}")
            };
            let body = match krate {
                Some(k) => cap(&format!("{safe_message} (dependency {k})"), 1000),
                None => cap(&safe_message, 1000),
            };
            Some(RawFinding {
                file: "Cargo.lock".to_string(),
                line: None,
                severity,
                category,
                source: "cargo-deny".to_string(),
                title,
                body,
            })
        })
        .collect()
}

/// Run cargo-deny from the project root. A missing binary is logged as a
/// SPECIFIC "not installed" (never a silent all-clear); the --version probe is
/// cheap and offline.
pub fn run(root: &Path) -> Vec<RawFinding> {
    if run_capture("cargo-deny", &["--version"], root).is_none() {
        // Once per session (max-recall fix): the coarse pass fires every few
        // seconds of activity — a per-pass line would flood the log.
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!(
                "censor: cargo-deny not installed — dependency policy scan skipped at {}",
                root.display()
            );
        }
        return Vec::new();
    }
    // Max-recall fixes:
    //   --offline: a censor runner must never block the serialized worker on a
    //     network advisory-DB fetch (and the runner posture is offline); the
    //     user keeps the DB fresh with `cargo deny fetch`.
    //   bans/sources only WITH a deny.toml: without one cargo-deny hard-errors
    //     (silent false-clean) or FP-floods on default source policy.
    let mut args: Vec<&str> = vec!["--offline", "--format", "json", "check", "advisories"];
    if root.join("deny.toml").exists() {
        args.push("bans");
        args.push("sources");
    }
    let stderr = match run_capture_stderr_with_timeout(
        "cargo-deny",
        &args,
        root,
        CARGO_DENY_TIMEOUT,
    ) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let findings = parse_cargo_deny(&stderr);
    if findings.is_empty()
        && !stderr.trim().is_empty()
        && !stderr.contains("\"type\":\"diagnostic\"")
    {
        // The tool spoke but produced NO parseable diagnostics: likely a
        // missing cached advisory DB (--offline) or a config error. Say so —
        // a security scanner must never silently read as all-clear.
        // Identity-only log (the text may echo project details).
        eprintln!(
            "censor: cargo-deny ran but produced no parseable diagnostics at {} — check `cargo deny fetch` / deny.toml",
            root.display()
        );
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::Severity;

    #[test]
    fn parses_diagnostics_and_skips_summaries() {
        let raw = concat!(
            "{\"type\":\"diagnostic\",\"fields\":{\"severity\":\"error\",\"code\":\"vulnerability\",\"message\":\"RUSTSEC-2026-0001 detected\",\"graphs\":[{\"Krate\":{\"name\":\"bad\",\"version\":\"1.0.0\"}}]}}\n",
            "{\"type\":\"summary\",\"fields\":{\"advisories\":{\"errors\":1}}}\n",
            "not json at all\n",
            "{\"type\":\"diagnostic\",\"fields\":{\"severity\":\"warning\",\"code\":\"duplicate\",\"message\":\"two versions of foo\"}}\n",
        );
        let findings = parse_cargo_deny(raw);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "Cargo.lock");
        assert_eq!(findings[0].severity, Severity::Medium, "advisory-first cap");
        assert_eq!(findings[0].title, "cargo-deny: vulnerability");
        assert!(findings[0].body.contains("dependency bad 1.0.0"));
        assert_eq!(findings[1].severity, Severity::Low);
    }

    #[test]
    fn empty_or_garbage_input_yields_no_findings() {
        assert!(parse_cargo_deny("").is_empty());
        assert!(parse_cargo_deny("plain text\n{}\n").is_empty());
    }

    #[test]
    fn coarse_granularity() {
        assert_eq!(granularity(), Granularity::Coarse);
    }
}