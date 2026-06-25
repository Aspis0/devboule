//! Permission broker — provider-agnostic consent types and transient-grant state.
//!
//! Slice 0 covers the LOCAL adapter only (net-blocked → prompt → next spawn unlocked).
//! Later slices will add `FolderWrite`/`Exec`/`Patch` kinds and the cloud adapters; the
//! types are kept intentionally minimal and general so the core does not need to change.
//!
//! # Design constraints (from the plan "Vincolo fisico")
//! - Seatbelt cannot be widened mid-run.
//! - The net flag is resolved ONCE per spawn in `claim_and_launch`.
//! - The worker thread has NO `AppHandle`.
//! - Consent is therefore "fail → prompt → applies at NEXT spawn/retry", never mid-run.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ──────────────────────────────────────────────
// Wire types (provider-agnostic)
// ──────────────────────────────────────────────

/// Category of permission the agent is requesting. Will grow with later slices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsentKind {
    Net,
    FolderWrite,
    Exec,
    Patch,
}

/// Structured event emitted to the frontend when an agent is blocked by a permission.
/// Event name: `sandbox://consent-request`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentRequest {
    pub kind: ConsentKind,
    pub project_id: String,
    /// The mini-coder agent id (`mini_agent_id(directive)`) for display in the UI.
    pub agent_id: String,
    /// Human-readable context string (e.g. the command that hit the block).
    pub detail: String,
}

/// Decision returned by the frontend via `grant_net_consent`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsentDecision {
    /// Persist `net_enabled = true` for the project (survives restart).
    AllowRemember,
    /// Grant network for the NEXT spawn only; the grant is consumed on first use.
    AllowOnce,
    /// No change — the next run will fail again (the user may retry manually).
    Deny,
}

// ──────────────────────────────────────────────
// Transient runtime state
// ──────────────────────────────────────────────

/// Managed singleton for per-project one-shot transient grants.
///
/// Registered with `.manage(PermissionBrokerState::new())` in `lib.rs`; retrieved via
/// `app.try_state::<PermissionBrokerState>()`.
///
/// Mirrors the `Arc<Mutex<HashSet<String>>>` pattern from `MiniCoderState` — plain
/// `Mutex` is sufficient here because this is the *only* interior mutable state and no
/// lock is held across any await or blocking call.
pub struct PermissionBrokerState {
    /// Project ids that have a pending one-shot net grant (consumed on first use).
    transient_net_grants: std::sync::Mutex<HashSet<String>>,
}

impl Default for PermissionBrokerState {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionBrokerState {
    pub fn new() -> Self {
        Self {
            transient_net_grants: std::sync::Mutex::new(HashSet::new()),
        }
    }

    /// Record a one-shot net grant for `project_id`.  Idempotent: inserting an already-
    /// present id is a no-op (still exactly one shot after this call).
    pub fn grant_net_once(&self, project_id: &str) {
        let mut set = self
            .transient_net_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.insert(project_id.to_string());
    }

    /// Consume the one-shot grant: returns `true` AND removes the entry if present,
    /// `false` (no mutation) if absent.  The caller should OR this with the persistent
    /// flag: `net_enabled || broker.take_net_grant(project_id)`.
    pub fn take_net_grant(&self, project_id: &str) -> bool {
        let mut set = self
            .transient_net_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.remove(project_id)
    }
}

// ──────────────────────────────────────────────
// Unit tests  (TDD: written before implementation — these define the contract)
// ──────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    // ── PermissionBrokerState one-shot grant ──────────────────────────────────

    #[test]
    fn take_net_grant_returns_false_when_no_grant() {
        let broker = PermissionBrokerState::new();
        assert!(!broker.take_net_grant("proj-1"));
    }

    #[test]
    fn take_net_grant_returns_true_exactly_once_after_grant() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-1");
        // First take: present → true, consumed.
        assert!(broker.take_net_grant("proj-1"));
        // Second take: already consumed → false.
        assert!(!broker.take_net_grant("proj-1"));
    }

    #[test]
    fn grant_is_per_project_id() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-a");
        // proj-b has no grant.
        assert!(!broker.take_net_grant("proj-b"));
        // proj-a still has it.
        assert!(broker.take_net_grant("proj-a"));
    }

    #[test]
    fn double_grant_is_idempotent_still_one_shot() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-1");
        broker.grant_net_once("proj-1"); // second insert → still in set once
        assert!(broker.take_net_grant("proj-1")); // consumed
        assert!(!broker.take_net_grant("proj-1")); // gone
    }

    #[test]
    fn multiple_projects_independent() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-1");
        broker.grant_net_once("proj-2");
        assert!(broker.take_net_grant("proj-1"));
        // proj-2 still present after proj-1 consumed
        assert!(broker.take_net_grant("proj-2"));
        assert!(!broker.take_net_grant("proj-1"));
        assert!(!broker.take_net_grant("proj-2"));
    }

    // ── FIX 1: re-insert on spawn failure ────────────────────────────────────

    /// Simulates the corrected claim_and_launch agentic path:
    /// take → spawn fails → re-insert → next spawn can take again.
    ///
    /// Without the re-insert the second take returns false and the user is never
    /// re-prompted (the grant is silently lost). This test pins that contract.
    #[test]
    fn take_reinsertion_on_spawn_failure_restores_grant() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-x");

        // Simulate: agentic path takes the grant.
        let used_transient = broker.take_net_grant("proj-x");
        assert!(used_transient, "grant was present before spawn");

        // Simulate: spawn_agentic_worker returns Err. Re-insert.
        if used_transient {
            broker.grant_net_once("proj-x");
        }

        // Next take must succeed (user will be re-prompted and can retry).
        assert!(
            broker.take_net_grant("proj-x"),
            "grant must be restored after re-insert so the next spawn can use it"
        );
        // And only one shot remains (idempotent).
        assert!(!broker.take_net_grant("proj-x"));
    }

    /// Persistent flag (AllowRemember) is never in the transient set.
    /// Re-insert only happens when `used_transient` is true; a persistent-flag
    /// spawn failure does NOT re-insert (nothing to restore).
    #[test]
    fn no_reinsertion_when_persistent_flag_was_used() {
        let broker = PermissionBrokerState::new();
        // No transient grant — simulates persistent-flag path.
        let used_transient = broker.take_net_grant("proj-y"); // false, nothing there
        assert!(!used_transient);

        // Spawn fails. used_transient=false → no re-insert.
        if used_transient {
            broker.grant_net_once("proj-y");
        }

        // Transient set is still empty — no spurious grant injected.
        assert!(!broker.take_net_grant("proj-y"));
    }

    // ── ConsentDecision serde round-trip ─────────────────────────────────────

    #[test]
    fn consent_decision_deserializes_all_variants() {
        let allow_remember: ConsentDecision =
            serde_json::from_str(r#""allowRemember""#).unwrap();
        assert_eq!(allow_remember, ConsentDecision::AllowRemember);

        let allow_once: ConsentDecision = serde_json::from_str(r#""allowOnce""#).unwrap();
        assert_eq!(allow_once, ConsentDecision::AllowOnce);

        let deny: ConsentDecision = serde_json::from_str(r#""deny""#).unwrap();
        assert_eq!(deny, ConsentDecision::Deny);
    }

    // ── ConsentRequest serde round-trip ──────────────────────────────────────

    #[test]
    fn consent_request_serializes_to_camel_case() {
        let req = ConsentRequest {
            kind: ConsentKind::Net,
            project_id: "proj-1".to_string(),
            agent_id: "agent-42".to_string(),
            detail: "cargo fetch failed".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"projectId\""));
        assert!(json.contains("\"agentId\""));
        assert!(json.contains("\"net\""));
    }
}
