//! Phase 1 — plan approval + reply-box: the data contract + PURE state machine for
//! the agent→human PLAN-approval flow and the human→agent reply-box.
//!
//! Mirrors `backend/git_push.rs` (the push-approval gate) exactly:
//!   * the serde contract (camelCase, every optional field `#[serde(default)]`, read
//!     leniently so one malformed entry never bricks the whole-state read),
//!   * the status lifecycle `pending_approval → approved | rejected | timeout` as PURE
//!     transition helpers (no double-decide).
//!
//! TWO co-owned channels live in `.aspis-agents.json`:
//!   * the top-level `planApprovalRequests` queue (Python's `plan_submit` MCP tool
//!     appends a `pending_approval` entry + lights the requesting session's
//!     `needsUser{reason:"needs_plan_approval"}` bell; the human's
//!     `approve_plan_request` / `deny_plan_request` Tauri command drives the rest and
//!     clears the bell),
//!   * the per-session `pendingQuestion` / `userReply` pair: Python's `ask_user` MCP
//!     tool writes `pendingQuestion{id,question,createdAt}` + lights
//!     `needsUser{reason:"question"}`; the human's `reply_to_agent` Tauri command
//!     writes `userReply{questionId,text,createdAt}` and clears the bell. Python's
//!     bounded poll consumes `userReply` and clears both fields.
//!
//! The PLAN ARTIFACTS themselves (`<projects_dir>/.aspis-plans/<project_id>/<plan_id>.md`
//! and the `<plan_id>.json` sidecar) are written by Python; Rust only READS the markdown
//! (path-traversal-guarded) and best-effort updates the sidecar STATUS after a decision.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::model::PlanApprovalRequest;
use super::state::BackendState;

/// Maximum bytes read from a plan markdown file. A plan is a human-readable design
/// note; anything larger is almost certainly not a real plan and would bloat the IPC
/// payload, so the read is capped (truncating, not erroring) for defense in depth.
pub const MAX_PLAN_MARKDOWN_BYTES: u64 = 1024 * 1024; // 1 MiB.

/// Max characters accepted for a `reply_to_agent` reply. A reply is a short human
/// answer to an agent's blocking question, not a document — anything larger is
/// rejected (not truncated) so the human knows their reply did not fully land.
pub const MAX_REPLY_CHARS: usize = 4096;

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Plan-request lifecycle status. snake_case over the wire to match the plan's
/// status strings exactly and the Python MCP reader/writer.
///
/// `Default` is `PendingApproval` so a request missing the key (hand-edited / older
/// writer) deserializes to the queue's entry state rather than hard-erroring. An
/// UNKNOWN status string must not brick the whole-state parse — see the model-level
/// lenient deserializer + `PlanApprovalRequest`'s lenient status handling below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanApprovalStatus {
    /// Written by the agent's `plan_submit`; awaiting the human's verdict.
    #[default]
    PendingApproval,
    /// Terminal: the human approved the plan; the agent may proceed.
    Approved,
    /// Terminal: the human rejected the plan; the agent must revise per the note.
    Rejected,
    /// Terminal: the agent's bounded poll gave up before the human acted (stamped by
    /// the Python poll). Mirrors `GitPushStatus::Timeout`.
    Timeout,
}

impl PlanApprovalStatus {
    /// `true` once the request has reached a terminal state (no further transition).
    // Not yet read in Rust (no bounded queue eviction for plans — the queue is small
    // and the human resolves every entry). Kept for state-machine symmetry with
    // `GitPushStatus::is_terminal` and a likely future cap/eviction; do not delete.
    #[allow(dead_code)]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            PlanApprovalStatus::Approved
                | PlanApprovalStatus::Rejected
                | PlanApprovalStatus::Timeout
        )
    }
}

// ---------------------------------------------------------------------------
// Pure state transitions (no double-decide)
// ---------------------------------------------------------------------------

/// APPROVE a `pending_approval` request: terminal `approved`, stamp `decidedAt` + the
/// optional `note`. Approving a non-pending request is an ERROR (the
/// no-double-decide / decide-after-terminal guard): two clicks, or an approve after
/// the request already went `rejected`/`timeout`, must NOT both resolve. Only the
/// FIRST transition out of `pending_approval` wins.
pub fn apply_approve(
    request: &PlanApprovalRequest,
    note: Option<String>,
    now: impl Into<String>,
) -> Result<PlanApprovalRequest, String> {
    apply_decision(request, PlanApprovalStatus::Approved, note, now)
}

/// REJECT a `pending_approval` request: terminal `rejected`, stamp `decidedAt` + the
/// optional revise-note. Same no-double-decide guard as `apply_approve`.
pub fn apply_reject(
    request: &PlanApprovalRequest,
    note: Option<String>,
    now: impl Into<String>,
) -> Result<PlanApprovalRequest, String> {
    apply_decision(request, PlanApprovalStatus::Rejected, note, now)
}

/// Shared transition core: only a `pending_approval` request may be decided. Stamps
/// the terminal status, `decidedAt`, and (when present and non-empty after trim) the
/// `note`. A blank/whitespace-only note is dropped (no churn — the key stays absent).
fn apply_decision(
    request: &PlanApprovalRequest,
    status: PlanApprovalStatus,
    note: Option<String>,
    now: impl Into<String>,
) -> Result<PlanApprovalRequest, String> {
    if request.status != PlanApprovalStatus::PendingApproval {
        return Err(format!(
            "cannot decide plan request {} in status {:?} (only pending_approval is decidable)",
            request.id, request.status
        ));
    }
    let mut next = request.clone();
    next.status = status;
    next.decided_at = Some(now.into());
    next.note = note.and_then(|n| {
        let trimmed = n.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    Ok(next)
}

// ---------------------------------------------------------------------------
// Listing helpers (PURE)
// ---------------------------------------------------------------------------

/// Order the plan-approval queue for the UI: PENDING entries first, then everything
/// else; within each group newest-`createdAt` first (RFC3339 lexicographic, tie-broken
/// on `id` for stability). Mirrors how the push-approval card surfaces work-to-do at
/// the top. PURE: returns a re-ordered clone, never mutates the input.
pub fn order_for_list(requests: &[PlanApprovalRequest]) -> Vec<PlanApprovalRequest> {
    let mut out = requests.to_vec();
    out.sort_by(|a, b| {
        let a_pending = a.status == PlanApprovalStatus::PendingApproval;
        let b_pending = b.status == PlanApprovalStatus::PendingApproval;
        // Pending group sorts before the rest.
        b_pending
            .cmp(&a_pending)
            // Newest first within a group.
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

// ---------------------------------------------------------------------------
// Path-traversal guard (PURE) for the plan_id used in the on-disk plan path
// ---------------------------------------------------------------------------

/// Validate that a `plan_id` is EXACTLY 32 lowercase hex characters BEFORE it is ever
/// joined into a filesystem path. This is the path-traversal guard for
/// `get_plan_markdown` / the sidecar reads: a 32-hex id cannot contain `/`, `\`, `.`,
/// `..`, a drive letter, or a NUL, so it can never escape the
/// `.aspis-plans/<project_id>/` directory. Returns the validated id (unchanged) on
/// success. Mirrors the strict-id idiom used elsewhere for push-request ids.
pub fn validate_plan_id(plan_id: &str) -> Result<String, String> {
    let id = plan_id.trim();
    if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        Ok(id.to_string())
    } else {
        Err("Plan id must be exactly 32 lowercase hexadecimal characters.".into())
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// List the plan-approval queue (pending first, newest first). Read-only snapshot —
/// the card may poll this; it never takes the write lock. Mirrors
/// `git_push_requests_list`'s read-only fast path (there is no list-time
/// reconciliation for plans, so it stays a pure snapshot read).
#[tauri::command]
pub fn plan_approval_requests_list(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<Vec<PlanApprovalRequest>, String> {
    state.ensure_unlocked()?;
    let snapshot = super::agents::read_agent_live_state_snapshot(&app)?;
    Ok(order_for_list(&snapshot.plan_approval_requests))
}

/// Read a plan's markdown from `<projects_dir>/.aspis-plans/<project_id>/<plan_id>.md`.
/// SECURITY: `plan_id` is validated as EXACTLY 32 lowercase hex AND `project_id` is
/// normalized with the same idiom every other project command uses, BOTH before any
/// path join, so neither can traverse out of the plans directory. The read is capped
/// at `MAX_PLAN_MARKDOWN_BYTES` (truncated, not erroring).
#[tauri::command]
pub fn get_plan_markdown(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    project_id: String,
    plan_id: String,
) -> Result<String, String> {
    state.ensure_unlocked()?;
    let plan_id = validate_plan_id(&plan_id)?;
    let project_id = super::projects::normalize_project_id_public(&project_id)?;
    let path = super::projects::ensure_projects_dir(&app)?
        .join(".aspis-plans")
        .join(&project_id)
        .join(format!("{plan_id}.md"));
    if !path.exists() {
        return Err("Plan not found.".into());
    }
    read_capped(&path, MAX_PLAN_MARKDOWN_BYTES)
}

/// List the sidecar JSONs for a project's plans
/// (`<projects_dir>/.aspis-plans/<project_id>/*.json`). Lenient: a malformed /
/// half-written sidecar is SKIPPED, never fails the whole list. Returns newest-first
/// (by `createdAt`). `project_id` is normalized before any path join.
#[tauri::command]
pub fn list_project_plans(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    project_id: String,
) -> Result<Vec<PlanApprovalRequest>, String> {
    state.ensure_unlocked()?;
    let project_id = super::projects::normalize_project_id_public(&project_id)?;
    let dir = super::projects::ensure_projects_dir(&app)?
        .join(".aspis-plans")
        .join(&project_id);
    let mut out = list_plan_sidecars(&dir);
    // Newest first (createdAt RFC3339 lexicographic, tie-broken on id).
    out.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// Approve a pending plan request: under the agent-state lock apply the transition,
/// clear the REQUESTING session's `needsUser` IFF its reason is "needs_plan_approval",
/// and append a coordination event. AFTER releasing the lock, best-effort update the
/// on-disk sidecar JSON status/decidedAt/note (last-write-wins; a failure is logged
/// path-only and does NOT fail the command — the in-file queue is the source of truth
/// the UI reads). Mirrors `approve_git_push_request`'s claim-under-lock shape, minus
/// the network step (the human IS the resolver; there is nothing to run).
#[tauri::command]
pub fn approve_plan_request(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    request_id: String,
    note: Option<String>,
) -> Result<PlanApprovalRequest, String> {
    decide_plan_request(&app, &state, &request_id, note, true)
}

/// Deny (reject) a pending plan request. Same flow as `approve_plan_request` but the
/// transition is `rejected`; the `note` carries the revise-instructions the agent
/// reads. NOTE: the contract names this command `deny_plan_request` (mirroring
/// `deny_git_push_request`) though the status string is `rejected`.
#[tauri::command]
pub fn deny_plan_request(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    request_id: String,
    note: Option<String>,
) -> Result<PlanApprovalRequest, String> {
    decide_plan_request(&app, &state, &request_id, note, false)
}

/// Shared approve/deny core for plan requests.
fn decide_plan_request(
    app: &tauri::AppHandle,
    state: &State<'_, BackendState>,
    request_id: &str,
    note: Option<String>,
    approve: bool,
) -> Result<PlanApprovalRequest, String> {
    state.ensure_unlocked()?;
    let request_id = request_id.trim().to_string();
    if request_id.is_empty() {
        return Err("Missing plan request id.".into());
    }
    let now = chrono::Utc::now().to_rfc3339();

    // CLAIM + decide UNDER the lock: re-reads the LIVE status so a double-decide /
    // decide-after-terminal is a clean error surfaced to the UI without acting twice.
    let resolved: Option<PlanApprovalRequest> =
        super::agents::mutate_agent_live_state(app, |live| {
            let (resolved, agent_id, transitioned) = {
                let Some(req) = live
                    .plan_approval_requests
                    .iter_mut()
                    .find(|r| r.id == request_id)
                else {
                    return None;
                };
                let agent_id = req.agent_id.clone();
                let outcome = if approve {
                    apply_approve(req, note.clone(), now.clone())
                } else {
                    apply_reject(req, note.clone(), now.clone())
                };
                match outcome {
                    Ok(next) => {
                        *req = next.clone();
                        (Some(next), agent_id, true)
                    }
                    // Not pending (double-decide / terminal): no-op, return current
                    // WITHOUT clearing the bell or appending an event.
                    Err(_) => (Some(req.clone()), agent_id, false),
                }
            };
            if transitioned {
                clear_plan_needs_user(live, &agent_id);
                let resolved_ref = resolved.as_ref();
                push_plan_decision_event(live, resolved_ref, &agent_id, approve, &now);
            }
            resolved
        })?;

    let resolved =
        resolved.ok_or_else(|| "Plan request not found (it may have been evicted).".to_string())?;

    // AFTER the lock: best-effort sidecar update (last-write-wins). Failure must NOT
    // fail the command — the in-file queue the list command reads is authoritative.
    best_effort_update_sidecar(app, &resolved);

    Ok(resolved)
}

/// Human→agent reply-box: answer an agent's blocking `ask_user` question. Caps the
/// reply at `MAX_REPLY_CHARS` (REJECTED over, not truncated), trims it, requires the
/// session's `pendingQuestion` to be present (else a clear error), writes
/// `userReply{questionId: pendingQuestion.id, text, createdAt}`, and clears the
/// `needsUser` bell IFF its reason is "question". Python's bounded poll consumes
/// `userReply` and clears both fields.
#[tauri::command]
pub fn reply_to_agent(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    agent_id: String,
    reply_text: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let agent_id = agent_id.trim().to_string();
    if agent_id.is_empty() {
        return Err("Missing agent id.".into());
    }
    // Cap on CHARACTER count (not bytes) so a multi-byte reply is judged by what the
    // human typed; reject (do not truncate) so a clipped reply never silently lands.
    if reply_text.chars().count() > MAX_REPLY_CHARS {
        return Err(format!(
            "Reply is too long (max {MAX_REPLY_CHARS} characters)."
        ));
    }
    let text = reply_text.trim().to_string();
    if text.is_empty() {
        return Err("Reply is empty.".into());
    }
    let now = chrono::Utc::now().to_rfc3339();

    super::agents::mutate_agent_live_state(&app, |live| {
        let Some(session) = live.sessions.iter_mut().find(|s| s.agent_id == agent_id) else {
            return Err("Agent not found.".to_string());
        };
        let Some(pending) = session.pending_question.as_ref() else {
            return Err("This agent is not waiting for a reply.".to_string());
        };
        session.user_reply = Some(super::model::AgentUserReply {
            question_id: pending.id.clone(),
            text: text.clone(),
            created_at: now.clone(),
        });
        // Clear the bell ONLY when it is THIS question's bell; a needsUser raised for a
        // different reason (e.g. a plan approval) must not be dropped by a reply.
        if session
            .needs_user
            .as_ref()
            .is_some_and(|n| n.reason == "question")
        {
            session.needs_user = None;
        }
        Ok(())
    })?
}

// ---------------------------------------------------------------------------
// IO helpers
// ---------------------------------------------------------------------------

/// Read at most `cap` bytes from `path` as UTF-8. Truncates silently at the cap:
/// when the cap splits a multi-byte UTF-8 sequence, the trailing partial char is
/// dropped (rather than failing a valid file as "not valid UTF-8"). A genuinely
/// non-UTF-8 file still decodes lossily (invalid bytes -> U+FFFD) but never errors.
fn read_capped(path: &std::path::Path, cap: u64) -> Result<String, String> {
    read_capped_with_cap(path, cap)
}

/// Testable inner: same as `read_capped` but the cap is an explicit parameter so a
/// small cap can be exercised against a tiny fixture in tests.
fn read_capped_with_cap(path: &std::path::Path, cap: u64) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("Could not read plan: {e}"))?;
    let mut buf = Vec::new();
    file.take(cap)
        .read_to_end(&mut buf)
        .map_err(|e| format!("Could not read plan: {e}"))?;
    Ok(decode_capped_utf8(buf))
}

/// Decode a (possibly cap-truncated) byte buffer as UTF-8 WITHOUT failing on a
/// boundary split: validate the prefix and keep only the longest valid UTF-8 prefix,
/// discarding any trailing incomplete sequence. PURE.
fn decode_capped_utf8(buf: Vec<u8>) -> String {
    match std::str::from_utf8(&buf) {
        Ok(s) => s.to_string(),
        Err(e) => {
            // `valid_up_to()` is the byte index of the longest valid UTF-8 prefix;
            // the bytes after it are either a split multibyte char (cap truncation,
            // the documented case) or genuinely invalid input. Either way, keep the
            // valid prefix and drop the rest — this truncates silently.
            let valid = e.valid_up_to();
            // SAFETY: `..valid` is guaranteed valid UTF-8 by `valid_up_to`.
            String::from_utf8_lossy(&buf[..valid]).into_owned()
        }
    }
}

/// Enumerate + leniently parse the `<plan_id>.json` sidecars in `dir`. A malformed /
/// half-written sidecar is SKIPPED; a missing directory yields an empty list. PURE
/// over the filesystem read (no agent-state lock).
fn list_plan_sidecars(dir: &std::path::Path) -> Vec<PlanApprovalRequest> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(req) = serde_json::from_str::<PlanApprovalRequest>(&content) {
            out.push(req);
        }
    }
    out
}

/// Best-effort: rewrite the plan's sidecar JSON to reflect the decided status /
/// decidedAt / note. Last-write-wins is acceptable (the in-file queue is the source of
/// truth the UI reads). A failure is logged PATH-ONLY (never the note/content) and
/// swallowed. The plan_id is re-validated before the path join (defense in depth even
/// though it came from a stored, already-validated request).
fn best_effort_update_sidecar(app: &tauri::AppHandle, request: &PlanApprovalRequest) {
    let Ok(plan_id) = validate_plan_id(&request.id) else {
        return;
    };
    let Ok(project_id) = super::projects::normalize_project_id_public(&request.project_id) else {
        return;
    };
    let Ok(base) = super::projects::ensure_projects_dir(app) else {
        return;
    };
    let path = base
        .join(".aspis-plans")
        .join(&project_id)
        .join(format!("{plan_id}.json"));
    if !path.exists() {
        return;
    }
    // Merge onto the existing sidecar so any Python-owned fields we do not model are
    // preserved; only status/decidedAt/note are overwritten.
    let merged = match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(serde_json::Value::Object(mut map)) => {
                map.insert(
                    "status".into(),
                    serde_json::to_value(request.status).unwrap_or(serde_json::Value::Null),
                );
                if let Some(decided) = &request.decided_at {
                    map.insert("decidedAt".into(), serde_json::Value::String(decided.clone()));
                }
                if let Some(note) = &request.note {
                    map.insert("note".into(), serde_json::Value::String(note.clone()));
                }
                serde_json::Value::Object(map)
            }
            // Unparseable existing sidecar: overwrite with our authoritative view.
            _ => serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
        },
        Err(_) => serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
    };
    if let Ok(serialized) = serde_json::to_string_pretty(&merged) {
        if write_sidecar_atomic(&path, &serialized).is_err() {
            eprintln!(
                "plan_approval: best-effort sidecar update failed for {}",
                path.display()
            );
        }
    }
}

/// Atomically replace the sidecar at `path` with `content`: write a sibling `.tmp`
/// then rename it onto the target via the shared `replace_file_with_backup` helper
/// (Windows-safe rename-onto-existing). Reuses the repo's atomic-write idiom from
/// `agents.rs` so a crash mid-write never leaves a truncated sidecar.
fn write_sidecar_atomic(path: &std::path::Path, content: &str) -> Result<(), String> {
    let temp_path = path.with_extension(format!(
        "json.{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::write(&temp_path, content)
        .map_err(|e| format!("Could not write plan sidecar temp: {e}"))?;
    let backup_path = path.with_extension(format!(
        "json.{}-{}.bak",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    super::fs_replace::replace_file_with_backup(&temp_path, path, &backup_path, "plan sidecar")
}

/// Clear the requesting session's `needsUser` bell ONLY when it was raised for a plan
/// approval (reason == "needs_plan_approval"). A bell lit for a different reason (a
/// question, a push) must survive the plan decision. Operates on the live state inside
/// the caller's locked mutation closure. Mirrors `clear_request_needs_user`.
fn clear_plan_needs_user(state: &mut super::model::AgentLiveState, agent_id: &str) {
    if agent_id.is_empty() {
        return;
    }
    if let Some(session) = state.sessions.iter_mut().find(|s| s.agent_id == agent_id) {
        if session
            .needs_user
            .as_ref()
            .is_some_and(|n| n.reason == "needs_plan_approval")
        {
            session.needs_user = None;
        }
    }
}

/// Unique event id for a plan decision. Mirrors the Python idiom
/// `f"E{time.time_ns()}-{uuid.uuid4().hex[:8]}"`: a nanosecond timestamp plus a short
/// random hex fragment, so two decisions inside the same millisecond never collide
/// (and therefore are not collapsed by the downstream normalize/dedup step).
fn new_plan_event_id() -> String {
    let nanos = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let mut rnd = [0u8; 4];
    // A failure to seed randomness is non-fatal: the nanosecond stamp already makes
    // same-ms collisions astronomically unlikely; fall back to a zero fragment.
    let _ = getrandom::fill(&mut rnd);
    let frag = rnd.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("E{nanos}-{frag}-plan")
}

/// Append a coordination event for a plan decision, mirroring the app-authored event
/// idiom in `record_manual_task_status` (app-user actor, merged "coder" role).
fn push_plan_decision_event(
    state: &mut super::model::AgentLiveState,
    request: Option<&PlanApprovalRequest>,
    agent_id: &str,
    approve: bool,
    now: &str,
) {
    let (event_type, verb) = if approve {
        ("plan_approved", "approved")
    } else {
        ("plan_rejected", "rejected")
    };
    let (project_id, title) = request
        .map(|r| (Some(r.project_id.clone()), r.title.clone()))
        .unwrap_or((None, String::new()));
    let detail = if title.is_empty() {
        format!("Human {verb} the plan from {agent_id}.")
    } else {
        format!("Human {verb} the plan \"{title}\" from {agent_id}.")
    };
    state.events.push(super::model::AgentEvent {
        id: new_plan_event_id(),
        timestamp: now.to_string(),
        agent_id: "app-user".into(),
        role: "coder".into(),
        event_type: event_type.into(),
        project_id,
        task_id: None,
        status: None,
        message: detail,
        evidence: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{AgentPendingQuestion, AgentUserReply, PlanApprovalRequest};

    fn req(id: &str, status: PlanApprovalStatus, created_at: &str) -> PlanApprovalRequest {
        PlanApprovalRequest {
            id: id.into(),
            agent_id: "coder-1".into(),
            project_id: "proj-1".into(),
            title: "Refactor the parser".into(),
            status,
            created_at: created_at.into(),
            decided_at: None,
            note: None,
        }
    }

    // -- serde --------------------------------------------------------------

    #[test]
    fn request_round_trips_camel_case() {
        let r = PlanApprovalRequest {
            id: "0123456789abcdef0123456789abcdef".into(),
            agent_id: "coder-1".into(),
            project_id: "proj-1".into(),
            title: "Plan X".into(),
            status: PlanApprovalStatus::PendingApproval,
            created_at: "2026-06-09T00:00:00Z".into(),
            decided_at: None,
            note: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"agentId\""), "json: {json}");
        assert!(json.contains("\"projectId\""), "json: {json}");
        assert!(json.contains("\"createdAt\""), "json: {json}");
        assert!(
            json.contains("\"status\":\"pending_approval\""),
            "json: {json}"
        );
        // No-churn: absent optionals do not serialize.
        assert!(!json.contains("decidedAt"), "json: {json}");
        assert!(!json.contains("\"note\""), "json: {json}");
        assert!(!json.contains("project_id"), "snake leaked: {json}");
        let back: PlanApprovalRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn status_snake_case_strings_match_contract() {
        for (s, tok) in [
            (PlanApprovalStatus::PendingApproval, "pending_approval"),
            (PlanApprovalStatus::Approved, "approved"),
            (PlanApprovalStatus::Rejected, "rejected"),
            (PlanApprovalStatus::Timeout, "timeout"),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{tok}\""));
            let back: PlanApprovalStatus = serde_json::from_str(&format!("\"{tok}\"")).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn partial_json_defaults_everything() {
        let json = r#"{ "id": "r1", "projectId": "p", "agentId": "a" }"#;
        let r: PlanApprovalRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.id, "r1");
        assert_eq!(r.status, PlanApprovalStatus::PendingApproval); // enum default
        assert_eq!(r.title, "");
        assert_eq!(r.decided_at, None);
        assert_eq!(r.note, None);
    }

    // -- transitions --------------------------------------------------------

    #[test]
    fn approve_from_pending_stamps_decided_and_note() {
        let r = req("r1", PlanApprovalStatus::PendingApproval, "t");
        let ok = apply_approve(&r, Some("looks good".into()), "2026-06-09T00:00:00Z").unwrap();
        assert_eq!(ok.status, PlanApprovalStatus::Approved);
        assert_eq!(ok.decided_at.as_deref(), Some("2026-06-09T00:00:00Z"));
        assert_eq!(ok.note.as_deref(), Some("looks good"));
    }

    #[test]
    fn reject_from_pending_stamps_decided_and_note() {
        let r = req("r1", PlanApprovalStatus::PendingApproval, "t");
        let ok = apply_reject(&r, Some("split step 3".into()), "2026-06-09T00:00:00Z").unwrap();
        assert_eq!(ok.status, PlanApprovalStatus::Rejected);
        assert_eq!(ok.decided_at.as_deref(), Some("2026-06-09T00:00:00Z"));
        assert_eq!(ok.note.as_deref(), Some("split step 3"));
    }

    #[test]
    fn blank_note_is_dropped() {
        let r = req("r1", PlanApprovalStatus::PendingApproval, "t");
        let ok = apply_approve(&r, Some("   ".into()), "t2").unwrap();
        assert_eq!(ok.note, None);
        let ok2 = apply_approve(&r, None, "t2").unwrap();
        assert_eq!(ok2.note, None);
    }

    #[test]
    fn double_decide_is_refused() {
        let live = req("r1", PlanApprovalStatus::PendingApproval, "t");
        let approved = apply_approve(&live, None, "t2").unwrap();
        assert_eq!(approved.status, PlanApprovalStatus::Approved);
        // Second decide against the now-approved state must fail.
        assert!(apply_approve(&approved, None, "t3").is_err());
        assert!(apply_reject(&approved, None, "t3").is_err());
    }

    #[test]
    fn timeout_entry_refuses_approve_and_reject() {
        let timed_out = req("r1", PlanApprovalStatus::Timeout, "t");
        assert!(apply_approve(&timed_out, None, "t2").is_err());
        assert!(apply_reject(&timed_out, None, "t2").is_err());
    }

    #[test]
    fn rejected_entry_refuses_redecision() {
        let rejected = req("r1", PlanApprovalStatus::Rejected, "t");
        assert!(apply_approve(&rejected, None, "t2").is_err());
        assert!(apply_reject(&rejected, None, "t2").is_err());
    }

    // -- ordering -----------------------------------------------------------

    #[test]
    fn order_puts_pending_first_then_newest() {
        let q = vec![
            req("a", PlanApprovalStatus::Approved, "2026-06-09T00:00:01Z"),
            req("b", PlanApprovalStatus::PendingApproval, "2026-06-09T00:00:02Z"),
            req("c", PlanApprovalStatus::PendingApproval, "2026-06-09T00:00:05Z"),
            req("d", PlanApprovalStatus::Rejected, "2026-06-09T00:00:09Z"),
        ];
        let ordered = order_for_list(&q);
        // Pending first, newest-pending ("c") before older-pending ("b").
        assert_eq!(ordered[0].id, "c");
        assert_eq!(ordered[1].id, "b");
        // Then the rest, newest first ("d" before "a").
        assert_eq!(ordered[2].id, "d");
        assert_eq!(ordered[3].id, "a");
    }

    // -- plan_id traversal guard (PURE) -------------------------------------

    #[test]
    fn validate_plan_id_accepts_exactly_32_lowercase_hex() {
        let ok = "0123456789abcdef0123456789abcdef";
        assert_eq!(validate_plan_id(ok).unwrap(), ok);
        // trims surrounding whitespace.
        assert!(validate_plan_id("  ").is_err());
        assert_eq!(
            validate_plan_id("  0123456789abcdef0123456789abcdef  ").unwrap(),
            ok
        );
    }

    #[test]
    fn validate_plan_id_rejects_non_hex_wrong_length_and_traversal() {
        // Wrong length (31 / 33).
        assert!(validate_plan_id("0123456789abcdef0123456789abcde").is_err());
        assert!(validate_plan_id("0123456789abcdef0123456789abcdef0").is_err());
        // Uppercase hex rejected (must be lowercase).
        assert!(validate_plan_id("0123456789ABCDEF0123456789ABCDEF").is_err());
        // Non-hex char.
        assert!(validate_plan_id("0123456789abcdef0123456789abcdeg").is_err());
        // Traversal attempts (right length-ish but not hex / contains separators).
        assert!(validate_plan_id("../../../../etc/passwd0000000000").is_err());
        assert!(validate_plan_id("..\\..\\..\\..\\system32\\cmd0000").is_err());
        assert!(validate_plan_id("0123456789abcdef0123456789ab/../").is_err());
        // Empty.
        assert!(validate_plan_id("").is_err());
    }

    // -- needsUser clearing (PURE helper) -----------------------------------

    fn session_with_needs(agent_id: &str, reason: &str) -> super::super::model::AgentSession {
        super::super::model::AgentSession {
            agent_id: agent_id.into(),
            role: "coder".into(),
            model: None,
            status: "online".into(),
            message: None,
            client: None,
            current_project_id: None,
            current_task_id: None,
            current_file_path: None,
            first_seen_at: None,
            last_seen_at: None,
            launch_token_hash: None,
            launch_token_issued_at: None,
            session_token_hash: None,
            session_token_issued_at: None,
            subagents: Vec::new(),
            needs_user: Some(super::super::model::AgentNeedsUser {
                reason: reason.into(),
                message: "m".into(),
                since: "s".into(),
            }),
            host: None,
            parent_agent_id: None,
            pending_question: None,
            user_reply: None,
        }
    }

    #[test]
    fn clear_plan_needs_user_only_clears_plan_reason() {
        let mut state = super::super::model::AgentLiveState {
            version: 2,
            updated_at: "t".into(),
            sessions: vec![session_with_needs("coder-1", "needs_plan_approval")],
            claims: Vec::new(),
            events: Vec::new(),
            rules: Vec::new(),
            state_path: String::new(),
            mcp_command: String::new(),
            mcp_client_config: String::new(),
            mini_coder_directives: Vec::new(),
            visual_check_directives: Vec::new(),
            git_push_requests: Vec::new(),
            plan_approval_requests: Vec::new(),
        };
        clear_plan_needs_user(&mut state, "coder-1");
        assert!(state.sessions[0].needs_user.is_none());

        // A question bell must NOT be cleared by the plan path.
        state.sessions[0].needs_user = Some(super::super::model::AgentNeedsUser {
            reason: "question".into(),
            message: "m".into(),
            since: "s".into(),
        });
        clear_plan_needs_user(&mut state, "coder-1");
        assert!(state.sessions[0].needs_user.is_some());
    }

    // -- sidecar listing (lenient) ------------------------------------------

    #[test]
    fn list_plan_sidecars_skips_malformed() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-plan-sidecar-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("0123456789abcdef0123456789abcdef.json"),
            r#"{"id":"0123456789abcdef0123456789abcdef","projectId":"p","agentId":"a","title":"ok","status":"pending_approval","createdAt":"2026-06-09T00:00:00Z"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        std::fs::write(dir.join("ignored.txt"), "not a sidecar").unwrap();
        let out = list_plan_sidecars(&dir);
        assert_eq!(out.len(), 1, "malformed + non-json skipped");
        assert_eq!(out[0].title, "ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_plan_sidecars_missing_dir_is_empty() {
        let dir = std::env::temp_dir().join("aspis-plan-sidecar-does-not-exist-xyz");
        assert!(list_plan_sidecars(&dir).is_empty());
    }

    // -- pendingQuestion / userReply passthrough (co-ownership) -------------

    #[test]
    fn pending_question_and_user_reply_round_trip_camel_case() {
        // Python writes pendingQuestion; Rust writes userReply. Both must round-trip
        // untouched through the session shape with camelCase keys.
        let pq = AgentPendingQuestion {
            id: "q-abc".into(),
            question: "Which schema?".into(),
            created_at: "2026-06-09T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&pq).unwrap();
        assert!(json.contains("\"createdAt\""), "json: {json}");
        assert!(!json.contains("created_at"), "snake leaked: {json}");
        let back: AgentPendingQuestion = serde_json::from_str(&json).unwrap();
        assert_eq!(pq, back);

        let ur = AgentUserReply {
            question_id: "q-abc".into(),
            text: "Use schema v2".into(),
            created_at: "2026-06-09T00:01:00Z".into(),
        };
        let json = serde_json::to_string(&ur).unwrap();
        assert!(json.contains("\"questionId\""), "json: {json}");
        assert!(json.contains("\"createdAt\""), "json: {json}");
        let back: AgentUserReply = serde_json::from_str(&json).unwrap();
        assert_eq!(ur, back);
    }

    // -- read_capped UTF-8 boundary (BLOCKER #2) ----------------------------

    #[test]
    fn read_capped_truncates_at_utf8_boundary_not_error() {
        // A file larger than the cap whose cap byte lands in the MIDDLE of a 3-byte
        // char must truncate silently (Ok), NOT error as "not valid UTF-8".
        let dir = std::env::temp_dir().join(format!(
            "aspis-plan-readcap-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plan.md");
        // "€" is U+20AC -> 3 bytes (0xE2 0x82 0xAC). Build a body where the cap (10)
        // splits the last euro sign: 8 ASCII bytes + one euro (3 bytes) = 11 bytes,
        // so a cap of 10 cuts the euro after its first 2 bytes.
        let body = format!("{}{}", "abcdefgh", "€");
        assert_eq!(body.len(), 11, "expected an 11-byte body");
        std::fs::write(&path, &body).unwrap();

        let out = read_capped_with_cap(&path, 10).expect("must truncate, not error");
        // The split euro is dropped; the 8 ASCII bytes survive.
        assert_eq!(out, "abcdefgh");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_capped_keeps_whole_multibyte_when_boundary_aligns() {
        // When the cap lands exactly on a char boundary, the whole prefix is kept.
        let body = "ab€cd"; // bytes: a b (€=3) c d => 7 bytes total
        let buf = body.as_bytes()[..5].to_vec(); // a b + full euro = 5 bytes
        assert_eq!(decode_capped_utf8(buf), "ab€");
    }

    // -- event id uniqueness within a millisecond (WARNING #6) --------------

    #[test]
    fn plan_decision_event_ids_are_unique_back_to_back() {
        // Two ids generated back-to-back (same millisecond) must differ so the
        // normalize/dedup step does not collapse two distinct decisions.
        let a = new_plan_event_id();
        let b = new_plan_event_id();
        assert_ne!(a, b, "event ids must be unique within the same ms: {a} == {b}");
        assert!(a.starts_with('E'), "id keeps the E-prefix: {a}");
    }

    // -- atomic sidecar write (WARNING #7) ----------------------------------

    #[test]
    fn write_sidecar_atomic_overwrites_existing_file() {
        // Overwriting an EXISTING sidecar via the atomic helper must succeed on
        // every platform (Windows rename-onto-existing is the tricky case) and
        // leave exactly the new content with no .tmp/.bak residue.
        let dir = std::env::temp_dir().join(format!(
            "aspis-plan-atomic-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0123456789abcdef0123456789abcdef.json");
        std::fs::write(&path, "{\"old\":true}").unwrap();

        write_sidecar_atomic(&path, "{\"new\":true}").expect("atomic overwrite must succeed");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"new\":true}");

        // No temp / backup residue next to the target.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp") || n.contains(".bak"))
            .collect();
        assert!(leftovers.is_empty(), "no tmp/bak residue: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
