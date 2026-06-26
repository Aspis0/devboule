//! Slice 5b: the data contract + PURE state machine for the Claude Code consent
//! file-bridge.
//!
//! Claude Code's `PreToolUse` hook is a SEPARATE OS process (no `AppHandle`, no shared
//! in-memory waiter map like Codex uses). It therefore round-trips its tool-permission
//! request through the SAME `.aspis-agents.json` file-bridge the git-push gate uses:
//! the hook appends a `pending_approval` `ConsentBridgeRequest` to the `consentRequests`
//! queue, lights the requesting session's `needsUser` bell, then BOUNDED-polls the
//! request's verdict. The human answers via the existing consent UI; `respond_cloud_consent`
//! claims the request terminal (`allowed`/`denied`) under the lock. The hook then prints
//! Claude's `permissionDecision` and exits.
//!
//! This file is the HEADLESS, IO-free core, mirroring `git_push.rs`:
//!   * the serde contract (camelCase, every optional/list field `#[serde(default)]`/
//!     `skip_serializing_if`, read leniently so one malformed entry never bricks the
//!     whole-state read),
//!   * the status lifecycle `pending_approval -> (allowed | denied | timeout)` as a
//!     pure no-double-act transition helper (`claim_terminal`),
//!   * `cap_consent_requests` — the bounded queue eviction (oldest TERMINAL first;
//!     never evict a `pending_approval`).
//!
//! The hook's STDIN tool-name -> ConsentKind mapping and the Claude hook output JSON
//! live in the hook bin (`src/bin/claude_consent_hook.rs`) as pure testable fns; this
//! module owns only the on-disk request contract + queue mechanics.

use crate::backend::broker::ConsentKind;
use serde::{Deserialize, Serialize};

/// Maximum number of consent requests retained in the in-file queue. Beyond this,
/// `cap_consent_requests` evicts the oldest TERMINAL requests (never a pending one).
/// Mirrors `git_push::MAX_PUSH_REQUESTS`.
pub const MAX_CONSENT_REQUESTS: usize = 50;

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Consent-request lifecycle status. snake_case over the wire to match the file-bridge
/// convention (the git-push gate's status strings) and a possible hand-edit.
///
/// `Default` is `PendingApproval` so a request missing the key (hand-edited / older
/// writer) deserializes to the queue's entry state rather than hard-erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConsentBridgeStatus {
    /// Written by the hook; awaiting the human's verdict.
    #[default]
    PendingApproval,
    /// Terminal: the human allowed the tool call (AllowOnce / AllowRemember).
    Allowed,
    /// Terminal: the human denied the tool call.
    Denied,
    /// Terminal: the hook's bounded poll gave up before the human acted (fail-closed →
    /// the hook prints `deny`). Stamped by the hook's poll path so a later answer no-ops.
    Timeout,
}

impl ConsentBridgeStatus {
    /// `true` once the request has reached a terminal state (no further transition).
    /// Only terminal requests are eligible for eviction.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ConsentBridgeStatus::Allowed
                | ConsentBridgeStatus::Denied
                | ConsentBridgeStatus::Timeout
        )
    }
}

// ---------------------------------------------------------------------------
// Request (hook -> app, lifecycle owned by the human-driven respond command)
// ---------------------------------------------------------------------------

/// One Claude consent request in the `.aspis-agents.json` `consentRequests` queue.
/// The hook appends it as `status:"pending_approval"`; `respond_cloud_consent` claims
/// it terminal (`allowed`/`denied`) when the human answers, and the hook's poll may
/// stamp `timeout` if the human never does.
///
/// camelCase over the wire; every optional field `#[serde(default)]`/
/// `skip_serializing_if` so a partial / hand-edited / older-writer request still
/// deserializes (the state-level `lenient_consent_requests` then drops only an entry
/// that fails entirely, never bricking the whole state read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentBridgeRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The requesting Claude session id (the cloud agent whose hook fired). Used to
    /// set/clear its `needs_user` bell and to attribute the request in the card.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    /// The project the agent is working in (display + scoping context).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    /// What category of action the agent is asking to perform (Exec / Patch / …).
    pub kind: ConsentKind,
    /// Human-readable context for the card (the bash command, or the file being
    /// edited). Display-only prose; for a machine-readable value use `path`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Machine-readable path the request concerns (the edited file for Patch). `None`
    /// for Exec / kinds with no associated path. NO-CHURN: skip-if-none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub status: ConsentBridgeStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Pure state transition (no double-act)
// ---------------------------------------------------------------------------

/// CLAIM a `pending_approval` request terminal with `status` (must itself be terminal).
/// Returns `true` and mutates the request only on the FIRST transition out of
/// `pending_approval`; a second claim (double-click, or a claim after the hook's poll
/// already stamped `timeout`) is refused (returns `false`, no mutation). This is the
/// single anchor that makes the human's answer idempotent.
///
/// `status` MUST be terminal (`Allowed`/`Denied`/`Timeout`); passing `PendingApproval`
/// is a programming error and is refused (returns `false`) so the request can never be
/// "claimed" back into a non-terminal state.
pub fn claim_terminal(req: &mut ConsentBridgeRequest, status: ConsentBridgeStatus) -> bool {
    if req.status != ConsentBridgeStatus::PendingApproval {
        return false; // already terminal — no double-act.
    }
    if !status.is_terminal() {
        return false; // refuse to "claim" into a non-terminal status.
    }
    req.status = status;
    true
}

/// `true` once the request has reached a terminal state. Thin forward to the status
/// helper, kept as a request-level predicate for symmetry with `git_push`.
pub fn is_terminal(req: &ConsentBridgeRequest) -> bool {
    req.status.is_terminal()
}

// ---------------------------------------------------------------------------
// Bounded queue eviction
// ---------------------------------------------------------------------------

/// Cap the consent-request queue at `max`, evicting the OLDEST TERMINAL requests first.
/// A `pending_approval` request is NEVER evicted — it represents an unanswered ask whose
/// hook is actively polling; dropping it would orphan that poll (the hook would see the
/// request vanish and fail-closed `deny`). If the queue is over `max` but every excess
/// slot is pending, the queue is left larger than `max` rather than dropping live work.
/// "Oldest" is by `created_at` (RFC3339 lexicographic), tie-broken on `id`. Mirrors
/// `git_push::cap_push_requests`.
pub fn cap_consent_requests(requests: &mut Vec<ConsentBridgeRequest>, max: usize) {
    if requests.len() <= max {
        return;
    }
    let mut to_remove = requests.len() - max;

    let mut terminal_idx: Vec<usize> = requests
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status.is_terminal())
        .map(|(i, _)| i)
        .collect();
    terminal_idx.sort_by(|&a, &b| {
        requests[a]
            .created_at
            .cmp(&requests[b].created_at)
            .then_with(|| requests[a].id.cmp(&requests[b].id))
    });

    let mut remove_flags = vec![false; requests.len()];
    for &idx in terminal_idx.iter() {
        if to_remove == 0 {
            break;
        }
        remove_flags[idx] = true;
        to_remove -= 1;
    }

    let mut i = 0;
    requests.retain(|_| {
        let keep = !remove_flags[i];
        i += 1;
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, status: ConsentBridgeStatus, created_at: &str) -> ConsentBridgeRequest {
        ConsentBridgeRequest {
            id: id.into(),
            agent_id: "claude-1".into(),
            project_id: "proj-1".into(),
            kind: ConsentKind::Exec,
            detail: "cargo build".into(),
            path: None,
            status,
            created_at: created_at.into(),
        }
    }

    // -- serde --------------------------------------------------------------

    #[test]
    fn request_round_trip_uses_camel_case() {
        let r = ConsentBridgeRequest {
            id: "r1".into(),
            agent_id: "claude-1".into(),
            project_id: "proj-1".into(),
            kind: ConsentKind::Patch,
            detail: "edit src/main.rs".into(),
            path: Some("/repo/src/main.rs".into()),
            status: ConsentBridgeStatus::PendingApproval,
            created_at: "2026-06-26T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"agentId\""), "json: {json}");
        assert!(json.contains("\"projectId\""), "json: {json}");
        assert!(json.contains("\"createdAt\""), "json: {json}");
        assert!(json.contains("\"kind\":\"patch\""), "json: {json}");
        assert!(
            json.contains("\"status\":\"pending_approval\""),
            "json: {json}"
        );
        assert!(!json.contains("project_id"), "snake leaked: {json}");
        let back: ConsentBridgeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn status_snake_case_strings_match_contract() {
        for (s, tok) in [
            (ConsentBridgeStatus::PendingApproval, "pending_approval"),
            (ConsentBridgeStatus::Allowed, "allowed"),
            (ConsentBridgeStatus::Denied, "denied"),
            (ConsentBridgeStatus::Timeout, "timeout"),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{tok}\""));
            let back: ConsentBridgeStatus = serde_json::from_str(&format!("\"{tok}\"")).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn partial_json_defaults_everything() {
        // Only the non-default field (kind has no Default) plus id present.
        let json = r#"{ "id": "r1", "kind": "exec" }"#;
        let r: ConsentBridgeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.id, "r1");
        assert_eq!(r.status, ConsentBridgeStatus::PendingApproval); // enum default
        assert!(r.agent_id.is_empty());
        assert!(r.project_id.is_empty());
        assert_eq!(r.path, None);
        assert!(r.detail.is_empty());
    }

    #[test]
    fn no_churn_empty_optionals_absent() {
        let mut r = request("r1", ConsentBridgeStatus::PendingApproval, "2026-06-26T00:00:00Z");
        r.path = None;
        r.detail = String::new();
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"path\""), "json: {json}");
        assert!(!json.contains("\"detail\""), "json: {json}");
    }

    // -- claim_terminal (no double-act) -------------------------------------

    #[test]
    fn claim_terminal_first_allow_wins() {
        let mut r = request("r1", ConsentBridgeStatus::PendingApproval, "t");
        assert!(claim_terminal(&mut r, ConsentBridgeStatus::Allowed));
        assert_eq!(r.status, ConsentBridgeStatus::Allowed);
    }

    #[test]
    fn claim_terminal_deny_wins() {
        let mut r = request("r1", ConsentBridgeStatus::PendingApproval, "t");
        assert!(claim_terminal(&mut r, ConsentBridgeStatus::Denied));
        assert_eq!(r.status, ConsentBridgeStatus::Denied);
    }

    #[test]
    fn claim_terminal_second_is_rejected_no_double_act() {
        // Two clicks: A allows; B's claim against the now-allowed request must fail
        // and must NOT clobber the recorded verdict.
        let mut r = request("r1", ConsentBridgeStatus::PendingApproval, "t");
        assert!(claim_terminal(&mut r, ConsentBridgeStatus::Allowed));
        assert!(!claim_terminal(&mut r, ConsentBridgeStatus::Denied));
        assert_eq!(r.status, ConsentBridgeStatus::Allowed, "verdict not clobbered");
    }

    #[test]
    fn claim_terminal_after_timeout_is_rejected() {
        // The hook's poll already stamped timeout; a late human answer must no-op.
        let mut r = request("r1", ConsentBridgeStatus::Timeout, "t");
        assert!(!claim_terminal(&mut r, ConsentBridgeStatus::Allowed));
        assert_eq!(r.status, ConsentBridgeStatus::Timeout);
    }

    #[test]
    fn claim_terminal_refuses_non_terminal_status() {
        // Passing PendingApproval is a programming error — refused, no mutation.
        let mut r = request("r1", ConsentBridgeStatus::PendingApproval, "t");
        assert!(!claim_terminal(&mut r, ConsentBridgeStatus::PendingApproval));
        assert_eq!(r.status, ConsentBridgeStatus::PendingApproval);
    }

    #[test]
    fn is_terminal_matches_status() {
        assert!(!is_terminal(&request("r", ConsentBridgeStatus::PendingApproval, "t")));
        assert!(is_terminal(&request("r", ConsentBridgeStatus::Allowed, "t")));
        assert!(is_terminal(&request("r", ConsentBridgeStatus::Denied, "t")));
        assert!(is_terminal(&request("r", ConsentBridgeStatus::Timeout, "t")));
    }

    // -- bounded queue ------------------------------------------------------

    #[test]
    fn cap_evicts_oldest_terminal_only() {
        let mut q = vec![
            request("a", ConsentBridgeStatus::Allowed, "2026-06-26T00:00:01Z"),
            request("b", ConsentBridgeStatus::PendingApproval, "2026-06-26T00:00:02Z"),
            request("c", ConsentBridgeStatus::Denied, "2026-06-26T00:00:03Z"),
        ];
        cap_consent_requests(&mut q, 2);
        // Oldest terminal ("a") evicted; the pending ("b") is never dropped.
        assert_eq!(q.len(), 2);
        assert!(q.iter().any(|r| r.id == "b"));
        assert!(q.iter().any(|r| r.id == "c"));
        assert!(!q.iter().any(|r| r.id == "a"));
    }

    #[test]
    fn cap_never_evicts_pending_even_over_max() {
        let mut q = vec![
            request("a", ConsentBridgeStatus::PendingApproval, "2026-06-26T00:00:01Z"),
            request("b", ConsentBridgeStatus::PendingApproval, "2026-06-26T00:00:02Z"),
            request("c", ConsentBridgeStatus::PendingApproval, "2026-06-26T00:00:03Z"),
        ];
        cap_consent_requests(&mut q, 1);
        // All pending -> nothing evicted, queue left larger than max.
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn cap_noop_under_max() {
        let mut q = vec![request("a", ConsentBridgeStatus::Allowed, "t")];
        cap_consent_requests(&mut q, 50);
        assert_eq!(q.len(), 1);
    }
}
