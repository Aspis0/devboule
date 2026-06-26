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
use std::collections::{HashMap, HashSet};

// ──────────────────────────────────────────────
// SandboxMode — per-project autonomy knob
// ──────────────────────────────────────────────

/// Controls how the permission broker handles agent requests for this project.
///
/// Serialized as camelCase JSON strings (`"ask"`, `"autoAcceptInWorkspace"`, `"unattended"`)
/// so the frontend can read/write it directly via the Tauri command.
///
/// # Design
/// - `Ask`: always prompt the user.  The safest default.
/// - `AutoAcceptInWorkspace`: intended to auto-grant writes **inside the project root**
///   without a prompt; still prompts for network (always sensitive) and out-of-workspace
///   folders.  **IMPORTANT — implementation status**: the folder auto-grant is not yet
///   wired; it is being built in Slice 2 (folder consent).  Until Slice 2 lands,
///   `AutoAcceptInWorkspace` behaves identically to `Ask` for all non-network actions.
///   For network it also behaves like `Ask` (prompts).  No caller should assume that
///   in-workspace writes are silently auto-granted today.
/// - `Unattended`: fail-closed — NO prompts, every blocked request is silently denied.
///   This is the mode that gates Pigeon go-live (the agent must operate unsupervised).
///   The ONLY way to enable network in Unattended is a standing `net_enabled` flag
///   (AllowRemember); transient one-shot grants are not honoured.
///
/// The default is `Ask`, which is equivalent to omitting the field from disk (NO-CHURN).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxMode {
    Ask,
    AutoAcceptInWorkspace,
    Unattended,
}

impl Default for SandboxMode {
    fn default() -> Self {
        SandboxMode::Ask
    }
}

impl SandboxMode {
    /// Returns `true` when this mode should emit a net-consent prompt to the frontend.
    ///
    /// Only `Unattended` returns `false` (fail-closed, no prompt).  Both `Ask` and
    /// `AutoAcceptInWorkspace` always prompt for network because network access is
    /// considered sensitive regardless of workspace scope.
    ///
    /// Note: `AutoAcceptInWorkspace`'s in-workspace **write** auto-grant is NOT yet
    /// active — it is implemented in Slice 2 (folder consent).  This method reflects
    /// only the network-prompt behaviour, which is identical for Ask and
    /// AutoAcceptInWorkspace.
    pub fn prompts_for_net(self) -> bool {
        !matches!(self, SandboxMode::Unattended)
    }

    /// Returns `true` when this mode should emit a FolderWrite consent prompt.
    ///
    /// Semantics mirror `prompts_for_net`: only `Unattended` returns `false` (fail-closed).
    /// `AutoAcceptInWorkspace` still prompts for a NEW out-of-project folder: the
    /// "auto-accept" only covers KNOWN workspace-internal writes; a previously-unseen
    /// folder outside the root always requires explicit user consent regardless of mode.
    pub fn prompts_for_folder_write(self) -> bool {
        !matches!(self, SandboxMode::Unattended)
    }
}

/// `skip_serializing_if` predicate for `SandboxMode` fields.
///
/// Used by `ProjectMetadata.sandbox_mode` to implement NO-CHURN: when the mode equals
/// the default (`Ask`) the field is **omitted** from the serialized frontmatter so
/// pre-existing project files remain byte-stable.
pub fn is_default_sandbox_mode(mode: &SandboxMode) -> bool {
    *mode == SandboxMode::default()
}

// ──────────────────────────────────────────────
// Net-policy resolution helper
// ──────────────────────────────────────────────

/// Pure, unit-testable net-policy resolver for the agentic spawn path.
///
/// Returns `true` (network enabled) according to the following fail-closed logic:
///
/// | `mode`                  | `persistent` | `transient` | result  |
/// |-------------------------|:------------:|:-----------:|:-------:|
/// | any                     | `true`       | any         | enabled |
/// | `Ask` / `AutoAccept`    | `false`      | `true`      | enabled |
/// | `Unattended`            | `false`      | `true`      | **disabled** |
/// | any                     | `false`      | `false`     | disabled |
///
/// Key invariant: a stale one-shot (`transient`) grant granted before the project was
/// switched to `Unattended` must NOT silently enable net.  `Unattended` is fail-closed:
/// only a standing, deliberate `persistent` (AllowRemember) opt-in can open the network.
///
/// This function is deliberately free of I/O so it can be unit-tested directly.
pub fn resolve_net_enabled(persistent: bool, transient: bool, mode: SandboxMode) -> bool {
    if persistent {
        // Explicit AllowRemember: always honoured regardless of mode.
        return true;
    }
    if transient && mode != SandboxMode::Unattended {
        // One-shot grant is only honoured when the mode allows interactive consent.
        return true;
    }
    false
}

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
    /// Per-project one-shot FOLDER grants (consumed all at once on the next spawn).
    /// `project_id → HashSet<folder_path>`. A HashSet deduplicates: adding the same
    /// folder twice is idempotent (still exactly one shot after this call).
    transient_folder_grants: std::sync::Mutex<HashMap<String, HashSet<String>>>,
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
            transient_folder_grants: std::sync::Mutex::new(HashMap::new()),
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

    // ── Transient folder grants (Slice 2) ─────────────────────────────────────

    /// Record a one-shot folder grant for `project_id`.  Idempotent: adding the same
    /// folder twice still results in exactly one entry (HashSet deduplication).
    pub fn grant_folder_once(&self, project_id: &str, folder: &str) {
        let mut map = self
            .transient_folder_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(project_id.to_string())
            .or_insert_with(HashSet::new)
            .insert(folder.to_string());
    }

    /// Consume ALL one-shot folder grants for `project_id`.  Returns the set (possibly
    /// empty) and removes the entry so a subsequent call returns an empty set.  The
    /// caller should union this with the persistent `working_set`:
    /// `working_set ∪ broker.take_folder_grants(project_id)`.
    pub fn take_folder_grants(&self, project_id: &str) -> HashSet<String> {
        let mut map = self
            .transient_folder_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(project_id).unwrap_or_default()
    }
}

// ──────────────────────────────────────────────
// Slice 2: working-set resolver (pure helper)
// ──────────────────────────────────────────────

/// Pure, unit-testable working-set resolver for the agentic spawn path.
///
/// Returns the effective set of EXTRA writable folders for the next spawn, by unioning
/// the project's persisted `working_set` with the one-shot `transient` grants.
///
/// Invariant mirrors `resolve_net_enabled`:
///
/// | `mode`                  | `transient` contents | result                            |
/// |-------------------------|:--------------------:|:----------------------------------|
/// | any                     | (irrelevant)         | persisted folders always included |
/// | `Ask` / `AutoAccept`    | non-empty            | union(persisted, transient)       |
/// | `Unattended`            | non-empty            | persisted only (transient ignored)|
///
/// `Unattended` is fail-closed: a stale one-shot grant (issued before the project was
/// switched to `Unattended`) must NOT silently expand the writable surface.  Only a
/// standing `AllowRemember` opt-in (which lands in the persisted `working_set`) counts.
///
/// This function is deliberately free of I/O so it can be unit-tested directly.
pub fn resolve_working_set(
    persisted: &[String],
    transient: HashSet<String>,
    mode: SandboxMode,
) -> Vec<String> {
    let mut result: HashSet<&str> = persisted.iter().map(String::as_str).collect();
    if mode != SandboxMode::Unattended {
        for f in &transient {
            result.insert(f.as_str());
        }
    }
    result.into_iter().map(str::to_string).collect()
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

    // ── SandboxMode serde + default + prompts_for_net ────────────────────────

    #[test]
    fn sandbox_mode_default_is_ask() {
        assert_eq!(SandboxMode::default(), SandboxMode::Ask);
    }

    #[test]
    fn sandbox_mode_serde_camel_case_round_trip() {
        // Serialize each variant and verify the camelCase JSON string.
        assert_eq!(serde_json::to_string(&SandboxMode::Ask).unwrap(), r#""ask""#);
        assert_eq!(
            serde_json::to_string(&SandboxMode::AutoAcceptInWorkspace).unwrap(),
            r#""autoAcceptInWorkspace""#
        );
        assert_eq!(
            serde_json::to_string(&SandboxMode::Unattended).unwrap(),
            r#""unattended""#
        );

        // Deserialize back.
        let ask: SandboxMode = serde_json::from_str(r#""ask""#).unwrap();
        assert_eq!(ask, SandboxMode::Ask);
        let auto: SandboxMode = serde_json::from_str(r#""autoAcceptInWorkspace""#).unwrap();
        assert_eq!(auto, SandboxMode::AutoAcceptInWorkspace);
        let unattended: SandboxMode = serde_json::from_str(r#""unattended""#).unwrap();
        assert_eq!(unattended, SandboxMode::Unattended);
    }

    #[test]
    fn prompts_for_net_only_false_for_unattended() {
        assert!(
            SandboxMode::Ask.prompts_for_net(),
            "Ask must prompt for net"
        );
        assert!(
            SandboxMode::AutoAcceptInWorkspace.prompts_for_net(),
            "AutoAcceptInWorkspace must prompt for net"
        );
        assert!(
            !SandboxMode::Unattended.prompts_for_net(),
            "Unattended must NOT prompt for net (fail-closed)"
        );
    }

    // ── resolve_net_enabled (FIX B pure helper) ──────────────────────────────

    /// Unattended + transient-only grant → net DISABLED (fail-closed).
    /// A stale AllowOnce granted before the mode was changed to Unattended must not
    /// silently enable network in an unattended run.
    #[test]
    fn unattended_with_transient_only_net_is_disabled() {
        assert!(
            !resolve_net_enabled(false, true, SandboxMode::Unattended),
            "Unattended + transient-only must be net-disabled (fail-closed)"
        );
    }

    /// Unattended + persistent (AllowRemember) → net ENABLED.
    /// A deliberate standing opt-in must still work even in Unattended mode — that is
    /// the whole point of AllowRemember (users that want unattended + net explicitly grant it).
    #[test]
    fn unattended_with_persistent_net_is_enabled() {
        assert!(
            resolve_net_enabled(true, false, SandboxMode::Unattended),
            "Unattended + persistent must be net-enabled (explicit AllowRemember)"
        );
    }

    /// Unattended + both flags → net ENABLED (persistent wins).
    #[test]
    fn unattended_with_both_flags_net_is_enabled() {
        assert!(
            resolve_net_enabled(true, true, SandboxMode::Unattended),
            "Unattended + persistent + transient must be net-enabled (persistent wins)"
        );
    }

    /// Ask + transient → net ENABLED (normal interactive grant path).
    #[test]
    fn ask_with_transient_net_is_enabled() {
        assert!(
            resolve_net_enabled(false, true, SandboxMode::Ask),
            "Ask + transient must be net-enabled"
        );
    }

    /// AutoAcceptInWorkspace + transient → net ENABLED (same as Ask for network).
    #[test]
    fn auto_accept_with_transient_net_is_enabled() {
        assert!(
            resolve_net_enabled(false, true, SandboxMode::AutoAcceptInWorkspace),
            "AutoAcceptInWorkspace + transient must be net-enabled"
        );
    }

    /// No flags, any mode → net DISABLED.
    #[test]
    fn no_flags_any_mode_net_is_disabled() {
        assert!(!resolve_net_enabled(false, false, SandboxMode::Ask));
        assert!(!resolve_net_enabled(false, false, SandboxMode::AutoAcceptInWorkspace));
        assert!(!resolve_net_enabled(false, false, SandboxMode::Unattended));
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

    // ── prompts_for_folder_write ──────────────────────────────────────────────

    /// `Unattended` never prompts for a FolderWrite either — fail-closed.
    /// `Ask` and `AutoAcceptInWorkspace` always prompt for a NEW out-of-project folder.
    #[test]
    fn prompts_for_folder_write_only_false_for_unattended() {
        assert!(
            SandboxMode::Ask.prompts_for_folder_write(),
            "Ask must prompt for folder write"
        );
        assert!(
            SandboxMode::AutoAcceptInWorkspace.prompts_for_folder_write(),
            "AutoAcceptInWorkspace must still prompt for out-of-project NEW folders"
        );
        assert!(
            !SandboxMode::Unattended.prompts_for_folder_write(),
            "Unattended must NOT prompt (fail-closed)"
        );
    }

    // ── transient folder grants ───────────────────────────────────────────────

    #[test]
    fn take_folder_grants_empty_when_no_grant() {
        let broker = PermissionBrokerState::new();
        assert!(broker.take_folder_grants("proj-1").is_empty());
    }

    #[test]
    fn take_folder_grants_returns_and_consumes_all_for_project() {
        let broker = PermissionBrokerState::new();
        broker.grant_folder_once("proj-1", "/tmp/extra");
        broker.grant_folder_once("proj-1", "/tmp/other");
        let grants = broker.take_folder_grants("proj-1");
        assert_eq!(grants.len(), 2);
        assert!(grants.contains("/tmp/extra"));
        assert!(grants.contains("/tmp/other"));
        // Consumed: second take is empty.
        assert!(broker.take_folder_grants("proj-1").is_empty());
    }

    #[test]
    fn folder_grants_are_per_project() {
        let broker = PermissionBrokerState::new();
        broker.grant_folder_once("proj-a", "/tmp/a");
        // proj-b has nothing.
        assert!(broker.take_folder_grants("proj-b").is_empty());
        // proj-a still has it.
        assert!(!broker.take_folder_grants("proj-a").is_empty());
    }

    #[test]
    fn double_grant_same_folder_is_idempotent() {
        let broker = PermissionBrokerState::new();
        broker.grant_folder_once("proj-1", "/tmp/x");
        broker.grant_folder_once("proj-1", "/tmp/x"); // duplicate
        let grants = broker.take_folder_grants("proj-1");
        // A HashSet dedupes: exactly one entry.
        assert_eq!(grants.len(), 1);
    }

    #[test]
    fn folder_grant_reinsertion_on_spawn_failure() {
        let broker = PermissionBrokerState::new();
        broker.grant_folder_once("proj-x", "/tmp/folder");
        let taken = broker.take_folder_grants("proj-x");
        assert_eq!(taken.len(), 1);
        // Spawn fails → re-insert.
        for folder in &taken {
            broker.grant_folder_once("proj-x", folder);
        }
        // Next spawn can take again.
        assert_eq!(broker.take_folder_grants("proj-x").len(), 1);
        assert!(broker.take_folder_grants("proj-x").is_empty());
    }

    // ── resolve_working_set (pure helper) ────────────────────────────────────

    /// Unattended: transient grants are never honoured — only persisted folders.
    #[test]
    fn resolve_working_set_unattended_ignores_transient() {
        use std::collections::HashSet;
        let persisted = vec!["/kept".to_string()];
        let transient: HashSet<String> = ["/transient".to_string()].into_iter().collect();
        let result = resolve_working_set(&persisted, transient, SandboxMode::Unattended);
        assert!(result.iter().any(|s| s == "/kept"), "persisted must be present");
        assert!(!result.iter().any(|s| s == "/transient"), "Unattended must ignore transient");
    }

    /// Ask: both persisted and transient folders are included.
    #[test]
    fn resolve_working_set_ask_includes_both() {
        use std::collections::HashSet;
        let persisted = vec!["/a".to_string()];
        let transient: HashSet<String> = ["/b".to_string()].into_iter().collect();
        let result = resolve_working_set(&persisted, transient, SandboxMode::Ask);
        assert!(result.iter().any(|s| s == "/a"));
        assert!(result.iter().any(|s| s == "/b"));
    }

    /// AutoAcceptInWorkspace: same as Ask for working-set resolution.
    #[test]
    fn resolve_working_set_auto_accept_includes_both() {
        use std::collections::HashSet;
        let persisted = vec!["/a".to_string()];
        let transient: HashSet<String> = ["/b".to_string()].into_iter().collect();
        let result =
            resolve_working_set(&persisted, transient, SandboxMode::AutoAcceptInWorkspace);
        assert!(result.iter().any(|s| s == "/a"));
        assert!(result.iter().any(|s| s == "/b"));
    }

    /// Empty inputs → empty result regardless of mode.
    #[test]
    fn resolve_working_set_empty_inputs() {
        use std::collections::HashSet;
        let empty_t: HashSet<String> = HashSet::new();
        let result = resolve_working_set(&[], empty_t, SandboxMode::Ask);
        assert!(result.is_empty());
    }

    /// Duplicates across persisted and transient are deduplicated.
    #[test]
    fn resolve_working_set_deduplicates() {
        use std::collections::HashSet;
        let persisted = vec!["/dup".to_string()];
        let transient: HashSet<String> = ["/dup".to_string()].into_iter().collect();
        let result = resolve_working_set(&persisted, transient, SandboxMode::Ask);
        // /dup appears once.
        let count = result.iter().filter(|s| s.as_str() == "/dup").count();
        assert_eq!(count, 1);
    }

    // ── WARNING 2: Unattended must DRAIN (consume) transient grants, not skip ──

    /// Simulates the corrected Unattended path in claim_and_launch:
    /// ALWAYS drain (call take_folder_grants) even in Unattended mode — the grants are
    /// consumed and discarded (not honoured). This prevents unbounded HashMap growth and
    /// stale-grant storms when the project later switches back to Ask.
    ///
    /// The fix: unconditionally call `take_folder_grants` and then pass an EMPTY set
    /// (not the taken set) to `resolve_working_set` when Unattended.
    #[test]
    fn unattended_must_drain_folder_grants_not_skip() {
        let broker = PermissionBrokerState::new();
        broker.grant_folder_once("proj-u", "/tmp/transient-folder");

        // Simulate corrected Unattended path: always drain.
        let mode = SandboxMode::Unattended;
        let taken = broker.take_folder_grants("proj-u");    // DRAIN
        // Resolve: Unattended -> ignore transient, pass empty.
        let effective = if mode != SandboxMode::Unattended {
            resolve_working_set(&[], taken, mode)
        } else {
            // Drain happened; discard.
            resolve_working_set(&[], HashSet::new(), mode)
        };

        // Broker map must be empty after drain (no growth).
        assert!(
            broker.take_folder_grants("proj-u").is_empty(),
            "broker must be empty after drain in Unattended — no unbounded growth"
        );
        // The effective working set must NOT contain the transient grant.
        assert!(
            !effective.iter().any(|s| s == "/tmp/transient-folder"),
            "Unattended must not honour transient folder grants"
        );
    }

    /// After Unattended drains the grants, switching back to Ask on the next spawn
    /// sees an empty broker — no stale-grant storm.
    #[test]
    fn after_unattended_drain_ask_sees_empty_broker() {
        let broker = PermissionBrokerState::new();
        broker.grant_folder_once("proj-u", "/tmp/stale");

        // Unattended path: drain without honouring.
        let _taken = broker.take_folder_grants("proj-u");

        // Now switch to Ask for next spawn — broker must be clean.
        let grants = broker.take_folder_grants("proj-u");
        assert!(
            grants.is_empty(),
            "Ask spawn after Unattended must see empty broker: stale grant was drained"
        );
    }

    /// Simulates the corrected net-grant Unattended path: always drain take_net_grant
    /// even in Unattended — the grant is consumed (returns false when discarded).
    ///
    /// The OLD bug: in Unattended the code returned `false` WITHOUT calling
    /// `take_net_grant`, so the entry stayed in the HashSet forever.
    #[test]
    fn unattended_must_drain_net_grant_not_skip() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-n");

        // Corrected Unattended path: always call take_net_grant (drain).
        let mode = SandboxMode::Unattended;
        let _taken = broker.take_net_grant("proj-n"); // always drain
        // Ignore the result when Unattended (fail-closed).
        let transient_net_used = if mode == SandboxMode::Unattended { false } else { _taken };

        // Broker must be empty.
        assert!(
            !broker.take_net_grant("proj-n"),
            "net grant must be drained (empty) after Unattended path"
        );
        assert!(
            !transient_net_used,
            "Unattended must not honour the net grant"
        );
    }
}

// ── Slice 3: broker gate test — gate_unattended_fails_closed_no_prompt ───────

#[cfg(test)]
mod broker_gates {
    use super::*;
    use std::collections::HashSet;

    /// CONTRACT (design-doc gate): Unattended is fully fail-closed across the entire
    /// permission surface. Asserts the complete truth table:
    ///
    /// - `prompts_for_net() == false` and `prompts_for_folder_write() == false`
    /// - `resolve_net_enabled(persistent=false, transient=true, Unattended) == false`
    ///   (transient grant ignored)
    /// - `resolve_net_enabled(persistent=true, transient=any, Unattended) == true`
    ///   (AllowRemember is still honoured)
    /// - `resolve_working_set` ignores transient when Unattended
    /// - A pending transient net grant is DRAINED (consumed) — not honoured, not kept
    #[test]
    fn gate_unattended_fails_closed_no_prompt() {
        // ── no prompts ────────────────────────────────────────────────────────
        assert!(
            !SandboxMode::Unattended.prompts_for_net(),
            "Unattended must NOT prompt for net"
        );
        assert!(
            !SandboxMode::Unattended.prompts_for_folder_write(),
            "Unattended must NOT prompt for folder write"
        );

        // ── net resolution: transient only → disabled ─────────────────────────
        assert!(
            !resolve_net_enabled(false, true, SandboxMode::Unattended),
            "Unattended + transient-only net must be disabled (fail-closed)"
        );

        // ── net resolution: persistent wins even in Unattended ────────────────
        assert!(
            resolve_net_enabled(true, false, SandboxMode::Unattended),
            "Unattended + persistent net must be enabled (AllowRemember)"
        );
        assert!(
            resolve_net_enabled(true, true, SandboxMode::Unattended),
            "Unattended + persistent + transient net must be enabled (persistent wins)"
        );

        // ── net resolution: no flags → disabled (any mode) ───────────────────
        assert!(
            !resolve_net_enabled(false, false, SandboxMode::Unattended),
            "Unattended + no flags must be net-disabled"
        );

        // ── working_set: transient grant is NOT honoured in Unattended ────────
        let persisted = vec!["/kept".to_string()];
        let transient: HashSet<String> = ["/transient".to_string()].into_iter().collect();
        let effective = resolve_working_set(&persisted, transient, SandboxMode::Unattended);
        assert!(
            effective.iter().any(|s| s == "/kept"),
            "persisted folder must be in effective working_set"
        );
        assert!(
            !effective.iter().any(|s| s == "/transient"),
            "transient folder must NOT be in effective working_set under Unattended"
        );

        // ── transient net grant is DRAINED (consumed), not merely skipped ─────
        // This section tests broker ATOMICITY: that `take_net_grant` consumes the HashSet
        // entry and leaves the broker empty. It does NOT reach the `claim_and_launch`
        // call-site (which requires an AppHandle and a live PTY) — it verifies the pure
        // helper contract that claim_and_launch relies on. The production call-site drains
        // unconditionally in the Unattended branch; the effect here is equivalent.
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-unatt");

        // Simulate the Unattended drain: always call take_net_grant so the HashSet entry
        // is consumed and cannot persist as a stale grant across runs.
        let _taken = broker.take_net_grant("proj-unatt");
        // Broker must now be empty regardless of the taken value.
        assert!(
            !broker.take_net_grant("proj-unatt"),
            "net grant must be drained (consumed) after Unattended path — no residual grant"
        );

        // ── transient folder grant is DRAINED (consumed), not merely skipped ──
        let broker2 = PermissionBrokerState::new();
        broker2.grant_folder_once("proj-unatt", "/tmp/transient-folder");

        // Simulate the Unattended drain: always call take_folder_grants so the map
        // entry is consumed and cannot persist as a stale grant across runs.
        let _taken_folders = broker2.take_folder_grants("proj-unatt");
        // Broker map must be empty — no stale grant survives.
        assert!(
            broker2.take_folder_grants("proj-unatt").is_empty(),
            "folder grant must be drained (consumed) after Unattended path — no residual grant"
        );

        // NOTE: the production `claim_and_launch` drain decision (the branch that always
        // calls `take_net_grant` under Unattended) cannot be tested here because
        // `claim_and_launch` requires a live `AppHandle` and PTY infrastructure. The tests
        // above verify the broker-level atomicity contract that `claim_and_launch` relies on.
        // An integration test covering the full call-site would require a mock AppHandle;
        // that is out of scope for this unit-test module.
    }

    /// Confirms that `resolve_net_enabled` honours a transient grant under Ask and
    /// AutoAcceptInWorkspace, but NOT under Unattended — exercising the `mode != Unattended`
    /// branch that the drain-drain section above cannot reach (it bypasses resolve_net_enabled).
    #[test]
    fn resolve_net_enabled_honours_transient_under_non_unattended_modes() {
        // Ask: transient grant enables net.
        assert!(
            resolve_net_enabled(false, true, SandboxMode::Ask),
            "Ask mode must honour a transient net grant"
        );
        // AutoAcceptInWorkspace: transient grant enables net.
        assert!(
            resolve_net_enabled(false, true, SandboxMode::AutoAcceptInWorkspace),
            "AutoAcceptInWorkspace must honour a transient net grant"
        );
        // Unattended: transient grant is ignored (the mode != Unattended guard fires).
        assert!(
            !resolve_net_enabled(false, true, SandboxMode::Unattended),
            "Unattended must ignore a transient net grant (fail-closed)"
        );
    }
}
