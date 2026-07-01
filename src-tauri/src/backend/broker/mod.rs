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
#[derive(Default)]
pub enum SandboxMode {
    #[default]
    Ask,
    AutoAcceptInWorkspace,
    Unattended,
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

/// Apply the platform **capability gate** to a per-project `SandboxMode`.
///
/// Decision B (with silent fallback): `Unattended` is the autonomy-bearing mode — it runs the
/// agent unsupervised (fail-closed, no prompts). That is only safe where the OS sandbox actually
/// confines the executed commands. So `Unattended` is honoured **only when `sandbox_enforced`**;
/// otherwise it silently degrades to `Ask` (supervised) — no banner, no error (per the owner).
///
/// `sandbox_enforced` comes from [`crate::backend::sandbox::is_enforced`] — the single platform
/// truth. On macOS it is `true`, so this function is the **identity** there (zero behaviour
/// change). On a not-yet-sandboxed platform (Windows today) an `Unattended` project behaves like
/// `Ask` until the Job Object backend lands and `is_enforced()` flips to `true` — at which point
/// `Unattended` lights up with **no change to this code**.
///
/// `Ask` and `AutoAcceptInWorkspace` are never altered (they are already supervised). Pure, no I/O.
pub fn effective_sandbox_mode(mode: SandboxMode, sandbox_enforced: bool) -> SandboxMode {
    if mode == SandboxMode::Unattended && !sandbox_enforced {
        SandboxMode::Ask
    } else {
        mode
    }
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
    /// Human-readable context string displayed to the user. Never use this field
    /// programmatically as an input to backend calls — it is display-only prose.
    /// For machine-readable values use the `path` field (FolderWrite) or the
    /// structured kind fields.
    pub detail: String,
    /// Machine-readable absolute path associated with this consent request.
    ///
    /// Set for `FolderWrite` requests: contains the raw canonical folder path that
    /// triggered the block (same value passed to `grant_folder_consent` as `folder`).
    /// `None` for `Net` and other kinds that have no associated path.
    ///
    /// The frontend MUST use this field (not `detail`) as the `folder` argument to
    /// `grant_folder_consent`. `detail` is a human sentence and will be rejected by
    /// `normalize_working_set_folder` (`!is_absolute` → AllowOnce/AllowRemember fail).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Correlation id for LIVE cloud adapters (Slice 5). When set, the frontend must
    /// answer via `respond_cloud_consent` (which round-trips the decision back to the
    /// blocked cloud agent) instead of the fire-and-forget `grant_*_consent` commands.
    ///
    /// Format: `"<agent_id>:<request_id>"`. `None` for the local seatbelt path
    /// (`Net`/`FolderWrite` emitted by the mini-coder), which keeps the on-disk and wire
    /// JSON byte-identical to Slices 0-3 (NO-CHURN).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
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
            .or_default()
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

    /// Atomically consume BOTH the net grant AND all folder grants for `project_id`
    /// under a single combined critical section.
    ///
    /// # Why atomic?
    /// `take_net_grant` and `take_folder_grants` are two separate `Mutex` acquisitions.
    /// When two concurrent same-project directives reach `claim_and_launch` simultaneously,
    /// one can take the net grant while the other takes the folder grants — each directive
    /// launches with only a partial grant set ("split-grant race").  Acquiring both locks
    /// together eliminates the window: either a caller gets both grants or neither.
    ///
    /// Returns `(net_taken, folder_grants_taken)`.
    pub fn take_all_grants(&self, project_id: &str) -> (bool, HashSet<String>) {
        // Acquire in a consistent lock order (net first, then folders) to prevent
        // lock-order inversion between this method and any hypothetical future caller
        // that also acquires both — the same order must be used everywhere.
        let mut net_set = self
            .transient_net_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut folder_map = self
            .transient_folder_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let net_taken = net_set.remove(project_id);
        let folders_taken = folder_map.remove(project_id).unwrap_or_default();
        (net_taken, folders_taken)
    }

    /// Re-insert BOTH a net grant and a set of folder grants atomically under one lock
    /// acquisition.  Used on the spawn-failure path to restore grants that were consumed
    /// but never used (the worker never launched, so the user was never re-prompted).
    ///
    /// This is the mirror of `take_all_grants`: call it when `spawn_agentic_worker`
    /// returns `Err` and the mode is non-Unattended.
    pub fn reinsert_all_grants(
        &self,
        project_id: &str,
        net: bool,
        folders: &HashSet<String>,
    ) {
        // Same lock order as take_all_grants (net first, then folders).
        let mut net_set = self
            .transient_net_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut folder_map = self
            .transient_folder_grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if net {
            net_set.insert(project_id.to_string());
        }
        if !folders.is_empty() {
            let slot = folder_map
                .entry(project_id.to_string())
                .or_default();
            for f in folders {
                slot.insert(f.clone());
            }
        }
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
    // CHEAP FIX C: use BTreeSet for deterministic (sorted) iteration order.
    // HashSet iteration order is non-deterministic across runs; sorted output is easier
    // to reason about in tests, logs, and when diffing seatbelt profiles.
    use std::collections::BTreeSet;
    let mut result: BTreeSet<&str> = persisted.iter().map(String::as_str).collect();
    if mode != SandboxMode::Unattended {
        for f in &transient {
            result.insert(f.as_str());
        }
    }
    result.into_iter().map(str::to_string).collect()
}

// ──────────────────────────────────────────────
// Cloud live-waiter registry (Slice 5)
// ──────────────────────────────────────────────

/// Managed singleton tracking in-flight cloud consent requests that a LIVE cloud agent
/// (Codex over the app-server JSON-RPC stream we own) is currently blocked on.
///
/// Flow: the Codex driver thread `register`s an `approval_id`, emits the `ConsentRequest`
/// to the frontend, then blocks on the returned `Receiver` (with a timeout). When the user
/// clicks a button, `respond_cloud_consent` calls `resolve`, which delivers the decision to
/// that blocked thread, which writes the JSON-RPC approval result back to Codex.
///
/// Claude does NOT use this map: its hook is a separate OS process that cannot share
/// in-memory state, so it round-trips through the `.aspis-agents.json` file-bridge instead
/// (Slice 5b). `respond_cloud_consent` tries this in-memory map first, then the file-bridge.
///
/// Uses `std::sync::mpsc` (not tokio) to match the all-`std::thread` design of the cloud
/// duplex executor. The mutex is only ever held for the brief map mutation — never across
/// the blocking `recv`.
pub struct CloudConsentState {
    pending: std::sync::Mutex<HashMap<String, std::sync::mpsc::Sender<ConsentDecision>>>,
}

impl Default for CloudConsentState {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudConsentState {
    pub fn new() -> Self {
        Self {
            pending: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Register a waiter for `approval_id` and return the `Receiver` the caller blocks on.
    /// Replaces (and drops) any existing sender for the same id — last writer wins; the
    /// stale waiter's `recv` then returns `Err`.
    pub fn register(&self, approval_id: &str) -> std::sync::mpsc::Receiver<ConsentDecision> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(approval_id.to_string(), tx);
        rx
    }

    /// Deliver `decision` to a waiting registrant (one-shot: the entry is removed).
    /// Returns `true` only if the decision was actually DELIVERED — i.e. a waiter existed
    /// AND its `Receiver` is still alive. Returns `false` if no waiter exists OR the waiter
    /// already timed out and dropped its `Receiver` (a `SendError`). This distinction
    /// matters: `respond_cloud_consent` must NOT report success to the UI when the blocked
    /// agent has already moved on, otherwise the user's decision is silently lost and the
    /// modal is dismissed as if it took effect.
    pub fn resolve(&self, approval_id: &str, decision: ConsentDecision) -> bool {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = map.remove(approval_id) {
            tx.send(decision).is_ok()
        } else {
            false
        }
    }

    /// Drop a pending waiter without delivering a decision (session kill / EOF / timeout).
    /// Dropping the `Sender` unblocks a thread parked in `recv`/`recv_timeout` (returns
    /// `Err`). Returns whether an entry existed.
    pub fn cancel(&self, approval_id: &str) -> bool {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(approval_id).is_some()
    }
}

// ──────────────────────────────────────────────
// Codex thread/start policy mapping (Slice 5a)
// ──────────────────────────────────────────────

/// `approvalPolicy` value sent on Codex `thread/start`.
///
/// ⚠️ WIRE CASING UNVERIFIED: the public app-server example at developers.openai.com shows
/// camelCase string values (`"never"`, `"onRequest"`), while the internal `codex-rs` core
/// protocol enum uses kebab-case (`"on-request"`). We emit camelCase per the documented v2
/// example; this (and the `sandbox` shape below) MUST be validated against a live
/// `codex app-server` in e2e (the owner's eyes). `as_wire()` is the single place to flip if needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CodexApprovalPolicy {
    Never,
    UnlessTrusted,
    OnRequest,
}

impl CodexApprovalPolicy {
    pub fn as_wire(self) -> &'static str {
        match self {
            CodexApprovalPolicy::Never => "never",
            CodexApprovalPolicy::UnlessTrusted => "unlessTrusted",
            CodexApprovalPolicy::OnRequest => "onRequest",
        }
    }
}

/// Resolved Codex thread policy, built purely from the per-project sandbox knobs. No I/O —
/// unit-testable. The encoder (`encode_thread_start`) emits this as the documented v2
/// `thread/start` params shape: `approvalPolicy` (string), `sandbox: "workspaceWrite"`
/// (a plain string, NOT a tagged object), and the writable roots in a SEPARATE
/// `runtimeWorkspaceRoots` array. (The tagged `{type,writableRoots}` object is the RESPONSE
/// `SandboxPolicy` shape, not the request param — fixed after review.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexThreadPolicy {
    pub approval_policy: CodexApprovalPolicy,
    /// Goes into `runtimeWorkspaceRoots` (root first, then working_set).
    pub writable_roots: Vec<String>,
    /// Best-effort `networkAccess` hint (⚠️ #10390: silently ignored by Codex's macOS seatbelt).
    pub network_access: bool,
    /// Slice 5c: reasoning effort (Codex `model_reasoning_effort`), emitted on thread/start when
    /// set. ⚠️ exact field name unverified — confirm live.
    pub effort: Option<String>,
    /// Slice 5c: extra developer instructions, emitted on thread/start when set. ⚠️ unverified.
    pub developer_instructions: Option<String>,
}

/// Map the per-project sandbox knobs to a Codex `thread/start` policy. `working_set` and
/// `net_enabled` are ALREADY resolved by the caller; `root` is the canonical project root.
///
/// - `Ask` → `onRequest`, writable = `[root]` ONLY (every out-of-root write becomes a prompt).
/// - `AutoAcceptInWorkspace` → `onRequest`, writable = `[root] + working_set`.
/// - `Unattended` → `never` (Codex auto-decides, no prompt). Fail-closed parity DEPENDS on the
///   `sandbox`/`runtimeWorkspaceRoots` actually confining Codex — verify live.
pub fn resolve_codex_thread_policy(
    mode: SandboxMode,
    root: &str,
    working_set: &[String],
    net_enabled: bool,
) -> CodexThreadPolicy {
    let (approval_policy, writable_roots) = match mode {
        SandboxMode::Ask => (CodexApprovalPolicy::OnRequest, vec![root.to_string()]),
        SandboxMode::AutoAcceptInWorkspace => {
            let mut roots = vec![root.to_string()];
            roots.extend_from_slice(working_set);
            (CodexApprovalPolicy::OnRequest, roots)
        }
        SandboxMode::Unattended => {
            let mut roots = vec![root.to_string()];
            roots.extend_from_slice(working_set);
            (CodexApprovalPolicy::Never, roots)
        }
    };
    CodexThreadPolicy {
        approval_policy,
        writable_roots,
        network_access: net_enabled,
        // Capability/cost controls (Slice 5c) are layered on by the caller from AgentControls;
        // the sandbox-mode mapping leaves them unset.
        effort: None,
        developer_instructions: None,
    }
}

// ──────────────────────────────────────────────
// Unit tests  (TDD: written before implementation — these define the contract)
// ──────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    // ── CloudConsentState (Slice 5) ───────────────────────────────────────────

    #[test]
    fn cloud_consent_register_then_resolve_delivers() {
        let state = CloudConsentState::new();
        let rx = state.register("a");
        assert!(state.resolve("a", ConsentDecision::AllowOnce));
        assert_eq!(rx.recv().unwrap(), ConsentDecision::AllowOnce);
    }

    #[test]
    fn cloud_consent_resolve_unknown_is_false() {
        let state = CloudConsentState::new();
        assert!(!state.resolve("missing", ConsentDecision::Deny));
    }

    #[test]
    fn cloud_consent_cancel_drops_waiter() {
        let state = CloudConsentState::new();
        let rx = state.register("b");
        assert!(state.cancel("b"));
        assert!(rx.recv().is_err());
        assert!(!state.resolve("b", ConsentDecision::AllowOnce));
    }

    #[test]
    fn cloud_consent_register_twice_last_wins() {
        let state = CloudConsentState::new();
        let r1 = state.register("c");
        let r2 = state.register("c");
        assert!(state.resolve("c", ConsentDecision::AllowRemember));
        assert_eq!(r2.recv().unwrap(), ConsentDecision::AllowRemember);
        assert!(r1.recv().is_err());
    }

    #[test]
    fn cloud_consent_resolve_false_when_receiver_dropped() {
        // The agent registered then timed out and dropped its Receiver. A late decision
        // must report `false` (NOT delivered) so the UI does not dismiss the modal as if
        // the decision took effect (silent-loss guard).
        let state = CloudConsentState::new();
        let rx = state.register("d");
        drop(rx); // agent gave up
        assert!(!state.resolve("d", ConsentDecision::AllowOnce));
    }

    // ── Codex thread policy mapping (Slice 5a) ────────────────────────────────

    #[test]
    fn codex_policy_ask_is_on_request_and_root_only_writable() {
        let policy = resolve_codex_thread_policy(SandboxMode::Ask, "/root", &["/extra".into()], true);
        assert_eq!(policy.approval_policy, CodexApprovalPolicy::OnRequest);
        // Ask exposes ONLY the root as writable (out-of-root writes become prompts).
        assert_eq!(policy.writable_roots, vec!["/root".to_string()]);
        assert!(policy.network_access);
    }

    #[test]
    fn codex_policy_auto_accept_includes_working_set() {
        let policy = resolve_codex_thread_policy(
            SandboxMode::AutoAcceptInWorkspace,
            "/root",
            &["/extra".into()],
            true,
        );
        assert_eq!(policy.writable_roots, vec!["/root".to_string(), "/extra".to_string()]);
    }

    #[test]
    fn codex_policy_unattended_is_never() {
        let policy =
            resolve_codex_thread_policy(SandboxMode::Unattended, "/root", &["/extra".into()], true);
        assert_eq!(policy.approval_policy, CodexApprovalPolicy::Never);
        assert_eq!(policy.writable_roots, vec!["/root".to_string(), "/extra".to_string()]);
    }

    #[test]
    fn codex_policy_net_enabled_maps_network_access() {
        let on = resolve_codex_thread_policy(SandboxMode::Ask, "/root", &[], true);
        let off = resolve_codex_thread_policy(SandboxMode::Ask, "/root", &[], false);
        assert!(on.network_access);
        assert!(!off.network_access);
    }

    #[test]
    fn codex_approval_policy_as_wire_is_camel_case() {
        assert_eq!(CodexApprovalPolicy::OnRequest.as_wire(), "onRequest");
        assert_eq!(CodexApprovalPolicy::UnlessTrusted.as_wire(), "unlessTrusted");
        assert_eq!(CodexApprovalPolicy::Never.as_wire(), "never");
    }

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

    // ── effective_sandbox_mode (Slice 1 capability gate) ─────────────────────

    /// Unattended is honoured only where the OS sandbox is enforced.
    #[test]
    fn effective_mode_unattended_honoured_when_enforced() {
        assert_eq!(
            effective_sandbox_mode(SandboxMode::Unattended, true),
            SandboxMode::Unattended,
            "enforced platform (macOS) keeps Unattended"
        );
    }

    /// Unattended silently degrades to Ask where the OS sandbox is NOT enforced (Decision B).
    #[test]
    fn effective_mode_unattended_degrades_to_ask_when_not_enforced() {
        assert_eq!(
            effective_sandbox_mode(SandboxMode::Unattended, false),
            SandboxMode::Ask,
            "un-sandboxed platform must not run unattended autonomy"
        );
    }

    /// Ask and AutoAcceptInWorkspace are never altered, regardless of enforcement.
    #[test]
    fn effective_mode_supervised_modes_unchanged() {
        assert_eq!(effective_sandbox_mode(SandboxMode::Ask, false), SandboxMode::Ask);
        assert_eq!(effective_sandbox_mode(SandboxMode::Ask, true), SandboxMode::Ask);
        assert_eq!(
            effective_sandbox_mode(SandboxMode::AutoAcceptInWorkspace, false),
            SandboxMode::AutoAcceptInWorkspace
        );
        assert_eq!(
            effective_sandbox_mode(SandboxMode::AutoAcceptInWorkspace, true),
            SandboxMode::AutoAcceptInWorkspace
        );
    }

    /// COMPOSED contract (pins the deliberate Decision-B semantics; addresses the Slice-1 review):
    /// on an un-enforced platform an `Unattended` project degrades to a FULLY supervised `Ask`
    /// session — it is NOT made stricter than Ask. The degrade is intentional: the owner chose "silent
    /// fallback to human-gated (supervised)", so prompts return AND user approvals (transient
    /// grants) are honoured. Suppressing transient grants under the degraded mode would create an
    /// infinite re-prompt loop (approve AllowOnce → suppressed → blocked → prompt again). An
    /// un-sandboxed platform is unconfined for EVERY mode anyway; the capability gate's job is to
    /// deny *autonomy* (Unattended's gate-bypass), not to make Unattended more restrictive than Ask.
    #[test]
    fn degraded_unattended_behaves_as_full_ask_not_stricter() {
        let enforced = false; // e.g. Windows today
        let effective = effective_sandbox_mode(SandboxMode::Unattended, enforced);
        assert_eq!(effective, SandboxMode::Ask, "degrades to Ask on un-enforced platform");
        // Under the degraded mode, a user-approved one-shot (transient) grant IS honoured — same
        // as any Ask session — so the user is never stuck in a re-prompt loop.
        assert!(
            resolve_net_enabled(false, true, effective),
            "degraded Ask honours a transient net grant (supervised, not fail-closed)"
        );
        // And the degraded mode prompts (supervised), unlike raw Unattended (fail-closed silent).
        assert!(effective.prompts_for_net(), "degraded mode is supervised → prompts for net");
        assert!(effective.prompts_for_folder_write(), "degraded mode prompts for folder write");
        // Sanity: on an enforced platform (macOS) there is NO degrade — Unattended stays Unattended.
        assert_eq!(effective_sandbox_mode(SandboxMode::Unattended, true), SandboxMode::Unattended);
    }

    // ── ConsentRequest serde round-trip ──────────────────────────────────────

    #[test]
    fn consent_request_serializes_to_camel_case() {
        let req = ConsentRequest {
            kind: ConsentKind::Net,
            project_id: "proj-1".to_string(),
            agent_id: "agent-42".to_string(),
            detail: "cargo fetch failed".to_string(),
            path: None,
            approval_id: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"projectId\""));
        assert!(json.contains("\"agentId\""));
        assert!(json.contains("\"net\""));
        // Net request must NOT carry a path field (skip_serializing_if = None).
        assert!(!json.contains("\"path\""), "Net ConsentRequest must not serialize path");
        // NO-CHURN (Slice 5.0): the local seatbelt path leaves approval_id None, so the
        // wire JSON must NOT carry approvalId — byte-identical to Slices 0-3.
        assert!(
            !json.contains("\"approvalId\""),
            "local ConsentRequest must not serialize approvalId"
        );
    }

    /// Slice 5.0: a cloud-adapter ConsentRequest carries `approvalId` (the live-waiter
    /// correlation key) and it serializes as camelCase. This is what tells the frontend
    /// to answer via `respond_cloud_consent` instead of the fire-and-forget grant_* path.
    #[test]
    fn cloud_consent_request_serializes_approval_id_camel_case() {
        let req = ConsentRequest {
            kind: ConsentKind::Exec,
            project_id: "proj-9".to_string(),
            agent_id: "agent-cloud".to_string(),
            detail: "cargo build".to_string(),
            path: None,
            approval_id: Some("agent-cloud:7".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"approvalId\":\"agent-cloud:7\""));
        assert!(json.contains("\"exec\""));
        let de: ConsentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(de.approval_id.as_deref(), Some("agent-cloud:7"));
        assert_eq!(de.kind, ConsentKind::Exec);
    }

    /// BLOCKER 1 regression test: FolderWrite ConsentRequest must carry `path` (the
    /// raw canonical folder) separately from `detail` (the human-readable sentence).
    /// The frontend passes `path` to `grant_folder_consent`; `detail` is display-only.
    #[test]
    fn folder_write_consent_request_carries_path_and_human_detail() {
        let folder = "/private/tmp/my-folder".to_string();
        let req = ConsentRequest {
            kind: ConsentKind::FolderWrite,
            project_id: "proj-2".to_string(),
            agent_id: "agent-7".to_string(),
            detail: format!(
                "A sandboxed command attempted to write outside the project to \
                 \"{folder}\". Grant to allow writes there and retry."
            ),
            path: Some(folder.clone()),
            approval_id: None,
        };
        // `path` must equal the raw folder path (machine-readable, passes is_absolute check).
        assert_eq!(req.path.as_deref(), Some("/private/tmp/my-folder"));
        // `detail` is prose and is NOT absolute — the frontend must not use it as `folder`.
        let detail = req.detail.clone();
        assert!(!detail.starts_with('/'), "detail is prose, not an absolute path");
        // Serialized JSON must contain `path` for FolderWrite (it is Some).
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"path\""), "FolderWrite ConsentRequest must serialize path");
        assert!(json.contains("/private/tmp/my-folder"));
        // Round-trip: deserialize and verify path survives.
        let de: ConsentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(de.path.as_deref(), Some("/private/tmp/my-folder"));
        assert_eq!(de.kind, ConsentKind::FolderWrite);
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

    // ── CHEAP FIX A: take_all_grants / reinsert_all_grants ────────────────────

    /// take_all_grants: when both a net grant and folder grants are present, both are
    /// returned and consumed in a single call — no split-grant risk.
    #[test]
    fn take_all_grants_returns_both_and_drains_both() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-a");
        broker.grant_folder_once("proj-a", "/tmp/folder1");
        broker.grant_folder_once("proj-a", "/tmp/folder2");

        let (net, folders) = broker.take_all_grants("proj-a");
        assert!(net, "net grant must be returned");
        assert_eq!(folders.len(), 2, "both folder grants must be returned");
        assert!(folders.contains("/tmp/folder1"));
        assert!(folders.contains("/tmp/folder2"));

        // Both are consumed — a second take returns empty.
        let (net2, folders2) = broker.take_all_grants("proj-a");
        assert!(!net2, "net grant must be consumed");
        assert!(folders2.is_empty(), "folder grants must be consumed");
    }

    /// take_all_grants: when only net is present, net is returned and folders are empty.
    #[test]
    fn take_all_grants_net_only() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-b");

        let (net, folders) = broker.take_all_grants("proj-b");
        assert!(net);
        assert!(folders.is_empty());
    }

    /// take_all_grants: when only folder grants are present, net is false and folders returned.
    #[test]
    fn take_all_grants_folders_only() {
        let broker = PermissionBrokerState::new();
        broker.grant_folder_once("proj-c", "/tmp/x");

        let (net, folders) = broker.take_all_grants("proj-c");
        assert!(!net);
        assert_eq!(folders.len(), 1);
        assert!(folders.contains("/tmp/x"));
    }

    /// take_all_grants: when nothing is present, both return empty/false.
    #[test]
    fn take_all_grants_empty() {
        let broker = PermissionBrokerState::new();
        let (net, folders) = broker.take_all_grants("proj-d");
        assert!(!net);
        assert!(folders.is_empty());
    }

    /// take_all_grants is per-project: taking for proj-a does not affect proj-b.
    #[test]
    fn take_all_grants_per_project_isolation() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-a");
        broker.grant_folder_once("proj-b", "/tmp/b");

        let (net_a, folders_a) = broker.take_all_grants("proj-a");
        assert!(net_a, "proj-a net must be taken");
        assert!(folders_a.is_empty(), "proj-a has no folders");

        // proj-b's folder grant must still be present.
        let (net_b, folders_b) = broker.take_all_grants("proj-b");
        assert!(!net_b, "proj-b has no net grant");
        assert_eq!(folders_b.len(), 1, "proj-b folder must survive");
        assert!(folders_b.contains("/tmp/b"));
    }

    /// reinsert_all_grants: after a spawn failure, both grants are restored atomically
    /// so the next spawn can pick them up.
    #[test]
    fn reinsert_all_grants_restores_both_after_spawn_failure() {
        let broker = PermissionBrokerState::new();
        broker.grant_net_once("proj-x");
        broker.grant_folder_once("proj-x", "/tmp/restore");

        // Simulate: take_all_grants before spawn.
        let (net_taken, folders_taken) = broker.take_all_grants("proj-x");
        assert!(net_taken);
        assert_eq!(folders_taken.len(), 1);

        // Simulate: spawn fails → reinsert.
        broker.reinsert_all_grants("proj-x", net_taken, &folders_taken);

        // Next take must recover both.
        let (net2, folders2) = broker.take_all_grants("proj-x");
        assert!(net2, "net must be restored after reinsert");
        assert_eq!(folders2.len(), 1, "folder must be restored after reinsert");
        assert!(folders2.contains("/tmp/restore"));

        // Idempotent: one shot only.
        let (net3, folders3) = broker.take_all_grants("proj-x");
        assert!(!net3);
        assert!(folders3.is_empty());
    }

    /// reinsert_all_grants with net=false and empty folders is a no-op (no spurious grants).
    #[test]
    fn reinsert_all_grants_noop_when_nothing_to_restore() {
        let broker = PermissionBrokerState::new();
        broker.reinsert_all_grants("proj-y", false, &HashSet::new());
        let (net, folders) = broker.take_all_grants("proj-y");
        assert!(!net, "no spurious net grant inserted");
        assert!(folders.is_empty(), "no spurious folder grants inserted");
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
