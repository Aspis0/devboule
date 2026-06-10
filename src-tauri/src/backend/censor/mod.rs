//! Censor — continuous, local-first, per-file code-review subsystem.
//!
//! Sub-phase A1 (this module set) is PURE logic + file IO: the on-disk ledger
//! schema, the per-file shard storage with the cross-process lock + atomic-write
//! pattern, the content-hash supersede logic, and the tool→severity normalizers.
//! NO watcher, NO subprocess, NO Gemma — those land in A2/A3/A4.
//!
//! Storage lives in `<projectRoot>/.aspis-censor/` as one JSON shard per file
//! (`<sha256(fileRelPath)>.json`), each guarded by a `<shard>.lock` sidecar that
//! interoperates with the Python MCP writer exactly like `.aspis-agents.json`.

pub mod commands;
pub mod detect;
pub mod gemma;
pub mod ledger;
pub mod orchestrator;
pub mod runners;
pub mod schema;
pub mod severity;
pub mod watch;

/// Per-project directory holding the censor shards. Lives in the watched project
/// root; A3 adds it to the watcher ignore set (self-trigger guard) and the panel
/// recommends gitignoring it.
pub const CENSOR_DIR: &str = ".aspis-censor";

/// The single shared audit timestamp helper for the Censor subsystem.
///
/// Returns an RFC3339 UTC stamp (e.g. `2026-06-05T12:34:56.789+00:00`). The
/// frontend parses these with `new Date(stamp)`, which requires RFC3339 — a bare
/// epoch-seconds string would parse as `Invalid Date`. Both the orchestrator
/// (finding `createdAt`/provenance) and the dispose command stamp through here so
/// the format can never drift between the two write paths. Matches the
/// `chrono::Utc::now().to_rfc3339()` convention used by the agent ledger.
pub fn now_stamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_stamp_is_rfc3339_parseable() {
        let stamp = now_stamp();
        // The frontend does `new Date(stamp)`; that demands RFC3339, which chrono
        // round-trips. A bare epoch-seconds string would fail this parse.
        let parsed = chrono::DateTime::parse_from_rfc3339(&stamp);
        assert!(parsed.is_ok(), "now_stamp must be RFC3339, got: {stamp}");
        // Sanity: it is a UTC stamp ('+00:00' or 'Z' offset).
        assert!(
            stamp.ends_with("+00:00") || stamp.ends_with('Z'),
            "expected a UTC offset, got: {stamp}"
        );
    }
}
