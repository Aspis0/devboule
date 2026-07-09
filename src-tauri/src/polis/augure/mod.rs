//! Augure — persisted, disposable Polis anomalies.
//!
//! This module provides the DATA LAYER ONLY (no Tauri commands, no scheduler,
//! no new checks). It defines:
//!   - [`SinRecord`] — a superset of the wire `UrbanSin` with persistence fields.
//!   - [`Disposition`] — the lifecycle state of a sin (Open | Ignored | Fixed).
//!   - [`compute_sin_id`] — deterministic id from (rel_path, rule_id, line,
//!     evidence), matching the censor house pattern (`\u{1f}`-separated SHA-256).
//!   - [`evidence_key`] — a stable, normalized digest of `evidence` so cosmetic
//!     re-phrasings don't mint new ids (lowercased, whitespace-collapsed).
//!   - [`to_records`] — pure conversion from the existing `UrbanSin` output
//!     of `sins.rs` into `SinRecord` values (rule_id inferred from sin_id).
//!
//! The persisted ledger is in [`ledger`].

pub mod ledger;

use crate::polis::model::UrbanSin;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// SinRecord — the persisted anomaly
// ---------------------------------------------------------------------------

/// A persisted, disposable Polis anomaly. Superset of the wire `UrbanSin`.
///
/// Fields serialized camelCase like the rest of the polis models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SinRecord {
    /// Deterministic id: `sha256(rel_path \u{1f} rule_id \u{1f} line \u{1f} evidence_key)`.
    /// 64 hex chars. Stable across re-runs for the same logical finding.
    #[serde(default)]
    pub id: String,
    /// Project-relative path with forward slashes. Empty string for project-level
    /// sins (reserved for future use; today all sins are per-file — the
    /// `_project.json` shard is exercised only by tests).
    #[serde(default)]
    pub rel_path: String,
    /// Which check produced this sin: `"secret"`, `"dep-cycle"`, `"todo-density"`,
    /// `"dead-export"`, `"env-missing"`.
    #[serde(default)]
    pub rule_id: String,
    /// 1-based line number, or `None` for file-level sins.
    #[serde(default)]
    pub line: Option<u32>,
    /// `"smoke"` | `"fire"` | `"inferno"` — reuses polis::model severity consts.
    #[serde(default)]
    pub severity: String,
    /// English, human-readable — the existing UrbanSin description.
    #[serde(default)]
    pub description: String,
    /// The measured fact that produced this sin (e.g. `"3 TODO markers"`,
    /// `"cycle: a.rs -> b.rs -> a.rs"`). Used in the id via `evidence_key`.
    #[serde(default)]
    pub evidence: String,
    /// SHA-256 of the file content the sin was evaluated at.
    /// Empty string for project-level sins (cycles).
    #[serde(default)]
    pub content_hash: String,
    /// Lifecycle state: `Open`, `Ignored`, or `Fixed`.
    #[serde(default)]
    pub disposition: Disposition,
    /// RFC 3339 UTC timestamp of first detection.
    #[serde(default)]
    pub created_at: String,
    /// RFC 3339 UTC timestamp of last state change.
    #[serde(default)]
    pub updated_at: String,
    /// Id of the last main-coder directive dispatched for this sin.
    /// Cleared when the file's content hash changes (different-hash upsert).
    #[serde(default)]
    pub fix_directive_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Disposition
// ---------------------------------------------------------------------------

/// Lifecycle state of a persisted sin.
///
/// - `Open`: active, visible in the city.
/// - `Ignored`: user-dismissed (D8 Ignore action). The checker may re-evaluate
///   on content-hash change.
/// - `Fixed`: the checker observed the condition gone. Human attempts to set
///   `Fixed` are rejected — the checker, not the coder, is the arbiter of fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    #[default]
    Open,
    Ignored,
    Fixed,
}

// ---------------------------------------------------------------------------
// Deterministic id (house pattern: sha256 over \u{1f}-separated fields)
// ---------------------------------------------------------------------------

/// Compute the deterministic sin id.
///
/// Fields joined by `\u{1f}` (unit separator) so field-boundary collisions are
/// impossible (e.g. `("ab","c")` vs `("a","bc")` always produce different ids).
///
/// `evidence_key(evidence)` is used in place of raw evidence so cosmetic
/// re-phrasings (capitalization, extra whitespace) don't mint new ids.
pub fn compute_sin_id(
    rel_path: &str,
    rule_id: &str,
    line: Option<u32>,
    evidence: &str,
) -> String {
    let line_token = match line {
        Some(n) => n.to_string(),
        None => String::new(),
    };
    let ek = evidence_key(evidence);
    let mut hasher = Sha256::new();
    hasher.update(rel_path.as_bytes());
    hasher.update([0x1f]);
    hasher.update(rule_id.as_bytes());
    hasher.update([0x1f]);
    hasher.update(line_token.as_bytes());
    hasher.update([0x1f]);
    hasher.update(ek.as_bytes());
    hex::encode(hasher.finalize())
}

/// Normalize evidence text for stable id computation: lowercased, all whitespace
/// runs collapsed to a single space, trimmed. Cosmetic re-phrasings (extra spaces,
/// line breaks, capitalization changes) produce the same key.
pub fn evidence_key(evidence: &str) -> String {
    let lower = evidence.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_space = false;
    for ch in lower.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// DetectedSin — internal pipeline type carrying rule_id explicitly
// ---------------------------------------------------------------------------

/// A sin freshly detected by one of the checks in `sins.rs`, carrying the
/// `rule_id` and `evidence` explicitly so `to_records` no longer needs to
/// re-infer them from `sin_id` strings.
///
/// The wire `UrbanSin` is unchanged; this is an internal pipeline wrapper.
/// `Serialize` is derived so redaction tests can assert no secret survives
/// anywhere in the full struct (including `evidence`), not just in the wire sin.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedSin {
    pub sin: UrbanSin,
    /// `"secret"`, `"dep-cycle"`, `"todo-density"`, `"dead-export"`, `"env-missing"`.
    pub rule_id: &'static str,
    /// The measured fact (e.g. `"secret at line 1"`, `"5 todo/fixme/hack markers"`).
    pub evidence: String,
    /// 1-based line number, or `None` for file-level sins.
    pub line: Option<u32>,
}

// ---------------------------------------------------------------------------
// Conversion from DetectedSin → SinRecord
// ---------------------------------------------------------------------------

/// Convert `DetectedSin` values into `SinRecord` values ready for the ledger.
/// Uses the explicit `rule_id`, `evidence`, and `line` from each `DetectedSin`
/// — no inference from `sin_id` strings.
///
/// `rel_path_by_file_id`: file_id → project-relative path (forward slashes).
/// `content_hash_by_file_id`: file_id → SHA-256 content hash.
/// Sins with `file_id: None` get `rel_path: ""` and `content_hash: ""`
/// (the `_project.json` shard is reserved for future project-level sins;
/// today all sins are per-file).
pub fn to_records(
    sins: &[DetectedSin],
    rel_path_by_file_id: &HashMap<String, String>,
    content_hash_by_file_id: &HashMap<String, String>,
) -> Vec<SinRecord> {
    let now = now_stamp();
    sins.iter()
        .map(|ds| {
            let rel_path = ds
                .sin
                .file_id
                .as_ref()
                .and_then(|fid| rel_path_by_file_id.get(fid))
                .cloned()
                .unwrap_or_default();
            let content_hash = ds
                .sin
                .file_id
                .as_ref()
                .and_then(|fid| content_hash_by_file_id.get(fid))
                .cloned()
                .unwrap_or_default();
            let id = compute_sin_id(&rel_path, ds.rule_id, ds.line, &ds.evidence);
            SinRecord {
                id,
                rel_path,
                rule_id: ds.rule_id.to_string(),
                line: ds.line,
                severity: ds.sin.severity.clone(),
                description: ds.sin.description.clone(),
                evidence: ds.evidence.clone(),
                content_hash,
                disposition: Disposition::Open,
                created_at: now.clone(),
                updated_at: now.clone(),
                fix_directive_id: None,
            }
        })
        .collect()
}

/// RFC 3339 UTC now stamp, matching `censor::now_stamp`.
fn now_stamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- evidence_key ----

    #[test]
    fn evidence_key_lowercases_and_collapses_whitespace() {
        assert_eq!(evidence_key("Hello  World"), "hello world");
        assert_eq!(evidence_key("  FOO\nBAR\tBAZ  "), "foo bar baz");
        assert_eq!(evidence_key("nochange"), "nochange");
        assert_eq!(evidence_key(""), "");
    }

    #[test]
    fn evidence_key_same_for_cosmetic_variations() {
        let a = "3 TODO/FIXME/HACK markers";
        let b = "  3  todo/fixme/hack   MARKERS  ";
        assert_eq!(evidence_key(a), evidence_key(b));
    }

    // ---- compute_sin_id ----

    #[test]
    fn compute_sin_id_is_deterministic() {
        let a = compute_sin_id("src/a.rs", "secret", Some(1), "secret at line 1");
        let b = compute_sin_id("src/a.rs", "secret", Some(1), "secret at line 1");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn compute_sin_id_differs_when_inputs_differ() {
        let base = compute_sin_id("src/a.rs", "secret", Some(1), "secret at line 1");
        assert_ne!(base, compute_sin_id("src/b.rs", "secret", Some(1), "secret at line 1"));
        assert_ne!(base, compute_sin_id("src/a.rs", "todo-density", Some(1), "secret at line 1"));
        assert_ne!(base, compute_sin_id("src/a.rs", "secret", Some(2), "secret at line 1"));
        assert_ne!(base, compute_sin_id("src/a.rs", "secret", Some(1), "secret at line 2"));
        // None vs Some(0) must differ.
        assert_ne!(
            compute_sin_id("src/a.rs", "secret", None, "secret at line 1"),
            compute_sin_id("src/a.rs", "secret", Some(0), "secret at line 1")
        );
    }

    #[test]
    fn compute_sin_id_cosmetic_evidence_does_not_change_id() {
        let a = compute_sin_id("src/a.rs", "secret", Some(1), "secret at line 1");
        let b = compute_sin_id("src/a.rs", "secret", Some(1), "SECRET AT LINE 1  ");
        assert_eq!(a, b, "cosmetic evidence variations must produce same id");
    }

    #[test]
    fn compute_sin_id_no_separator_collision() {
        let x = compute_sin_id("ab", "c", Some(1), "d");
        let y = compute_sin_id("a", "bc", Some(1), "d");
        assert_ne!(x, y);
    }

    // ---- to_records ----

    fn mk_detected(sin_id: &str, severity: &str, description: &str, file_id: Option<&str>, rule_id: &'static str, evidence: &str, line: Option<u32>) -> DetectedSin {
        DetectedSin {
            sin: UrbanSin {
                sin_id: sin_id.to_string(),
                severity: severity.to_string(),
                description: description.to_string(),
                auto_detectable: true,
                file_id: file_id.map(|s| s.to_string()),
            },
            rule_id,
            evidence: evidence.to_string(),
            line,
        }
    }

    #[test]
    fn to_records_converts_sins_with_rel_path_and_hash() {
        let sins = vec![
            mk_detected("sin-secret-1-fid12345", "inferno", "Hardcoded secret-like value at line 1", Some("fid-secret"), "secret", "secret at line 1", Some(1)),
            mk_detected("sin-todo-fid12345", "smoke", "5 TODO/FIXME/HACK comments accumulated", Some("fid-todo"), "todo-density", "5 todo/fixme/hack markers", None),
        ];
        let mut rels = HashMap::new();
        rels.insert("fid-secret".to_string(), "src/secret.rs".to_string());
        rels.insert("fid-todo".to_string(), "src/todo.rs".to_string());
        let mut hashes = HashMap::new();
        hashes.insert("fid-secret".to_string(), "abc123".to_string());
        hashes.insert("fid-todo".to_string(), "def456".to_string());

        let records = to_records(&sins, &rels, &hashes);
        assert_eq!(records.len(), 2);

        let sec = records.iter().find(|r| r.rule_id == "secret").unwrap();
        assert_eq!(sec.rel_path, "src/secret.rs");
        assert_eq!(sec.content_hash, "abc123");
        assert_eq!(sec.line, Some(1));
        assert_eq!(sec.severity, "inferno");
        assert_eq!(sec.disposition, Disposition::Open);
        assert!(!sec.id.is_empty());
        assert!(!sec.created_at.is_empty());

        let todo = records.iter().find(|r| r.rule_id == "todo-density").unwrap();
        assert_eq!(todo.rel_path, "src/todo.rs");
        assert_eq!(todo.content_hash, "def456");
        assert_eq!(todo.line, None);
    }

    #[test]
    fn to_records_fileless_sin_gets_empty_path_and_hash() {
        let sins = vec![
            mk_detected("sin-cycle-fedcba98", "fire", "Cyclic import detected in the road graph", None, "dep-cycle", "cyclic import detected", None),
        ];
        let rels = HashMap::new();
        let hashes = HashMap::new();
        let records = to_records(&sins, &rels, &hashes);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.rel_path, "");
        assert_eq!(r.content_hash, "");
        assert_eq!(r.rule_id, "dep-cycle");
    }

    #[test]
    fn to_records_id_is_deterministic_across_calls() {
        let sins = vec![
            mk_detected("sin-secret-1-abcdef01", "inferno", "Hardcoded secret-like value at line 1", Some("fid1"), "secret", "secret at line 1", Some(1)),
        ];
        let mut rels = HashMap::new();
        rels.insert("fid1".to_string(), "src/a.rs".to_string());
        let mut hashes = HashMap::new();
        hashes.insert("fid1".to_string(), "hash1".to_string());

        let r1 = to_records(&sins, &rels, &hashes);
        let r2 = to_records(&sins, &rels, &hashes);
        assert_eq!(r1[0].id, r2[0].id, "id must be deterministic across calls");
    }
}
