//! Censor ledger serde types.
//!
//! Every type crosses the IPC boundary (Tauri events/commands later) AND the
//! cross-process boundary to the Python MCP server, which lock-reads these same
//! shard files. So the on-disk shape is a CONTRACT: camelCase keys, and every
//! optional field carries `#[serde(default)]` for forward-compat — a shard
//! written by a newer build (or hand-edited) must still deserialize on an older
//! build without panicking. This mirrors the tolerance of `AgentLedgerEntry`
//! (`backend/agents.rs`): one unknown/missing field must never brick a read.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Finding severity. `high|medium|low` vocab — there is intentionally NO
/// critical/info bucket. Serialized lowercase to match the TS union.
///
/// `Default` is `Medium` so a shard missing the key (newer/hand-edited build)
/// still deserializes to a conservative value rather than hard-erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    #[default]
    Medium,
    Low,
}

/// Finding category. `dead-code` is kebab-cased over the wire to match the TS
/// union in the plan; the rest are single lowercase words.
///
/// `Default` is `Correctness` so a shard missing the key still deserializes to a
/// neutral, non-security bucket rather than hard-erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Security,
    #[default]
    Correctness,
    Complexity,
    Duplication,
    DeadCode,
    Style,
}

impl Category {
    /// Canonical over-the-wire token for this category, IDENTICAL to what serde
    /// produces (kebab-case). Used by `Finding::compute_id` so the stable id never
    /// depends on a JSON round-trip that could silently degrade to an empty
    /// string on serialization failure (collapsing distinct categories into one
    /// colliding hash).
    pub fn id_token(self) -> &'static str {
        match self {
            Category::Security => "security",
            Category::Correctness => "correctness",
            Category::Complexity => "complexity",
            Category::Duplication => "duplication",
            Category::DeadCode => "dead-code",
            Category::Style => "style",
        }
    }
}

/// Confidence of the finding. Deterministic linters emit `suspected`; the final
/// reviewer (or an explicit confirm) promotes to `confirmed`.
///
/// `Default` is `Suspected` so a shard missing the key deserializes to the
/// lower-confidence value rather than hard-erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    #[default]
    Suspected,
    Confirmed,
}

/// Lifecycle disposition. `fp` = false positive. Set by a coder/reviewer via the
/// MCP `censor_dispose` tool and PRESERVED across re-reviews of the same id at
/// the same content-hash (see `ledger::supersede`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    #[default]
    Open,
    Fixed,
    Fp,
    Wontfix,
}

/// One audit-trail entry on a finding: who did what, when. Appended (never
/// rewritten) so the history survives supersede.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceEntry {
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub action: String,
    /// The disposing principal's role (`coder` / `verifier`), or "" for a
    /// machine/legacy entry. CONTRACT MIRROR: the Python MCP writer
    /// (`dispose_censor_finding`) stamps this so the coder-cannot-override-verifier
    /// precedence (WARNING 2) survives cross-process round-trips. `#[serde(default)]`
    /// keeps older shards (no `role` key) readable.
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub at: String,
}

/// A single code-review finding for one file.
///
/// Every optional/list field is `#[serde(default)]` so an older build reading a
/// shard written by a newer one (or a hand-edited shard) never fails on a
/// missing key. `id` is deterministic over (file, line, category, source, title)
/// so the same issue re-flagged across re-reviews keeps the same id — that
/// stability is what lets supersede preserve a coder's disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub content_hash: String,
    /// 1-based line, or `None` for a file-level finding.
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub category: Category,
    /// Tool name (e.g. "clippy", "gitleaks") or "gemma".
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub title: String,
    /// English. NEVER raw tool stdout that could carry a secret value.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub verdict: Verdict,
    #[serde(default)]
    pub disposition: Disposition,
    #[serde(default)]
    pub provenance: Vec<ProvenanceEntry>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub commit: Option<String>,
}

impl Finding {
    /// Stable id for a finding: sha256 hex of the identity tuple
    /// (file_rel_path, line, category, source, title). Deterministic and stable
    /// across re-reviews of the same issue, so a coder's disposition survives a
    /// re-flag. Any differing input yields a different id.
    ///
    /// Fields are joined with a `\u{1f}` (unit separator) so distinct field
    /// boundaries can never collide via concatenation (e.g. ("ab","c") vs
    /// ("a","bc")).
    #[allow(dead_code)] // first non-test caller is the A2 runners (id each finding).
    pub fn compute_id(
        file_rel_path: &str,
        line: Option<u32>,
        category: Category,
        source: &str,
        title: &str,
    ) -> String {
        let line_token = match line {
            Some(n) => n.to_string(),
            None => String::new(),
        };
        let category_token = category.id_token();
        let mut hasher = Sha256::new();
        hasher.update(file_rel_path.as_bytes());
        hasher.update([0x1f]);
        hasher.update(line_token.as_bytes());
        hasher.update([0x1f]);
        hasher.update(category_token.as_bytes());
        hasher.update([0x1f]);
        hasher.update(source.as_bytes());
        hasher.update([0x1f]);
        hasher.update(title.as_bytes());
        hex::encode(hasher.finalize())
    }
}

/// One per-file shard: the file's current content-hash plus its findings array.
/// Stored at `<root>/.aspis-censor/<sha256(fileRelPath)>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CensorShard {
    #[serde(default)]
    pub file_rel_path: String,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_finding() -> Finding {
        Finding {
            id: "abc".into(),
            file: "src/main.rs".into(),
            content_hash: "hash1".into(),
            line: Some(42),
            severity: Severity::High,
            category: Category::Security,
            source: "gitleaks".into(),
            title: "Hardcoded secret".into(),
            body: "A credential pattern was detected.".into(),
            verdict: Verdict::Suspected,
            disposition: Disposition::Open,
            provenance: vec![ProvenanceEntry {
                actor: "censor".into(),
                action: "created".into(),
                role: String::new(),
                at: "2026-06-05T00:00:00Z".into(),
            }],
            created_at: "2026-06-05T00:00:00Z".into(),
            commit: Some("deadbeef".into()),
        }
    }

    #[test]
    fn shard_serde_round_trip_uses_camel_case() {
        let shard = CensorShard {
            file_rel_path: "src/main.rs".into(),
            content_hash: "hash1".into(),
            updated_at: "2026-06-05T00:00:00Z".into(),
            findings: vec![sample_finding()],
        };
        let json = serde_json::to_string(&shard).unwrap();

        // camelCase keys verified explicitly (the cross-process contract).
        assert!(json.contains("\"fileRelPath\""), "json: {json}");
        assert!(json.contains("\"contentHash\""), "json: {json}");
        assert!(json.contains("\"updatedAt\""), "json: {json}");
        assert!(json.contains("\"createdAt\""), "json: {json}");
        assert!(!json.contains("file_rel_path"), "snake_case leaked: {json}");

        // Enum values lowercase / kebab-case over the wire.
        assert!(json.contains("\"severity\":\"high\""), "json: {json}");
        assert!(json.contains("\"category\":\"security\""), "json: {json}");
        assert!(json.contains("\"verdict\":\"suspected\""), "json: {json}");
        assert!(json.contains("\"disposition\":\"open\""), "json: {json}");

        let back: CensorShard = serde_json::from_str(&json).unwrap();
        assert_eq!(shard, back);
    }

    #[test]
    fn dead_code_category_is_kebab_case() {
        let json = serde_json::to_string(&Category::DeadCode).unwrap();
        assert_eq!(json, "\"dead-code\"");
        let back: Category = serde_json::from_str("\"dead-code\"").unwrap();
        assert_eq!(back, Category::DeadCode);
    }

    #[test]
    fn forward_compat_shard_and_finding_scalars_default() {
        // A minimal shard whose finding still carries severity/category/verdict:
        // every shard-level field and the remaining finding scalars must default.
        let json = r#"{
            "findings": [
                { "severity": "low", "category": "style", "verdict": "suspected" }
            ]
        }"#;
        let shard: CensorShard = serde_json::from_str(json).unwrap();
        assert_eq!(shard.file_rel_path, "");
        assert_eq!(shard.content_hash, "");
        assert_eq!(shard.updated_at, "");
        assert_eq!(shard.findings.len(), 1);
        let f = &shard.findings[0];
        assert_eq!(f.id, "");
        assert_eq!(f.line, None);
        assert_eq!(f.disposition, Disposition::Open);
        assert!(f.provenance.is_empty());
        assert_eq!(f.commit, None);
    }

    #[test]
    fn finding_missing_severity_category_verdict_use_enum_defaults() {
        // The real forward-compat guard: a finding JSON that OMITS severity,
        // category AND verdict entirely must still deserialize (no hard error),
        // filling the documented enum defaults. A regression that drops the
        // `#[serde(default)]` on these fields or the `Default` derive on the enums
        // turns this from a graceful default into a deserialize failure.
        let json = r#"{ "title": "legacy finding without enum keys" }"#;
        let f: Finding = serde_json::from_str(json).unwrap();
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.category, Category::Correctness);
        assert_eq!(f.verdict, Verdict::Suspected);
        assert_eq!(f.title, "legacy finding without enum keys");

        // And the same omission inside a shard's findings array.
        let shard_json = r#"{ "findings": [ {} ] }"#;
        let shard: CensorShard = serde_json::from_str(shard_json).unwrap();
        assert_eq!(shard.findings.len(), 1);
        assert_eq!(shard.findings[0].severity, Severity::Medium);
        assert_eq!(shard.findings[0].category, Category::Correctness);
        assert_eq!(shard.findings[0].verdict, Verdict::Suspected);
    }

    #[test]
    fn category_id_token_matches_serde_token() {
        // compute_id relies on id_token producing EXACTLY serde's kebab-case token.
        for c in [
            Category::Security,
            Category::Correctness,
            Category::Complexity,
            Category::Duplication,
            Category::DeadCode,
            Category::Style,
        ] {
            let serde_token = serde_json::to_string(&c).unwrap();
            let serde_token = serde_token.trim_matches('"');
            assert_eq!(c.id_token(), serde_token, "mismatch for {c:?}");
        }
    }

    #[test]
    fn unknown_extra_field_does_not_break_read() {
        // A newer build adds a field; an older build must still parse the shard.
        let json = r#"{
            "fileRelPath": "src/a.rs",
            "contentHash": "h",
            "updatedAt": "t",
            "someFutureField": { "nested": true },
            "findings": []
        }"#;
        let shard: CensorShard = serde_json::from_str(json).unwrap();
        assert_eq!(shard.file_rel_path, "src/a.rs");
        assert!(shard.findings.is_empty());
    }

    #[test]
    fn compute_id_is_deterministic() {
        let a = Finding::compute_id("src/a.rs", Some(10), Category::Security, "clippy", "t");
        let b = Finding::compute_id("src/a.rs", Some(10), Category::Security, "clippy", "t");
        assert_eq!(a, b);
        // sha256 hex length.
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn compute_id_differs_when_any_input_differs() {
        let base = Finding::compute_id("src/a.rs", Some(10), Category::Security, "clippy", "t");
        assert_ne!(
            base,
            Finding::compute_id("src/b.rs", Some(10), Category::Security, "clippy", "t")
        );
        assert_ne!(
            base,
            Finding::compute_id("src/a.rs", Some(11), Category::Security, "clippy", "t")
        );
        assert_ne!(
            base,
            Finding::compute_id("src/a.rs", Some(10), Category::Correctness, "clippy", "t")
        );
        assert_ne!(
            base,
            Finding::compute_id("src/a.rs", Some(10), Category::Security, "ruff", "t")
        );
        assert_ne!(
            base,
            Finding::compute_id("src/a.rs", Some(10), Category::Security, "clippy", "t2")
        );
        // None line vs Some(0) must not collide.
        assert_ne!(
            Finding::compute_id("src/a.rs", None, Category::Security, "clippy", "t"),
            Finding::compute_id("src/a.rs", Some(0), Category::Security, "clippy", "t")
        );
    }

    #[test]
    fn compute_id_no_separator_collision() {
        // ("ab","c") must differ from ("a","bc") for adjacent fields.
        let x = Finding::compute_id("ab", Some(1), Category::Style, "c", "t");
        let y = Finding::compute_id("a", Some(1), Category::Style, "bc", "t");
        assert_ne!(x, y);
    }
}
