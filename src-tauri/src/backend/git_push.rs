//! Git push-approval gate (GH-P4): the data contract + PURE state machine for the
//! agent→human push-approval flow.
//!
//! An agent (a coder) may COMMIT freely, but every PUSH must be approved by the
//! human. The agent's MCP `request_git_push` tool appends a `pending_approval`
//! `GitPushRequest` to the `gitPushRequests` queue in `.aspis-agents.json` and then
//! BOUNDED-polls its verdict (mirroring the mini-coder directive bridge). The human,
//! via the `PushApprovalCard` UI, calls `approve_git_push_request` /
//! `deny_git_push_request`; the APPROVE Tauri command itself runs the real push (via
//! GH-P2 `git_run_authenticated`) — there is NO background executor thread for this
//! gate, the human IS the resolver.
//!
//! This file is the HEADLESS, IO-free core, mirroring `mini_coder.rs`:
//!   * the serde contract (camelCase, every optional/list field `#[serde(default)]`,
//!     read leniently so one malformed entry never bricks the whole-state read),
//!   * the status lifecycle `pending_approval → (approved → pushing → pushed |
//!     push_failed) | denied | timeout` as pure transition helpers (no double-act),
//!   * `cap_push_requests` — the bounded queue eviction (oldest TERMINAL first;
//!     never evict a `pending_approval`/`approved`/`pushing` request).
//!
//! TIMEOUT/STALE DECISION (documented, simplest-correct): the Rust-side request
//! stays `pending_approval` until the human acts. There is NO Rust background sweep
//! to `timeout` it. The AGENT's view of timeout is owned entirely by the Python
//! bounded poll: when the poll cap expires the agent receives `timeout` and STOPS
//! (it does not retry, does not raw-push). If the human clicks Approve AFTER the
//! agent already gave up, the push is STILL honored as a real push — the human
//! explicitly asked for it. The only cost is that the agent has already moved on
//! (its poll returned `timeout`); the push still happens and the card shows the
//! result. The `Timeout` status below therefore exists so the Python poll can stamp
//! a terminal `timeout` verdict onto the request when it gives up AND the human has
//! not acted — making the request terminal so a later approve no-ops cleanly. The
//! `pending_approval` queue is bounded by `cap_push_requests` (evict oldest TERMINAL
//! first), so it can never grow without limit.

use serde::{Deserialize, Serialize};

/// Maximum number of push requests retained in the in-file queue. Beyond this,
/// `cap_push_requests` evicts the oldest TERMINAL requests (never an active one).
/// Mirrors `mini_coder::MAX_DIRECTIVES`.
pub const MAX_PUSH_REQUESTS: usize = 50;

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Push-request lifecycle status. snake_case over the wire to match the plan's
/// status strings exactly and the Python MCP reader/writer.
///
/// `Default` is `PendingApproval` so a request missing the key (hand-edited / older
/// writer) deserializes to the queue's entry state rather than hard-erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GitPushStatus {
    /// Written by the agent's `request_git_push`; awaiting the human's verdict.
    #[default]
    PendingApproval,
    /// The human approved; the push is about to run (claimed by the approve command
    /// under the lock, BEFORE the lock is released to run the network push). This is
    /// the no-double-approve anchor.
    Approved,
    /// The approve command is running the actual `git push` (lock released).
    Pushing,
    /// Terminal: the push succeeded.
    Pushed,
    /// Terminal: the push ran but git returned non-zero (or could not start).
    PushFailed,
    /// Terminal: the human denied the push; nothing was pushed.
    Denied,
    /// Terminal: the agent's bounded poll gave up before the human acted (stamped by
    /// the Python poll). See the module-level TIMEOUT/STALE decision.
    Timeout,
}

impl GitPushStatus {
    /// `true` once the request has reached a terminal state (no further transition).
    /// Only terminal requests are eligible for eviction.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            GitPushStatus::Pushed
                | GitPushStatus::PushFailed
                | GitPushStatus::Denied
                | GitPushStatus::Timeout
        )
    }
}

// ---------------------------------------------------------------------------
// Result payload
// ---------------------------------------------------------------------------

/// The app-owned terminal payload stored in `GitPushRequest.result` once the push
/// (or denial/timeout) resolves. The agent's `request_git_push` poll returns this.
///
/// camelCase + every field `#[serde(default)]`/`skip_serializing_if` so a partial
/// object still parses and a clean entry never carries an empty key (no churn).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitPushResult {
    /// Resolved terminal status (`pushed`/`push_failed`/`denied`/`timeout`).
    pub status: GitPushStatus,
    /// git's exit code for an executed push (0 on success). None for denied/timeout
    /// (no push ran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Short, already-SANITIZED git stdout (token-redacted by `git_run_authenticated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Short, already-SANITIZED git stderr / app error message (token-redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GitPushResult {
    /// Terminal `pushed` from a successful authenticated push.
    pub fn pushed(output: impl Into<String>) -> Self {
        Self {
            status: GitPushStatus::Pushed,
            exit_code: Some(0),
            output: Some(output.into()),
            error: None,
        }
    }

    /// Terminal `push_failed` (git non-zero / could not start). The `error` is the
    /// already-sanitized git stderr or an app message — NEVER carries a raw token.
    pub fn push_failed(exit_code: Option<i32>, error: impl Into<String>) -> Self {
        Self {
            status: GitPushStatus::PushFailed,
            exit_code,
            output: None,
            error: Some(error.into()),
        }
    }

    /// Terminal `denied` (human declined; nothing pushed).
    pub fn denied() -> Self {
        Self {
            status: GitPushStatus::Denied,
            exit_code: None,
            output: None,
            error: Some("Push denied by the human.".to_string()),
        }
    }

    /// Terminal `timeout` (the agent's bounded poll gave up; see module doc).
    // Dead in Rust: the Rust side never times out a push — the Python poll path owns
    // the timeout (it stamps the request + result). Kept for state-machine symmetry
    // with the other `GitPushResult` constructors and a possible future Rust-side
    // timeout sweep; do not delete.
    #[allow(dead_code)]
    pub fn timeout(error: impl Into<String>) -> Self {
        Self {
            status: GitPushStatus::Timeout,
            exit_code: None,
            output: None,
            error: Some(error.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Request (agent -> app, lifecycle owned by the human-driven Tauri commands)
// ---------------------------------------------------------------------------

/// `skip_serializing_if` predicate for a `bool` that is false-by-default, so a
/// request whose Python writer omitted the key round-trips through Rust without
/// gaining a `"...": false` we'd otherwise inject (no-churn co-ownership).
fn is_false(b: &bool) -> bool {
    !*b
}

/// One git-push approval request in the `.aspis-agents.json` `gitPushRequests`
/// queue. The agent's MCP `request_git_push` tool appends it as
/// `status:"pending_approval"`; the human's approve/deny Tauri command drives the
/// rest of the lifecycle and stamps `result`.
///
/// camelCase over the wire; every optional/list field `#[serde(default)]` so a
/// partial / hand-edited / older-writer request still deserializes (the
/// state-level `lenient_git_push_requests` then drops only an entry that fails
/// entirely, never bricking the whole state read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPushRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The requesting agent session id (the coder asking to push). Used to set/clear
    /// its `needs_user` bell and to attribute the request in the card.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    /// The project whose repo will be pushed. MUST resolve to a real project root on
    /// approve (rejected otherwise).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(default)]
    pub status: GitPushStatus,
    /// Optional branch the agent intends to push (informational for the card). The
    /// actual push targets the repo's current `HEAD`, so this is display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Optional remote name (default `origin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Whether the agent requested a FORCE push. The card warns prominently; the push
    /// still requires human approval. NO-CHURN: omitted when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub force: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    /// RFC3339 timestamp the human APPROVED the request (set by `apply_approve`).
    /// FIX F2: list-time reconciliation uses this to tell an ACTIVELY-pushing request
    /// (just approved, push still running within the bounded push window) from a truly
    /// STUCK one (approved long ago, finalize never landed), so it never clobbers an
    /// in-flight push. NO-CHURN: absent until a request is approved. Python preserves
    /// it verbatim (it never approves, only the Rust command does).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    /// The app-owned terminal verdict, set once the request reaches a terminal state.
    /// None while pending_approval/approved/pushing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<GitPushResult>,
}

// ---------------------------------------------------------------------------
// Pure state transitions (no double-act)
// ---------------------------------------------------------------------------

/// CLAIM a `pending_approval` request for approval: move it to `approved`. Claiming
/// a non-pending request is an ERROR (the no-double-approve / approve-after-terminal
/// guard): two clicks, or an approve after the request already went `denied`/
/// `timeout`, must NOT both push. This is the single anchor that makes approve
/// idempotent: only the FIRST transition out of `pending_approval` wins.
pub fn apply_approve(
    request: &GitPushRequest,
    approved_at: impl Into<String>,
) -> Result<GitPushRequest, String> {
    if request.status != GitPushStatus::PendingApproval {
        return Err(format!(
            "cannot approve push request {} in status {:?} (only pending_approval is approvable)",
            request.id, request.status
        ));
    }
    let mut next = request.clone();
    next.status = GitPushStatus::Approved;
    // FIX F2: stamp the approval time so list-time reconciliation can distinguish an
    // actively-pushing request from a stuck one.
    next.approved_at = Some(approved_at.into());
    Ok(next)
}

/// Move an `approved` request to `pushing` (the push is about to run, lock released).
/// Only an `approved` request may transition; anything else is an error.
pub fn apply_pushing(request: &GitPushRequest) -> Result<GitPushRequest, String> {
    if request.status != GitPushStatus::Approved {
        return Err(format!(
            "cannot mark push request {} pushing from status {:?} (expected approved)",
            request.id, request.status
        ));
    }
    let mut next = request.clone();
    next.status = GitPushStatus::Pushing;
    Ok(next)
}

/// Apply the terminal push `result` (pushed | push_failed) to a `pushing` request.
/// Only a `pushing` request may reach pushed/push_failed (the push actually ran);
/// an already-terminal request is not clobbered (idempotence guard).
pub fn apply_push_result(
    request: &GitPushRequest,
    result: GitPushResult,
) -> Result<GitPushRequest, String> {
    if request.status != GitPushStatus::Pushing {
        return Err(format!(
            "cannot apply push result to request {} in status {:?} (only a pushing request resolves)",
            request.id, request.status
        ));
    }
    let mut next = request.clone();
    next.status = result.status;
    next.result = Some(result);
    Ok(next)
}

/// RECONCILIATION transition (GH-P4 FIX F2 + F6): apply the REAL terminal push
/// outcome of a push that PHYSICALLY RAN, even when the request's recorded status
/// drifted away from the happy `pushing` path while the network push was in flight
/// (the agent-state lock is NOT held across the push, so the Python poll can stamp
/// the request `timeout` in that window, and a contended/failed re-lock can leave it
/// stuck at `approved`).
///
/// Accepts `approved | pushing | timeout` and overwrites them with the actual
/// `pushed | push_failed` result. WHY each source is allowed:
///   * `pushing`  — the normal path (push ran, this records the result).
///   * `approved` — the finalize re-lock that should have moved it to `pushing`
///     never landed (e.g. a previous finalize attempt failed mid-way); the push
///     STILL ran, so record its real outcome.
///   * `timeout`  — the Python poll gave up DURING the push window and speculatively
///     stamped `timeout`, but the human-approved push physically landed; the real
///     outcome MUST win over the speculative timeout (lost-audit / double-push fix).
///
/// It REFUSES an already-real terminal (`pushed`/`push_failed`/`denied`) so a late
/// duplicate finalize never clobbers a recorded real outcome or a human denial. This
/// is ONLY for the finalize-after-an-actually-executed-push path; the strict
/// `apply_push_result` (pushing-only) remains the happy-path transition and is reused
/// here after normalizing the drifted source state to `pushing`.
pub fn apply_push_result_override(
    request: &GitPushRequest,
    result: GitPushResult,
) -> Result<GitPushRequest, String> {
    // Normalize the drifted source state to the canonical `pushing` anchor so the
    // strict `apply_push_result` performs the actual write — one terminal-write path,
    // no duplicated status/result assignment:
    //   * approved -> pushing via `apply_pushing` (the finalize never reached pushing);
    //   * timeout  -> pushing directly (a speculative Python timeout we now override);
    //   * pushing  -> already canonical.
    // Anything else (pending_approval / a REAL terminal) is refused.
    let pushing = match request.status {
        GitPushStatus::Approved => apply_pushing(request)?,
        GitPushStatus::Pushing => request.clone(),
        GitPushStatus::Timeout => {
            let mut next = request.clone();
            next.status = GitPushStatus::Pushing;
            next.result = None; // drop the speculative timeout result before overwrite.
            next
        }
        _ => {
            return Err(format!(
                "cannot reconcile push result onto request {} in status {:?} \
                 (only approved/pushing/timeout reconcile after a real push)",
                request.id, request.status
            ));
        }
    };
    apply_push_result(&pushing, result)
}

/// DENY a `pending_approval` request: terminal `denied`, no push. Only a
/// `pending_approval` request may be denied (idempotent against double-deny and
/// deny-after-approve/timeout).
pub fn apply_deny(request: &GitPushRequest) -> Result<GitPushRequest, String> {
    if request.status != GitPushStatus::PendingApproval {
        return Err(format!(
            "cannot deny push request {} in status {:?} (only pending_approval is deniable)",
            request.id, request.status
        ));
    }
    let mut next = request.clone();
    next.status = GitPushStatus::Denied;
    next.result = Some(GitPushResult::denied());
    Ok(next)
}

/// TIMEOUT a `pending_approval` request (the agent's bounded poll gave up before the
/// human acted; stamped by the Python poll path). Only a `pending_approval` request
/// may be timed out — an already-approved/pushing/terminal one is left as-is so a
/// late poll-timeout never clobbers a push the human approved. See the module doc.
// Dead in Rust: the Rust side never times out a push — the Python poll path owns the
// timeout (it stamps the request + result). Kept (with its tests) for state-machine
// symmetry with the other `apply_*` transitions and a possible future Rust-side
// timeout sweep; do not delete.
#[allow(dead_code)]
pub fn apply_timeout(
    request: &GitPushRequest,
    reason: impl Into<String>,
) -> Result<GitPushRequest, String> {
    if request.status != GitPushStatus::PendingApproval {
        return Err(format!(
            "cannot time out push request {} in status {:?} (only pending_approval times out)",
            request.id, request.status
        ));
    }
    let mut next = request.clone();
    next.status = GitPushStatus::Timeout;
    next.result = Some(GitPushResult::timeout(reason));
    Ok(next)
}

// ---------------------------------------------------------------------------
// Stuck-request reconciliation (FIX F2)
// ---------------------------------------------------------------------------

/// Grace period (seconds) after approval before a still-non-terminal request is
/// considered STUCK by `reconcile_stuck_requests`. It must comfortably exceed the
/// worst-case active push window (bounded `git push` timeout 60s + the finalize lock
/// retry budget) so a list call DURING a legitimate in-flight push never reconciles
/// it out from under the approve command. Generous on purpose.
pub const STUCK_REQUEST_GRACE_SECS: i64 = 180;

/// True when a request is STUCK: it is in a transient non-terminal state
/// (`approved`/`pushing`) with NO result AND it was approved longer ago than the
/// grace window (or carries no `approved_at`, the signature of an older/pre-fix
/// stuck request — safe to reconcile). This is the fingerprint of a finalize that
/// never landed: the approve command claimed the request (and may have run the
/// push) but its step-3 bookkeeping re-lock failed, leaving it mid-flight with the
/// bell still lit. A `pending_approval` request is NOT stuck (it is legitimately
/// awaiting the human); a request WITH a result is already resolved; and a request
/// approved within the grace window is treated as a (possibly) live push.
///
/// `now`/`approved_at` are RFC3339 strings; an unparseable `approved_at` is treated
/// as stuck (we cannot prove it is recent, so favour clearing the bell).
fn is_stuck_request(request: &GitPushRequest, now: &str) -> bool {
    let transient = matches!(
        request.status,
        GitPushStatus::Approved | GitPushStatus::Pushing
    );
    if !transient || request.result.is_some() {
        return false;
    }
    match request.approved_at.as_deref() {
        None => true, // no approval stamp -> older/pre-fix stuck request.
        Some(approved_at) => {
            match (
                chrono::DateTime::parse_from_rfc3339(approved_at),
                chrono::DateTime::parse_from_rfc3339(now),
            ) {
                (Ok(approved), Ok(current)) => {
                    (current - approved).num_seconds() >= STUCK_REQUEST_GRACE_SECS
                }
                // Unparseable -> cannot prove it is recent; treat as stuck.
                _ => true,
            }
        }
    }
}

/// FIX F2: list-time reconciliation of STUCK push requests. A request left in
/// `approved`/`pushing` with no result (its finalize re-lock failed after the push
/// already physically ran — see `approve_git_push_request` step 3) would otherwise
/// keep its agent's `needs_user` bell lit forever and never reach a terminal state.
///
/// This stamps each stuck request (older than the grace window) terminal as
/// `push_failed` with an explanatory, already-safe message and returns the set of
/// requesting `agent_id`s whose bell the caller must clear (deduped, empty strings
/// dropped). We CANNOT know the real push outcome here (the approve command owned it
/// and crashed before recording it), so we conservatively record `push_failed` with a
/// message that says the result could not be confirmed — the human can re-check git /
/// re-request if needed. The actual outcome can never be silently lost as a success,
/// which is the safe direction. The grace window guarantees we never reconcile a push
/// that is still legitimately in flight.
///
/// PURE: mutates the passed slice and returns the agent ids; the IO (clearing the
/// bell in the session list, writing the state) is the caller's, under the lock.
pub fn reconcile_stuck_requests(requests: &mut [GitPushRequest], now: &str) -> Vec<String> {
    let mut agent_ids: Vec<String> = Vec::new();
    for request in requests.iter_mut() {
        if !is_stuck_request(request, now) {
            continue;
        }
        let outcome = GitPushResult::push_failed(
            None,
            "Push approval could not be finalized (the app could not record the \
             result). The push may or may not have landed — re-check the remote.",
        );
        request.status = outcome.status;
        request.result = Some(outcome);
        if !request.agent_id.is_empty() && !agent_ids.contains(&request.agent_id) {
            agent_ids.push(request.agent_id.clone());
        }
    }
    agent_ids
}

// ---------------------------------------------------------------------------
// Bounded queue eviction
// ---------------------------------------------------------------------------

/// Cap the push-request queue at `max`, evicting the OLDEST TERMINAL requests first.
/// Active requests (`pending_approval`/`approved`/`pushing`) are NEVER evicted —
/// they represent an unanswered ask or an in-flight push, and losing them would
/// orphan an agent's poll or a running push. If the queue is over `max` but every
/// excess slot is active, the queue is left larger than `max` rather than dropping
/// live work. "Oldest" is by `created_at` (RFC3339 lexicographic), tie-broken on
/// `id`. Mirrors `mini_coder::cap_directives`.
pub fn cap_push_requests(requests: &mut Vec<GitPushRequest>, max: usize) {
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

    fn request(id: &str, status: GitPushStatus, created_at: &str) -> GitPushRequest {
        GitPushRequest {
            id: id.into(),
            agent_id: "coder-1".into(),
            project_id: "proj-1".into(),
            status,
            branch: Some("main".into()),
            remote: Some("origin".into()),
            force: false,
            created_at: created_at.into(),
            approved_at: None,
            result: None,
        }
    }

    // -- serde --------------------------------------------------------------

    #[test]
    fn request_round_trip_uses_camel_case() {
        let r = GitPushRequest {
            id: "r1".into(),
            agent_id: "coder-1".into(),
            project_id: "proj-1".into(),
            status: GitPushStatus::PendingApproval,
            branch: Some("feature/x".into()),
            remote: Some("origin".into()),
            force: true,
            created_at: "2026-06-06T00:00:00Z".into(),
            approved_at: None,
            result: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"agentId\""), "json: {json}");
        assert!(json.contains("\"projectId\""), "json: {json}");
        assert!(json.contains("\"createdAt\""), "json: {json}");
        assert!(json.contains("\"force\":true"), "json: {json}");
        assert!(
            json.contains("\"status\":\"pending_approval\""),
            "json: {json}"
        );
        assert!(!json.contains("project_id"), "snake leaked: {json}");
        let back: GitPushRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn status_snake_case_strings_match_plan() {
        for (s, tok) in [
            (GitPushStatus::PendingApproval, "pending_approval"),
            (GitPushStatus::Approved, "approved"),
            (GitPushStatus::Pushing, "pushing"),
            (GitPushStatus::Pushed, "pushed"),
            (GitPushStatus::PushFailed, "push_failed"),
            (GitPushStatus::Denied, "denied"),
            (GitPushStatus::Timeout, "timeout"),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{tok}\""));
            let back: GitPushStatus = serde_json::from_str(&format!("\"{tok}\"")).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn partial_json_defaults_everything() {
        let json = r#"{ "id": "r1", "projectId": "p", "agentId": "a" }"#;
        let r: GitPushRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.id, "r1");
        assert_eq!(r.status, GitPushStatus::PendingApproval); // enum default
        assert!(!r.force);
        assert_eq!(r.branch, None);
        assert_eq!(r.remote, None);
        assert_eq!(r.result, None);
    }

    #[test]
    fn no_churn_false_force_and_empty_optionals_absent() {
        let mut r = request("r1", GitPushStatus::PendingApproval, "2026-06-06T00:00:00Z");
        r.branch = None;
        r.remote = None;
        r.force = false;
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"force\""), "json: {json}");
        assert!(!json.contains("\"branch\""), "json: {json}");
        assert!(!json.contains("\"remote\""), "json: {json}");
        assert!(!json.contains("\"result\""), "json: {json}");
    }

    #[test]
    fn result_round_trip_and_error_field() {
        let o = GitPushResult::push_failed(Some(1), "rejected non-fast-forward");
        let json = serde_json::to_string(&o).unwrap();
        assert!(json.contains("\"status\":\"push_failed\""), "json: {json}");
        assert!(json.contains("\"exitCode\":1"), "json: {json}");
        let back: GitPushResult = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);

        // A clean pushed result carries no `error` (no churn).
        let pushed = GitPushResult::pushed("Everything up-to-date");
        let pjson = serde_json::to_string(&pushed).unwrap();
        assert!(!pjson.contains("\"error\""), "json: {pjson}");
        assert!(pjson.contains("\"exitCode\":0"), "json: {pjson}");
    }

    // -- transitions --------------------------------------------------------

    #[test]
    fn lifecycle_pending_to_pushed() {
        let r = request("r1", GitPushStatus::PendingApproval, "t");
        let approved = apply_approve(&r, "2026-06-06T00:00:00Z").unwrap();
        assert_eq!(approved.status, GitPushStatus::Approved);
        assert_eq!(approved.approved_at.as_deref(), Some("2026-06-06T00:00:00Z"));
        let pushing = apply_pushing(&approved).unwrap();
        assert_eq!(pushing.status, GitPushStatus::Pushing);
        let pushed = apply_push_result(&pushing, GitPushResult::pushed("ok")).unwrap();
        assert_eq!(pushed.status, GitPushStatus::Pushed);
        assert_eq!(
            pushed.result.as_ref().unwrap().status,
            GitPushStatus::Pushed
        );
    }

    #[test]
    fn double_approve_second_is_rejected() {
        // Two clicks: A approves against live pending -> approved. B applies its
        // approve against the NOW-approved live state and must fail (no double push).
        let live = request("r1", GitPushStatus::PendingApproval, "t");
        let after_a = apply_approve(&live, "2026-06-06T00:00:00Z").unwrap();
        assert_eq!(after_a.status, GitPushStatus::Approved);
        assert!(
            apply_approve(&after_a, "2026-06-06T00:00:01Z").is_err(),
            "second approve against now-approved must be rejected"
        );
    }

    #[test]
    fn approve_after_timeout_is_rejected() {
        // The agent's poll already gave up and the request was stamped timeout. A
        // human approve clicked afterwards must NOT push (terminal -> no-op).
        let timed_out = request("r1", GitPushStatus::Timeout, "t");
        assert!(apply_approve(&timed_out, "2026-06-06T00:00:00Z").is_err());
    }

    #[test]
    fn approve_after_denied_is_rejected() {
        let denied = request("r1", GitPushStatus::Denied, "t");
        assert!(apply_approve(&denied, "2026-06-06T00:00:00Z").is_err());
    }

    #[test]
    fn deny_requires_pending() {
        assert!(apply_deny(&request("r1", GitPushStatus::Approved, "t")).is_err());
        assert!(apply_deny(&request("r1", GitPushStatus::Pushing, "t")).is_err());
        assert!(apply_deny(&request("r1", GitPushStatus::Denied, "t")).is_err());
        let ok = apply_deny(&request("r1", GitPushStatus::PendingApproval, "t")).unwrap();
        assert_eq!(ok.status, GitPushStatus::Denied);
        assert_eq!(ok.result.unwrap().status, GitPushStatus::Denied);
    }

    #[test]
    fn timeout_requires_pending_and_never_clobbers_approved() {
        // A poll-timeout that races a human approve must NOT clobber the approved/
        // pushing/terminal request — only a still-pending one times out.
        assert!(apply_timeout(&request("r1", GitPushStatus::Approved, "t"), "x").is_err());
        assert!(apply_timeout(&request("r1", GitPushStatus::Pushing, "t"), "x").is_err());
        assert!(apply_timeout(&request("r1", GitPushStatus::Pushed, "t"), "x").is_err());
        let ok = apply_timeout(&request("r1", GitPushStatus::PendingApproval, "t"), "gave up")
            .unwrap();
        assert_eq!(ok.status, GitPushStatus::Timeout);
    }

    #[test]
    fn push_result_requires_pushing() {
        assert!(
            apply_push_result(
                &request("r1", GitPushStatus::PendingApproval, "t"),
                GitPushResult::pushed("x")
            )
            .is_err()
        );
        assert!(
            apply_push_result(
                &request("r1", GitPushStatus::Approved, "t"),
                GitPushResult::pushed("x")
            )
            .is_err()
        );
        // A terminal request is not clobbered by a late push result.
        assert!(
            apply_push_result(
                &request("r1", GitPushStatus::Pushed, "t"),
                GitPushResult::push_failed(Some(1), "x")
            )
            .is_err()
        );
    }

    // -- reconciliation override (FIX F2 + F6) ------------------------------

    #[test]
    fn override_reconciles_timeout_to_real_push_outcome() {
        // FIX F6: the Python poll stamped `timeout` during the push window, but the
        // human-approved push physically landed. The real outcome must overwrite the
        // speculative timeout (not stay `timeout`).
        let timed_out = request("r1", GitPushStatus::Timeout, "t");
        let pushed =
            apply_push_result_override(&timed_out, GitPushResult::pushed("ok")).unwrap();
        assert_eq!(pushed.status, GitPushStatus::Pushed);
        assert_eq!(pushed.result.unwrap().status, GitPushStatus::Pushed);

        let timed_out2 = request("r2", GitPushStatus::Timeout, "t");
        let failed = apply_push_result_override(
            &timed_out2,
            GitPushResult::push_failed(Some(1), "rejected"),
        )
        .unwrap();
        assert_eq!(failed.status, GitPushStatus::PushFailed);
    }

    #[test]
    fn override_reconciles_stuck_approved_to_real_push_outcome() {
        // FIX F2: a finalize re-lock that never reached `pushing` leaves the request
        // at `approved`, yet the push ran. Reconcile to the real outcome.
        let approved = request("r1", GitPushStatus::Approved, "t");
        let pushed =
            apply_push_result_override(&approved, GitPushResult::pushed("ok")).unwrap();
        assert_eq!(pushed.status, GitPushStatus::Pushed);
    }

    #[test]
    fn override_accepts_normal_pushing_path() {
        let pushing = request("r1", GitPushStatus::Pushing, "t");
        let pushed =
            apply_push_result_override(&pushing, GitPushResult::pushed("ok")).unwrap();
        assert_eq!(pushed.status, GitPushStatus::Pushed);
    }

    #[test]
    fn override_refuses_real_terminal_and_denied() {
        // A late duplicate finalize must NOT clobber a recorded real outcome or a
        // human denial.
        assert!(
            apply_push_result_override(
                &request("r1", GitPushStatus::Pushed, "t"),
                GitPushResult::push_failed(Some(1), "x")
            )
            .is_err()
        );
        assert!(
            apply_push_result_override(
                &request("r1", GitPushStatus::PushFailed, "t"),
                GitPushResult::pushed("x")
            )
            .is_err()
        );
        assert!(
            apply_push_result_override(
                &request("r1", GitPushStatus::Denied, "t"),
                GitPushResult::pushed("x")
            )
            .is_err()
        );
        // pending_approval is also refused: nothing has run yet.
        assert!(
            apply_push_result_override(
                &request("r1", GitPushStatus::PendingApproval, "t"),
                GitPushResult::pushed("x")
            )
            .is_err()
        );
    }

    // -- stuck-request reconciliation (FIX F2) ------------------------------

    #[test]
    fn reconcile_stamps_stuck_approved_and_reports_agent_to_clear() {
        // A request stuck `approved` with NO result and no approval stamp (pre-fix /
        // crashed finalize) is reconciled to push_failed and its agent reported so the
        // caller clears the bell.
        let mut q = vec![request("r1", GitPushStatus::Approved, "t")];
        let cleared = reconcile_stuck_requests(&mut q, "2026-06-06T00:05:00Z");
        assert_eq!(q[0].status, GitPushStatus::PushFailed);
        assert!(q[0].result.is_some());
        assert_eq!(cleared, vec!["coder-1".to_string()]);
    }

    #[test]
    fn reconcile_stamps_stuck_pushing() {
        let mut q = vec![request("r1", GitPushStatus::Pushing, "t")];
        let cleared = reconcile_stuck_requests(&mut q, "2026-06-06T00:05:00Z");
        assert_eq!(q[0].status, GitPushStatus::PushFailed);
        assert_eq!(cleared, vec!["coder-1".to_string()]);
    }

    #[test]
    fn reconcile_leaves_recently_approved_in_flight_push_untouched() {
        // A request approved JUST NOW (within the grace window) is treated as a live
        // push and must NOT be reconciled out from under the approve command.
        let mut r = request("r1", GitPushStatus::Approved, "t");
        r.approved_at = Some("2026-06-06T00:00:00Z".into());
        let mut q = vec![r];
        // 10s later — well inside STUCK_REQUEST_GRACE_SECS.
        let cleared = reconcile_stuck_requests(&mut q, "2026-06-06T00:00:10Z");
        assert_eq!(q[0].status, GitPushStatus::Approved);
        assert!(q[0].result.is_none());
        assert!(cleared.is_empty());
    }

    #[test]
    fn reconcile_stamps_old_approved_past_grace() {
        let mut r = request("r1", GitPushStatus::Approved, "t");
        r.approved_at = Some("2026-06-06T00:00:00Z".into());
        let mut q = vec![r];
        // Well past the grace window.
        let cleared = reconcile_stuck_requests(&mut q, "2026-06-06T01:00:00Z");
        assert_eq!(q[0].status, GitPushStatus::PushFailed);
        assert_eq!(cleared, vec!["coder-1".to_string()]);
    }

    #[test]
    fn reconcile_ignores_pending_and_terminal_and_resulted() {
        let mut pending = request("p", GitPushStatus::PendingApproval, "t");
        pending.approved_at = None;
        let mut resulted = request("a", GitPushStatus::Approved, "t"); // approved WITH a result
        resulted.result = Some(GitPushResult::pushed("ok"));
        let mut q = vec![
            pending,
            resulted,
            request("done", GitPushStatus::Pushed, "t"),
            request("den", GitPushStatus::Denied, "t"),
            request("to", GitPushStatus::Timeout, "t"),
        ];
        let cleared = reconcile_stuck_requests(&mut q, "2026-06-06T05:00:00Z");
        assert!(cleared.is_empty());
        assert_eq!(q[0].status, GitPushStatus::PendingApproval);
        assert_eq!(q[1].status, GitPushStatus::Approved); // had a result -> untouched
    }

    // -- bounded queue ------------------------------------------------------

    #[test]
    fn cap_evicts_oldest_terminal_only() {
        let mut q = vec![
            request("a", GitPushStatus::Pushed, "2026-06-06T00:00:01Z"),
            request("b", GitPushStatus::PendingApproval, "2026-06-06T00:00:02Z"),
            request("c", GitPushStatus::Denied, "2026-06-06T00:00:03Z"),
        ];
        cap_push_requests(&mut q, 2);
        // Oldest terminal ("a") evicted; the pending ("b") is never dropped.
        assert_eq!(q.len(), 2);
        assert!(q.iter().any(|r| r.id == "b"));
        assert!(q.iter().any(|r| r.id == "c"));
        assert!(!q.iter().any(|r| r.id == "a"));
    }

    #[test]
    fn cap_never_evicts_active_even_over_max() {
        let mut q = vec![
            request("a", GitPushStatus::PendingApproval, "2026-06-06T00:00:01Z"),
            request("b", GitPushStatus::Pushing, "2026-06-06T00:00:02Z"),
            request("c", GitPushStatus::Approved, "2026-06-06T00:00:03Z"),
        ];
        cap_push_requests(&mut q, 1);
        // All active -> nothing evicted, queue left larger than max.
        assert_eq!(q.len(), 3);
    }
}
