//! Human-gate tools (P3): plan_submit / plan_status / request_git_push / ask_user
//! + project_create_plan_tasks (depends on approved plan_id).
//!
//! # Security model (parity with `oracle/server/aspis_mcp.py`)
//!
//! * **Queues only.** MCP agents append `pending_approval` entries and poll. They
//!   NEVER write terminal verdicts (`approved` / `rejected` / `pushed` / …) except
//!   the agent-side **timeout** stamp, and only when the request is still
//!   `pending_approval` (a human decision that races in always wins).
//! * **Double-approve prevention** is owned by the Tauri approve commands
//!   (`apply_approve` / plan decision only from `pending_approval`). This MCP
//!   surface has no approve tool — agents cannot self-approve or spoof a
//!   terminal status into the queue.
//! * **Unlock is not required on the MCP path.** The agent is a separate principal
//!   from the human UI; the human still must act in the app to approve. Soft lock /
//!   unlock gates apply to Tauri steers only.
//! * **Role allowlists** from `role_rules.json` (via `require_agent_tool`).
//! * **Session token** required when the session stores a hash (managed register).
//! * **JSON field names** camelCase to match Tauri UI (`planApprovalRequests`,
//!   `gitPushRequests`, `needsUser`, `pendingQuestion`, `userReply`).

use crate::project_file::{
    ensure_inside_projects, load_project_locked, next_task_id, normalize_project_id,
    normalize_task_id, note_id, project_lock_path, project_path, public_project,
    read_project_file, validate_project_state, validate_task_dependency_dag,
    write_project_file,
};
use crate::state::{
    add_event, clean_text, find_session_mut, now_rfc3339, read_agents_state, upsert_session,
    with_agents_lock, with_file_lock, write_agents_state, write_text_crash_safe, ToolError,
    ToolResult,
};
use crate::tools::agent_lifecycle::require_agent_tool;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

// ── constants (co-owned with Python aspis_mcp + Tauri) ──────────────────────

const MAX_GIT_PUSH_REQUESTS: usize = 50;
const MAX_PLAN_APPROVAL_REQUESTS: usize = 20;
const PLAN_MAX_MARKDOWN_CHARS: usize = 200_000;
const MAX_PLAN_TASKS: usize = 40;
const MAX_PLAN_TASK_SCOPE: usize = 3;

const DEFAULT_POLL_TIMEOUT_SECS: f64 = 600.0;
const DEFAULT_POLL_INTERVAL_SECS: f64 = 0.75;

const GIT_PUSH_TERMINAL: &[&str] = &["pushed", "push_failed", "denied", "timeout"];
const PLAN_TERMINAL: &[&str] = &["approved", "rejected", "timeout"];
const PLAN_VERDICT: &[&str] = &["approved", "rejected"];

// ── poll timing (env-overridable for tests; prefer DEVBOULE_, fall back ASPIS_) ─

fn env_f64(keys: &[&str], default: f64) -> f64 {
    for key in keys {
        if let Ok(raw) = std::env::var(key) {
            if let Ok(n) = raw.parse::<f64>() {
                return n.max(0.0);
            }
        }
    }
    default
}

fn poll_timeout_secs() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS",
            "ASPIS_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS",
        ],
        DEFAULT_POLL_TIMEOUT_SECS,
    )
}

fn poll_interval_secs() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_HUMAN_GATE_POLL_INTERVAL_SECS",
            "ASPIS_MCP_HUMAN_GATE_POLL_INTERVAL_SECS",
        ],
        DEFAULT_POLL_INTERVAL_SECS,
    )
}

// ── small helpers ───────────────────────────────────────────────────────────

/// Strip invisible / bidi control characters (Python `_INVISIBLE_AND_BIDI_RE`).
fn strip_invisible_and_bidi(text: &str) -> String {
    text.chars()
        .filter(|c| {
            let u = *c as u32;
            // C0 controls except tab/LF/CR handled separately for markdown; strip
            // zero-width, bidi, and other format chars commonly used to obfuscate.
            !matches!(
                u,
                0x00..=0x08
                    | 0x0B..=0x0C
                    | 0x0E..=0x1F
                    | 0x7F
                    | 0x200B..=0x200F
                    | 0x202A..=0x202E
                    | 0x2060..=0x2064
                    | 0x2066..=0x206F
                    | 0xFEFF
            )
        })
        .collect()
}

fn new_hex_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn is_plan_id(plan_id: &str) -> bool {
    plan_id.len() == 32 && plan_id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn validate_plan_id(raw: &str) -> ToolResult<String> {
    let plan_id = raw.trim().to_ascii_lowercase();
    if !is_plan_id(&plan_id) {
        return Err(ToolError::new(
            "plan_id must be exactly 32 lowercase hexadecimal characters.",
        ));
    }
    Ok(plan_id)
}

/// Redact classic/fine-grained GitHub tokens (parity with Python `_GITHUB_TOKEN_RE`).
fn redact_github_tokens(input: &str) -> String {
    let prefixes = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let rest = &input[i..];
        let mut matched = None;
        for p in prefixes {
            if rest.starts_with(p) {
                matched = Some(p);
                break;
            }
        }
        if let Some(p) = matched {
            let after = &rest[p.len()..];
            let body_len = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .map(|c| c.len_utf8())
                .sum::<usize>();
            if body_len > 0 {
                out.push_str("[redacted-github-token]");
                i += p.len() + body_len;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// FIX F9: same remote allowlist as Tauri `validate_push_remote`.
fn validate_push_remote(value: Option<&str>) -> ToolResult<Option<String>> {
    let raw = value.unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > 100 {
        return Err(ToolError::new("Remote name is too long."));
    }
    let first = raw.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err(ToolError::new(
            "Remote name must start with a letter or digit.",
        ));
    }
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
    {
        return Err(ToolError::new(
            "Remote name may only contain letters, digits, . _ - /",
        ));
    }
    Ok(Some(raw.to_string()))
}

fn validate_plan_scope_path(rel: &str) -> ToolResult<String> {
    let text = rel.trim();
    if text.is_empty() {
        return Err(ToolError::new("Plan task scope path is required."));
    }
    if text.len() > 1024 {
        return Err(ToolError::new(format!(
            "Plan scope path too long (max 1024 chars): got {}",
            text.len()
        )));
    }
    if text.starts_with('/') || text.starts_with('\\') {
        return Err(ToolError::new(format!(
            "Plan scope path must be relative, got absolute: {rel}"
        )));
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return Err(ToolError::new(format!(
            "Plan scope path must be relative, got absolute: {rel}"
        )));
    }
    for component in text.split(|c| c == '/' || c == '\\') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(ToolError::new(format!(
                "Plan scope path must not contain '..': {rel}"
            )));
        }
        if component.starts_with('-') {
            return Err(ToolError::new(format!(
                "Plan scope path component must not start with '-': {rel}"
            )));
        }
    }
    Ok(text.to_string())
}

fn plans_dir(projects_dir: &Path) -> PathBuf {
    projects_dir.join(".aspis-plans")
}

/// Resolve plan artifact base under `.aspis-plans/<project_id>/`.
///
/// Unlike `ensure_inside_projects` on a missing nested path (which needs the
/// parent to already exist for canonicalize), this creates the plans tree under
/// the projects root and re-checks confinement after creation.
fn plan_project_base(projects_dir: &Path, project_id: &str) -> ToolResult<PathBuf> {
    // project_id is already normalize_project_id'd (lowercase allowlist) so it
    // cannot escape via `..` / absolute segments.
    let base = plans_dir(projects_dir).join(project_id);
    std::fs::create_dir_all(&base).map_err(|e| {
        ToolError::new(format!("Could not create plan artifact directory: {e}"))
    })?;
    ensure_inside_projects(projects_dir, &base)
}

fn plan_artifact_paths(
    projects_dir: &Path,
    project_id: &str,
    plan_id: &str,
) -> ToolResult<(PathBuf, PathBuf)> {
    let base = plan_project_base(projects_dir, project_id)?;
    Ok((
        base.join(format!("{plan_id}.md")),
        base.join(format!("{plan_id}.json")),
    ))
}

fn needs_user_reason(session: &Map<String, Value>) -> String {
    session
        .get("needsUser")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn set_needs_user(
    session: &mut Map<String, Value>,
    reason: &str,
    message: &str,
    preserve_same_reason_since: bool,
) -> ToolResult<()> {
    let existing_reason = needs_user_reason(session);
    if !existing_reason.is_empty() && existing_reason != reason {
        return Err(ToolError::new(format!(
            "This session already has an outstanding needsUser (reason: {existing_reason}); resolve it before continuing."
        )));
    }
    let since = if preserve_same_reason_since && existing_reason == reason {
        session
            .get("needsUser")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("since"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(now_rfc3339)
    } else {
        now_rfc3339()
    };
    let msg = clean_text(message, "Message", 1000)?;
    session.insert(
        "needsUser".into(),
        json!({
            "reason": reason,
            "message": msg,
            "since": since,
        }),
    );
    Ok(())
}

fn clear_needs_user_if_reason(session: &mut Map<String, Value>, reason: &str) {
    let current = needs_user_reason(session);
    if current == reason || (reason.is_empty() && !current.is_empty()) {
        session.insert("needsUser".into(), Value::Null);
    }
}

/// True when this agent still has a non-terminal plan/git request other than `exclude_id`.
/// Used so a timeout on one gate does not clear the needsUser bell for another.
fn agent_has_outstanding_gate_request(
    state: &Value,
    agent_id: &str,
    exclude_id: &str,
) -> bool {
    for key in ["planApprovalRequests", "gitPushRequests"] {
        let Some(arr) = state.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for request in arr {
            if request.get("agentId").and_then(|v| v.as_str()) != Some(agent_id) {
                continue;
            }
            let id = request.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if id == exclude_id {
                continue;
            }
            let status = request
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            // Outstanding human-gate work: still waiting, or git mid-flight.
            if status == "pending_approval"
                || status == "approved"
                || status == "pushing"
            {
                return true;
            }
        }
    }
    false
}

/// Clear needsUser for `reason` only when no other outstanding plan/git gates remain.
fn clear_gate_needs_user_if_idle(
    state: &mut Value,
    agent_id: &str,
    reason: &str,
    exclude_id: &str,
) {
    if agent_has_outstanding_gate_request(state, agent_id, exclude_id) {
        return;
    }
    if let Some(session) = find_session_mut(state, agent_id) {
        clear_needs_user_if_reason(session, reason);
    }
}

fn count_non_terminal(requests: &[Value], terminal: &[&str]) -> usize {
    requests
        .iter()
        .filter(|r| {
            let status = r
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            !terminal.contains(&status.as_str())
        })
        .count()
}

/// Refuse new queue entries when non-terminal count is already at the hard cap
/// (cap_by_terminal only evicts terminal rows — pure-pending growth must be refused).
fn refuse_if_pending_at_cap(
    requests: &[Value],
    max: usize,
    terminal: &[&str],
    kind: &str,
) -> ToolResult<()> {
    if count_non_terminal(requests, terminal) >= max {
        return Err(ToolError::new(format!(
            "Too many pending {kind} requests (cap {max}); wait for human decisions before submitting more."
        )));
    }
    Ok(())
}

/// Bounded sleep between human-gate polls.
///
/// Handlers are synchronous (no tokio runtime in this path), so `thread::sleep`
/// is correct. If these tools move behind async MCP handlers, switch to
/// `tokio::time::sleep` / `spawn_blocking` so the async runtime is not blocked.
fn poll_sleep(interval: Duration) {
    if !interval.is_zero() {
        thread::sleep(interval);
    }
}

// ── queue caps ──────────────────────────────────────────────────────────────

fn cap_by_terminal(
    requests: Vec<Value>,
    max: usize,
    terminal: &[&str],
) -> Vec<Value> {
    let clean: Vec<Value> = requests
        .into_iter()
        .filter(|r| r.is_object())
        .collect();
    if clean.len() <= max {
        return clean;
    }
    let drop_count = clean.len() - max;
    let mut terminal_idx: Vec<(usize, String, String)> = Vec::new();
    for (i, r) in clean.iter().enumerate() {
        let status = r
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if terminal.contains(&status.as_str()) {
            let created = r
                .get("createdAt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            terminal_idx.push((i, created, id));
        }
    }
    if drop_count == 0 || terminal_idx.is_empty() {
        return clean;
    }
    terminal_idx.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
    let to_drop: std::collections::HashSet<usize> = terminal_idx
        .into_iter()
        .take(drop_count)
        .map(|(i, _, _)| i)
        .collect();
    clean
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !to_drop.contains(i))
        .map(|(_, r)| r)
        .collect()
}

fn cap_git_push_requests(requests: Vec<Value>) -> Vec<Value> {
    cap_by_terminal(requests, MAX_GIT_PUSH_REQUESTS, GIT_PUSH_TERMINAL)
}

fn cap_plan_approval_requests(requests: Vec<Value>) -> Vec<Value> {
    // Like cap_by_terminal, but never evict `approved` rows that still lack
    // tasksCreated — those still need materialize and must survive queue rewrites.
    let clean: Vec<Value> = requests
        .into_iter()
        .filter(|r| r.is_object())
        .collect();
    if clean.len() <= MAX_PLAN_APPROVAL_REQUESTS {
        return clean;
    }
    let drop_count = clean.len() - MAX_PLAN_APPROVAL_REQUESTS;
    let mut terminal_idx: Vec<(usize, String, String)> = Vec::new();
    for (i, r) in clean.iter().enumerate() {
        let status = r
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !PLAN_TERMINAL.contains(&status.as_str()) {
            continue;
        }
        // Protect approved plans that have not yet materialized tasks.
        if status == "approved" {
            let created_flag = r
                .get("tasksCreated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let stamped = r
                .get("tasksMaterializedAt")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if !created_flag && !stamped {
                continue;
            }
        }
        let created = r
            .get("createdAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        terminal_idx.push((i, created, id));
    }
    if drop_count == 0 || terminal_idx.is_empty() {
        return clean;
    }
    terminal_idx.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
    let to_drop: std::collections::HashSet<usize> = terminal_idx
        .into_iter()
        .take(drop_count)
        .map(|(i, _, _)| i)
        .collect();
    clean
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !to_drop.contains(i))
        .map(|(_, r)| r)
        .collect()
}

fn ensure_queue_array<'a>(
    state: &'a mut Value,
    key: &str,
) -> ToolResult<&'a mut Vec<Value>> {
    let obj = state
        .as_object_mut()
        .ok_or_else(|| ToolError::new("Agents state is invalid."))?;
    if !obj.contains_key(key) {
        obj.insert(key.to_string(), json!([]));
    }
    obj.get_mut(key)
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| ToolError::new(format!("Agents state {key} must be a list.")))
}

// ── scrub + poll helpers ────────────────────────────────────────────────────

fn scrub_push_result(result: &Value) -> Value {
    let Some(obj) = result.as_object() else {
        return result.clone();
    };
    let mut scrubbed = obj.clone();
    for key in ["output", "error"] {
        if let Some(Value::String(s)) = scrubbed.get(key) {
            if !s.is_empty() {
                scrubbed.insert(key.to_string(), json!(redact_github_tokens(s)));
            }
        }
    }
    Value::Object(scrubbed)
}

/// `(present, status, result_opt)` for a git push request.
fn git_push_request_result_in_state(
    state: &Value,
    request_id: &str,
) -> (bool, String, Option<Value>) {
    let Some(arr) = state.get("gitPushRequests").and_then(|v| v.as_array()) else {
        return (false, String::new(), None);
    };
    for request in arr {
        if request.get("id").and_then(|v| v.as_str()) != Some(request_id) {
            continue;
        }
        let status = request
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(result) = request.get("result") {
            if result.is_object() && result.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                return (true, status, Some(result.clone()));
            }
        }
        return (true, status, None);
    }
    (false, String::new(), None)
}

/// `(present, status, note)` for a plan request.
pub fn plan_request_outcome_in_state(
    state: &Value,
    plan_id: &str,
) -> (bool, String, Option<String>) {
    let Some(arr) = state.get("planApprovalRequests").and_then(|v| v.as_array()) else {
        return (false, String::new(), None);
    };
    for request in arr {
        if request.get("id").and_then(|v| v.as_str()) != Some(plan_id) {
            continue;
        }
        let status = request
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let note = request
            .get("note")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        return (true, status, note);
    }
    (false, String::new(), None)
}

pub fn plan_status_from_sidecar(projects_dir: &Path, plan_id: &str) -> Option<String> {
    let base = plans_dir(projects_dir);
    if !base.is_dir() {
        return None;
    }
    let Ok(entries) = std::fs::read_dir(&base) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path().join(format!("{plan_id}.json"));
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(status) = data.get("status").and_then(|v| v.as_str()) {
            if !status.is_empty() {
                return Some(status.to_string());
            }
        }
    }
    None
}

fn update_plan_sidecar_status(
    projects_dir: &Path,
    project_id: &str,
    plan_id: &str,
    status: &str,
    note: Option<&str>,
) {
    let Ok((_, sidecar_path)) = plan_artifact_paths(projects_dir, project_id, plan_id) else {
        return;
    };
    if !sidecar_path.exists() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(&sidecar_path) else {
        return;
    };
    let Ok(mut data) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    obj.insert("status".into(), json!(status));
    obj.insert("decidedAt".into(), json!(now_rfc3339()));
    if let Some(n) = note {
        obj.insert("note".into(), json!(n));
    }
    if let Ok(content) = serde_json::to_string_pretty(&data) {
        let _ = write_text_crash_safe(&sidecar_path, &content, "plan sidecar");
    }
}

// ── request_git_push ────────────────────────────────────────────────────────

pub fn request_git_push(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    project_id: &str,
    branch: Option<&str>,
    remote: Option<&str>,
    force: bool,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "request_git_push",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    let branch = match branch {
        Some(b) if !b.trim().is_empty() => Some(clean_text(b, "Branch", 200)?),
        _ => None,
    };
    let remote = validate_push_remote(remote)?;

    let request_id = new_hex_id();
    let created_at = now_rfc3339();
    let mut request = json!({
        "id": request_id,
        "agentId": agent_id,
        "projectId": project_id,
        "status": "pending_approval",
        "createdAt": created_at,
    });
    {
        let obj = request.as_object_mut().unwrap();
        if let Some(b) = &branch {
            obj.insert("branch".into(), json!(b));
        }
        if let Some(r) = &remote {
            obj.insert("remote".into(), json!(r));
        }
        if force {
            obj.insert("force".into(), json!(true));
        }
    }

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        // Live session check (owned status so we can drop the session borrow).
        {
            let session = find_session_mut(&mut state, &agent_id).ok_or_else(|| {
                ToolError::new(
                    "request_git_push requires a live session; register (and keep it active) before requesting a push.",
                )
            })?;
            let status = session
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if matches!(status.as_str(), "" | "closed" | "launch_pending") {
                return Err(ToolError::new(
                    "request_git_push requires a live session; register (and keep it active) before requesting a push.",
                ));
            }
        }
        let force_note = if force { " (FORCE)" } else { "" };
        let target = branch.as_deref().unwrap_or("current branch");
        let remote_label = remote.as_deref().unwrap_or("origin");
        // Cap check before needsUser so a refuse does not leave a dirty bell.
        {
            let requests = ensure_queue_array(&mut state, "gitPushRequests")?;
            refuse_if_pending_at_cap(
                requests,
                MAX_GIT_PUSH_REQUESTS,
                GIT_PUSH_TERMINAL,
                "git push",
            )?;
        }
        // HIGH #4: shared needsUser helper — refuse if a different outstanding reason.
        let session = find_session_mut(&mut state, &agent_id).ok_or_else(|| {
            ToolError::new(
                "request_git_push requires a live session; register (and keep it active) before requesting a push.",
            )
        })?;
        set_needs_user(
            session,
            "needs_push_approval",
            &format!("Awaiting approval to push {target}{force_note} to {remote_label}."),
            true,
        )?;
        {
            let requests = ensure_queue_array(&mut state, "gitPushRequests")?;
            requests.push(request.clone());
            let capped = cap_git_push_requests(std::mem::take(requests));
            *ensure_queue_array(&mut state, "gitPushRequests")? = capped;
        }
        add_event(
            &mut state,
            &agent_id,
            &role,
            "git_push_request",
            &format!("Requested human approval to push{force_note}."),
            Some(&project_id),
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })?;

    // Bounded poll.
    let deadline = Instant::now() + Duration::from_secs_f64(poll_timeout_secs());
    let interval = Duration::from_secs_f64(poll_interval_secs());
    let mut seen = false;
    loop {
        let (present, _status, result): (bool, String, Option<Value>) =
            with_agents_lock(projects_dir, || {
                let state = read_agents_state(projects_dir)?;
                Ok::<_, ToolError>(git_push_request_result_in_state(&state, &request_id))
            })?;
        if let Some(result) = result {
            return Ok(json!({
                "requestId": request_id,
                "result": scrub_push_result(&result),
            }));
        }
        if present {
            seen = true;
        } else if seen {
            // Vanished: clear bell only if no other outstanding gates remain.
            let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
                let mut state = read_agents_state(projects_dir)?;
                clear_gate_needs_user_if_idle(
                    &mut state,
                    &agent_id,
                    "needs_push_approval",
                    &request_id,
                );
                write_agents_state(projects_dir, state)?;
                Ok(())
            });
            return Ok(json!({
                "requestId": request_id,
                "result": {
                    "status": "push_failed",
                    "error": "push request vanished before producing a result.",
                },
            }));
        }
        if Instant::now() >= deadline {
            break;
        }
        if interval.is_zero() {
            break;
        }
        poll_sleep(interval);
    }

    // Timeout sweep: stamp only if still pending_approval.
    // HIGH #6: if already approved/pushing without result, return honest status.
    let mut synthesized = json!({
        "status": "timeout",
        "error": "push approval timed out — STOP, do not retry, do not push directly.",
    });
    let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let mut modified = false;
        let mut timed_out_pending = false;
        if let Some(arr) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("gitPushRequests"))
            .and_then(|v| v.as_array_mut())
        {
            for request in arr.iter_mut() {
                if request.get("id").and_then(|v| v.as_str()) != Some(request_id.as_str()) {
                    continue;
                }
                if let Some(existing) = request.get("result") {
                    if existing.is_object()
                        && existing.as_object().map(|o| !o.is_empty()).unwrap_or(false)
                    {
                        synthesized = existing.clone();
                        break;
                    }
                }
                let status = request
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if status == "pending_approval" {
                    if let Some(obj) = request.as_object_mut() {
                        obj.insert("status".into(), json!("timeout"));
                        obj.insert("result".into(), synthesized.clone());
                    }
                    modified = true;
                    timed_out_pending = true;
                } else if status == "approved" || status == "pushing" {
                    // HIGH #6: do not lie with `timeout` while human/UI owns the push.
                    synthesized = json!({
                        "status": status,
                        "inProgress": true,
                    });
                }
                break;
            }
        }
        // HIGH #5: clear needsUser only if no other pending gates remain
        // (after releasing the queue-array borrow).
        if timed_out_pending {
            clear_gate_needs_user_if_idle(
                &mut state,
                &agent_id,
                "needs_push_approval",
                &request_id,
            );
        }
        if modified {
            write_agents_state(projects_dir, state)?;
        }
        Ok(())
    });

    Ok(json!({
        "requestId": request_id,
        "result": scrub_push_result(&synthesized),
    }))
}

// ── plan_submit ─────────────────────────────────────────────────────────────

pub fn plan_submit(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    project_id: &str,
    title: &str,
    plan_markdown: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "plan_submit", session_token)?;
    let project_id = normalize_project_id(project_id)?;
    // Draft projects MUST accept plan_submit: the approval gate is what promotes
    // draft→active. Rejecting here deadlocks the planner (UI creates drafts, launch
    // is allowed on draft, but submit was blocked). Other mutations (mini, notes,
    // title, task claims) keep their draft rejection elsewhere.
    // Still resolve the project so a missing id fails closed.
    // Defense-in-depth: only active|draft may receive a plan; paused/done/archived
    // are terminal/idle and must not queue a new approval bell.
    let project = load_project_locked(projects_dir, &project_id)?;
    let pstatus = project.metadata.status();
    if !matches!(pstatus, "active" | "draft") {
        return Err(ToolError::new(format!(
            "plan_submit: project status must be active or draft (got '{pstatus}')."
        )));
    }
    let title = clean_text(title, "Plan title", 200)?;
    if strip_invisible_and_bidi(plan_markdown).trim().is_empty() {
        return Err(ToolError::new(
            "plan_submit requires a non-empty plan_markdown.",
        ));
    }
    if plan_markdown.len() > PLAN_MAX_MARKDOWN_CHARS {
        return Err(ToolError::new(format!(
            "plan_markdown is too long (max {PLAN_MAX_MARKDOWN_CHARS} characters)."
        )));
    }

    let plan_id = new_hex_id();
    let created_at = now_rfc3339();

    // Artifacts OUTSIDE the state lock (before queue append).
    let (md_path, sidecar_path) = plan_artifact_paths(projects_dir, &project_id, &plan_id)?;
    let sidecar = json!({
        "id": plan_id,
        "projectId": project_id,
        "agentId": agent_id,
        "title": title,
        "status": "pending_approval",
        "createdAt": created_at,
    });
    write_text_crash_safe(&md_path, plan_markdown, "plan markdown")?;
    write_text_crash_safe(
        &sidecar_path,
        &serde_json::to_string_pretty(&sidecar).map_err(|e| {
            ToolError::new(format!("Could not serialize plan sidecar: {e}"))
        })?,
        "plan sidecar",
    )?;

    let request = json!({
        "id": plan_id,
        "agentId": agent_id,
        "projectId": project_id,
        "title": title,
        "status": "pending_approval",
        "createdAt": created_at,
    });

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        {
            let session = find_session_mut(&mut state, &agent_id).ok_or_else(|| {
                ToolError::new(
                    "plan_submit requires a live session; register (and keep it active) before submitting a plan.",
                )
            })?;
            let status = session
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if matches!(status.as_str(), "" | "closed" | "launch_pending") {
                return Err(ToolError::new(
                    "plan_submit requires a live session; register (and keep it active) before submitting a plan.",
                ));
            }
        }
        // Cap check before needsUser so a refuse does not leave a dirty bell.
        {
            let requests = ensure_queue_array(&mut state, "planApprovalRequests")?;
            refuse_if_pending_at_cap(
                requests,
                MAX_PLAN_APPROVAL_REQUESTS,
                PLAN_TERMINAL,
                "plan approval",
            )?;
        }
        let session = find_session_mut(&mut state, &agent_id).ok_or_else(|| {
            ToolError::new(
                "plan_submit requires a live session; register (and keep it active) before submitting a plan.",
            )
        })?;
        set_needs_user(
            session,
            "needs_plan_approval",
            &format!("Plan '{title}' awaits approval."),
            true,
        )?;
        {
            let requests = ensure_queue_array(&mut state, "planApprovalRequests")?;
            requests.push(request.clone());
            let capped = cap_plan_approval_requests(std::mem::take(requests));
            *ensure_queue_array(&mut state, "planApprovalRequests")? = capped;
        }
        add_event(
            &mut state,
            &agent_id,
            &role,
            "plan_submit",
            &format!("Submitted a plan for approval: {title}."),
            Some(&project_id),
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })?;

    // Bounded poll for approved/rejected.
    let deadline = Instant::now() + Duration::from_secs_f64(poll_timeout_secs());
    let interval = Duration::from_secs_f64(poll_interval_secs());
    let mut seen = false;
    let mut first = true;
    loop {
        if !first && Instant::now() >= deadline {
            break;
        }
        first = false;
        let (present, status, note): (bool, String, Option<String>) =
            with_agents_lock(projects_dir, || {
                let state = read_agents_state(projects_dir)?;
                Ok::<_, ToolError>(plan_request_outcome_in_state(&state, &plan_id))
            })?;
        if present && PLAN_VERDICT.contains(&status.as_str()) {
            let mut result = json!({
                "planId": plan_id,
                "status": status,
            });
            if let Some(n) = note {
                result
                    .as_object_mut()
                    .unwrap()
                    .insert("note".into(), json!(n));
            }
            return Ok(result);
        }
        if present {
            seen = true;
        } else if seen {
            let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
                let mut state = read_agents_state(projects_dir)?;
                clear_gate_needs_user_if_idle(
                    &mut state,
                    &agent_id,
                    "needs_plan_approval",
                    &plan_id,
                );
                write_agents_state(projects_dir, state)?;
                Ok(())
            });
            return Ok(json!({
                "planId": plan_id,
                "status": "vanished",
                "note": "plan request vanished before producing a verdict.",
            }));
        }
        if Instant::now() >= deadline {
            break;
        }
        if interval.is_zero() {
            break;
        }
        poll_sleep(interval);
    }

    // Timeout sweep.
    let mut final_status = "timeout".to_string();
    let mut final_note: Option<String> = None;
    let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let mut modified = false;
        let mut timed_out_pending = false;
        if let Some(arr) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("planApprovalRequests"))
            .and_then(|v| v.as_array_mut())
        {
            for request in arr.iter_mut() {
                if request.get("id").and_then(|v| v.as_str()) != Some(plan_id.as_str()) {
                    continue;
                }
                let current = request
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let existing_note = request
                    .get("note")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if PLAN_VERDICT.contains(&current.as_str()) {
                    final_status = current;
                    final_note = existing_note;
                } else if current == "pending_approval" {
                    if let Some(obj) = request.as_object_mut() {
                        obj.insert("status".into(), json!("timeout"));
                        obj.insert("decidedAt".into(), json!(now_rfc3339()));
                    }
                    modified = true;
                    timed_out_pending = true;
                    final_status = "timeout".into();
                } else {
                    final_status = if current.is_empty() {
                        "timeout".into()
                    } else {
                        current
                    };
                    final_note = existing_note;
                }
                break;
            }
        }
        // HIGH #5: clear needsUser only if no other pending gates remain.
        if timed_out_pending {
            clear_gate_needs_user_if_idle(
                &mut state,
                &agent_id,
                "needs_plan_approval",
                &plan_id,
            );
        }
        if modified {
            write_agents_state(projects_dir, state)?;
        }
        Ok(())
    });

    update_plan_sidecar_status(
        projects_dir,
        &project_id,
        &plan_id,
        &final_status,
        final_note.as_deref(),
    );

    let mut result = json!({
        "planId": plan_id,
        "status": final_status,
    });
    if let Some(n) = final_note {
        result
            .as_object_mut()
            .unwrap()
            .insert("note".into(), json!(n));
    }
    Ok(result)
}

// ── plan_status ─────────────────────────────────────────────────────────────

pub fn plan_status(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    plan_id: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (_agent_id, _role) =
        require_agent_tool(projects_dir, agent_id, role, "plan_status", session_token)?;
    let plan_id = validate_plan_id(plan_id)?;

    let (present, status, note): (bool, String, Option<String>) =
        with_agents_lock(projects_dir, || {
            let state = read_agents_state(projects_dir)?;
            Ok::<_, ToolError>(plan_request_outcome_in_state(&state, &plan_id))
        })?;
    if present {
        let mut result = json!({
            "planId": plan_id,
            "status": if status.is_empty() { "pending_approval".into() } else { status },
        });
        if let Some(n) = note {
            result
                .as_object_mut()
                .unwrap()
                .insert("note".into(), json!(n));
        }
        return Ok(result);
    }

    // Durable sidecar fallback (bounded: plan_id is validated 32-hex).
    // Sidecar is agent-writable — never return authoritative "approved" (or any
    // queue verdict) from it alone. Report artifact_only so callers re-check the queue.
    if plan_status_from_sidecar(projects_dir, &plan_id).is_some() {
        let base = plans_dir(projects_dir);
        if base.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let path = entry.path().join(format!("{plan_id}.json"));
                    if !path.is_file() {
                        continue;
                    }
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        if let Ok(data) = serde_json::from_str::<Value>(&text) {
                            let mut result = json!({
                                "planId": plan_id,
                                "status": "artifact_only",
                            });
                            if let Some(n) = data.get("note").and_then(|v| v.as_str()) {
                                result
                                    .as_object_mut()
                                    .unwrap()
                                    .insert("note".into(), json!(n));
                            }
                            return Ok(result);
                        }
                    }
                }
            }
        }
        return Ok(json!({
            "planId": plan_id,
            "status": "artifact_only",
        }));
    }

    Ok(json!({
        "planId": plan_id,
        "status": "not_found",
    }))
}

// ── ask_user ────────────────────────────────────────────────────────────────

enum AskPollHit {
    Reply(String),
    Waiting,
    SessionGone,
}

pub fn ask_user(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    question: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "ask_user", session_token)?;
    let question = clean_text(question, "Question", 4000)?;
    let question_id = new_hex_id();
    let created_at = now_rfc3339();

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let session = find_session_mut(&mut state, &agent_id).ok_or_else(|| {
            ToolError::new(
                "ask_user requires a live session; register (and keep it active) before asking the human.",
            )
        })?;
        let status = session
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if matches!(status.as_str(), "" | "closed" | "launch_pending") {
            return Err(ToolError::new(
                "ask_user requires a live session; register (and keep it active) before asking the human.",
            ));
        }
        set_needs_user(session, "question", &question, true)?;
        session.insert(
            "pendingQuestion".into(),
            json!({
                "id": question_id,
                "question": question,
                "createdAt": created_at,
            }),
        );
        // New question supersedes any stale reply.
        session.remove("userReply");
        add_event(
            &mut state,
            &agent_id,
            &role,
            "ask_user",
            "Asked the human a question.",
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })?;

    let deadline = Instant::now() + Duration::from_secs_f64(poll_timeout_secs());
    let interval = Duration::from_secs_f64(poll_interval_secs());
    let mut first = true;
    loop {
        if !first && Instant::now() >= deadline {
            break;
        }
        first = false;

        let hit: AskPollHit = with_agents_lock(projects_dir, || {
            let mut state = read_agents_state(projects_dir)?;
            let Some(session) = find_session_mut(&mut state, &agent_id) else {
                return Ok::<_, ToolError>(AskPollHit::SessionGone);
            };
            let pending_id = session
                .get("pendingQuestion")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let reply = session.get("userReply").cloned();
            if let Some(reply) = reply {
                if let Some(robj) = reply.as_object() {
                    let reply_qid = robj
                        .get("questionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !pending_id.is_empty()
                        && reply_qid == question_id
                        && reply_qid == pending_id
                    {
                        let text = robj
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        session.remove("pendingQuestion");
                        session.remove("userReply");
                        clear_needs_user_if_reason(session, "question");
                        write_agents_state(projects_dir, state)?;
                        return Ok(AskPollHit::Reply(text));
                    }
                    // Stale reply — drop it, keep waiting.
                    session.remove("userReply");
                    write_agents_state(projects_dir, state)?;
                }
            }
            Ok(AskPollHit::Waiting)
        })?;

        match hit {
            AskPollHit::Reply(text) => return Ok(json!({ "reply": text })),
            AskPollHit::SessionGone => {
                return Ok(json!({
                    "timeout": true,
                    "note": "session ended before the human replied.",
                }));
            }
            AskPollHit::Waiting => {}
        }

        if Instant::now() >= deadline {
            break;
        }
        if interval.is_zero() {
            break;
        }
        poll_sleep(interval);
    }

    // Timeout: clear our pending question + question bell.
    let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let mut modified = false;
        if let Some(session) = find_session_mut(&mut state, &agent_id) {
            let pending_id = session
                .get("pendingQuestion")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if pending_id == question_id {
                session.remove("pendingQuestion");
                modified = true;
            }
            if needs_user_reason(session) == "question" {
                session.insert("needsUser".into(), Value::Null);
                modified = true;
            }
        }
        if modified {
            write_agents_state(projects_dir, state)?;
        }
        Ok(())
    });

    Ok(json!({ "timeout": true }))
}

// ── project_create_plan_tasks ───────────────────────────────────────────────

struct ParsedPlanTask {
    internal_id: String,
    title: String,
    acceptance: String,
    scope: Vec<String>,
    deps: Vec<String>,
    weight_main: bool,
}

pub fn project_create_plan_tasks(
    projects_dir: &Path,
    project_id: &str,
    plan_id: &str,
    tasks: &[Value],
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_create_plan_tasks",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    let plan_id = validate_plan_id(plan_id)?;
    if tasks.is_empty() {
        return Err(ToolError::new(
            "project_create_plan_tasks requires a non-empty tasks list.",
        ));
    }
    if tasks.len() > MAX_PLAN_TASKS {
        return Err(ToolError::new(format!(
            "Too many plan tasks: {} (max {MAX_PLAN_TASKS}).",
            tasks.len()
        )));
    }

    let mut seen_incoming = std::collections::HashSet::new();
    let mut parsed: Vec<ParsedPlanTask> = Vec::new();
    for entry in tasks {
        let obj = entry
            .as_object()
            .ok_or_else(|| ToolError::new("Each plan task must be an object."))?;
        let internal_id = normalize_task_id(
            obj.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        )?;
        if !seen_incoming.insert(internal_id.clone()) {
            return Err(ToolError::new(format!(
                "Duplicate plan task id in request: {internal_id}."
            )));
        }
        let title = clean_text(
            obj.get("title").and_then(|v| v.as_str()).unwrap_or(""),
            "Task title",
            500,
        )?;
        let acceptance = strip_invisible_and_bidi(
            obj.get("acceptance")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        )
        .trim()
        .chars()
        .take(4000)
        .collect::<String>();
        let raw_scope = obj.get("scope").cloned().unwrap_or(json!([]));
        let scope_arr = raw_scope.as_array().ok_or_else(|| {
            ToolError::new("Plan task scope must be a list of file paths.")
        })?;
        if !scope_arr.iter().all(|s| s.is_string()) {
            return Err(ToolError::new(
                "Plan task scope must be a list of file paths.",
            ));
        }
        if scope_arr.len() > MAX_PLAN_TASK_SCOPE {
            return Err(ToolError::new(format!(
                "Plan task {internal_id} scope has {} files (max {MAX_PLAN_TASK_SCOPE}).",
                scope_arr.len()
            )));
        }
        let scope: Vec<String> = scope_arr
            .iter()
            .map(|s| validate_plan_scope_path(s.as_str().unwrap_or("")))
            .collect::<ToolResult<_>>()?;
        let raw_deps = obj.get("dependsOn").cloned().unwrap_or(json!([]));
        let deps_arr = raw_deps.as_array().ok_or_else(|| {
            ToolError::new("Plan task dependsOn must be a list of task ids.")
        })?;
        if !deps_arr.iter().all(|d| d.is_string()) {
            return Err(ToolError::new(
                "Plan task dependsOn must be a list of task ids.",
            ));
        }
        let deps: Vec<String> = deps_arr
            .iter()
            .map(|d| normalize_task_id(d.as_str().unwrap_or("")))
            .collect::<ToolResult<_>>()?;

        let mut weight_main = false;
        if let Some(raw_weight) = obj.get("weight") {
            if !raw_weight.is_null() {
                let weight_str = match raw_weight {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let weight = weight_str.trim().to_ascii_lowercase();
                if !weight.is_empty() {
                    let weight: String = weight.chars().take(16).collect();
                    if weight != "mini" && weight != "main" {
                        return Err(ToolError::new(format!(
                            "task {internal_id} has invalid weight '{weight}' (allowed: 'mini' or 'main')"
                        )));
                    }
                    weight_main = weight == "main";
                }
            }
        }
        parsed.push(ParsedPlanTask {
            internal_id,
            title,
            acceptance,
            scope,
            deps,
            weight_main,
        });
    }

    for entry in &parsed {
        for dep in &entry.deps {
            if !seen_incoming.contains(dep) {
                return Err(ToolError::new(format!(
                    "Plan task {} dependsOn references id {dep} not in the request.",
                    entry.internal_id
                )));
            }
        }
    }

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;

        // BLOCKER #1: authorization comes ONLY from agents-state planApprovalRequests.
        // Sidecar JSON status=approved is NOT sufficient authz (agent-writable artifact).
        let (present, plan_status, _) = plan_request_outcome_in_state(&state, &plan_id);
        if !present {
            return Err(ToolError::new(format!(
                "project_create_plan_tasks: plan {plan_id} not_found in planApprovalRequests (queue is the sole authority; sidecar status is ignored)."
            )));
        }
        if plan_status != "approved" {
            return Err(ToolError::new(format!(
                "project_create_plan_tasks requires an approved plan (plan {plan_id} status: {}).",
                if plan_status.is_empty() {
                    "not_found"
                } else {
                    plan_status.as_str()
                }
            )));
        }

        // HIGH #2 + #3: bind projectId and enforce one-shot materialize under the agents lock.
        {
            let arr = state
                .get("planApprovalRequests")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    ToolError::new("project_create_plan_tasks: planApprovalRequests missing.")
                })?;
            let request = arr
                .iter()
                .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(plan_id.as_str()))
                .ok_or_else(|| {
                    ToolError::new(format!(
                        "project_create_plan_tasks: plan {plan_id} not_found in planApprovalRequests."
                    ))
                })?;
            let req_project = request
                .get("projectId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if req_project != project_id {
                return Err(ToolError::new(format!(
                    "project_create_plan_tasks: plan {plan_id} belongs to project '{req_project}', not '{project_id}'."
                )));
            }
            let already = request
                .get("tasksCreated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || request
                    .get("tasksMaterializedAt")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
            if already {
                return Err(ToolError::new(format!(
                    "project_create_plan_tasks: plan {plan_id} tasks already materialized (one-shot)."
                )));
            }
        }

        let lock = project_lock_path(projects_dir, &project_id)?;
        let path = project_path(projects_dir, &project_id)?;
        let (saved, id_map, created) = with_file_lock(&lock, || {
            if !path.exists() {
                return Err(ToolError::new("Project not found."));
            }
            let mut project = read_project_file(&path)?;
            let pstatus = project.metadata.status();
            if matches!(pstatus, "draft" | "paused" | "archived" | "done") {
                return Err(ToolError::new(
                    "Cannot create plan tasks on draft, paused, done or archived projects.",
                ));
            }
            let tasks_arr = project
                .state
                .as_object_mut()
                .and_then(|o| o.get_mut("tasks"))
                .and_then(|t| t.as_array_mut())
                .ok_or_else(|| ToolError::new("Project state tasks must be a list."))?;

            // Defensive one-shot: survive queue flag strip (e.g. Tauri rewrite missing
            // tasksCreated). Any board task already stamped with this planId means
            // materialize already ran.
            let already_on_board = tasks_arr.iter().any(|t| {
                t.get("planId")
                    .and_then(|v| v.as_str())
                    .map(|s| s == plan_id.as_str())
                    .unwrap_or(false)
                    || t.get("plan_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s == plan_id.as_str())
                        .unwrap_or(false)
            });
            if already_on_board {
                return Err(ToolError::new(format!(
                    "project_create_plan_tasks: plan {plan_id} tasks already materialized (one-shot; existing board tasks with this planId)."
                )));
            }

            let mut id_map = Map::new();
            let mut allocated_ids: Vec<String> = Vec::new();
            let mut working = tasks_arr.clone();
            for entry in &parsed {
                let new_id = next_task_id(&working);
                working.push(json!({ "id": new_id }));
                id_map.insert(entry.internal_id.clone(), json!(new_id));
                allocated_ids.push(new_id);
            }

            let ts = now_rfc3339();
            let mut created: Vec<Value> = Vec::new();
            let mut created_deps: Vec<(String, Vec<String>)> = Vec::new();
            for (entry, new_id) in parsed.iter().zip(allocated_ids.iter()) {
                let remapped_deps: Vec<String> = entry
                    .deps
                    .iter()
                    .map(|d| {
                        id_map
                            .get(d)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    })
                    .collect();
                let mut task = json!({
                    "id": new_id,
                    "title": entry.title,
                    "status": "todo",
                    "priority": "medium",
                    "assignee": null,
                    "due": null,
                    "linkedResources": [],
                    "updatedAt": ts,
                    "planId": plan_id,
                });
                let tobj = task.as_object_mut().unwrap();
                if entry.weight_main {
                    tobj.insert("weight".into(), json!("main"));
                }
                // NO-CHURN: omit empty scope/acceptance/dependsOn.
                if !entry.scope.is_empty() {
                    tobj.insert("scope".into(), json!(entry.scope));
                }
                if !entry.acceptance.is_empty() {
                    tobj.insert("acceptance".into(), json!(entry.acceptance));
                }
                if !remapped_deps.is_empty() {
                    tobj.insert("dependsOn".into(), json!(remapped_deps));
                }
                created_deps.push((new_id.clone(), remapped_deps));
                created.push(task);
            }
            validate_task_dependency_dag(&created_deps)?;

            for task in &created {
                tasks_arr.push(task.clone());
            }
            validate_project_state(&project.state)?;

            if let Some(notes) = project
                .state
                .as_object_mut()
                .and_then(|o| o.get_mut("notes"))
                .and_then(|n| n.as_array_mut())
            {
                notes.push(json!({
                    "id": note_id(),
                    "text": format!(
                        "{agent_id} ({role}) created {} task(s) from plan {plan_id}.",
                        created.len()
                    ),
                    "source": format!("agent:{agent_id}"),
                    "createdAt": ts,
                }));
            }
            project.metadata.set("status", "active".into());
            project.metadata.set("updated_at", now_rfc3339());
            let saved = write_project_file(projects_dir, project)?;
            Ok((saved, id_map, created))
        })?;

        // HIGH #3: mark one-shot materialize atomically with the success path
        // (still under agents lock; after project file write succeeded).
        let materialized_at = now_rfc3339();
        if let Some(arr) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("planApprovalRequests"))
            .and_then(|v| v.as_array_mut())
        {
            for request in arr.iter_mut() {
                if request.get("id").and_then(|v| v.as_str()) != Some(plan_id.as_str()) {
                    continue;
                }
                if let Some(obj) = request.as_object_mut() {
                    obj.insert("tasksCreated".into(), json!(true));
                    obj.insert("tasksMaterializedAt".into(), json!(materialized_at));
                }
                break;
            }
        }

        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            "plan_tasks",
            Some(&format!("Created {} plan task(s).", created.len())),
            None,
            None,
            None,
            Some(&project_id),
            None,
        )?;
        add_event(
            &mut state,
            &agent_id,
            &role,
            "plan_tasks",
            &format!("Created {} task(s) from plan {plan_id}.", created.len()),
            Some(&project_id),
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(json!({
            "project": public_project(&saved),
            "planId": plan_id,
            "idMap": id_map,
            "tasks": created,
        }))
    })
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_file::write_test_project;
    use crate::state::{seed_launch_pending, with_agents_lock};
    use crate::tools::agent_lifecycle::agent_register;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct PollTimeoutGuard;
    impl Drop for PollTimeoutGuard {
        fn drop(&mut self) {
            std::env::remove_var("DEVBOULE_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS");
            std::env::remove_var("DEVBOULE_MCP_HUMAN_GATE_POLL_INTERVAL_SECS");
        }
    }

    fn set_poll_timeout_zero() -> PollTimeoutGuard {
        std::env::set_var("DEVBOULE_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS", "0");
        std::env::set_var("DEVBOULE_MCP_HUMAN_GATE_POLL_INTERVAL_SECS", "0");
        PollTimeoutGuard
    }

    fn set_unmanaged(on: bool) {
        if on {
            std::env::set_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "1");
        } else {
            std::env::remove_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS");
            std::env::remove_var("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS");
            std::env::set_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "");
            std::env::set_var("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "");
        }
    }

    fn temp_projects() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        (tmp, projects)
    }

    fn register(projects: &Path, agent_id: &str, role: &str) -> String {
        let token = format!("launch-{agent_id}");
        seed_launch_pending(projects, agent_id, role, &token).unwrap();
        let ack = agent_register(
            projects,
            agent_id,
            role,
            Some("opus"),
            None,
            Some("reg"),
            Some(&token),
        )
        .unwrap();
        ack["sessionToken"].as_str().unwrap().to_string()
    }

    fn seed_active_project(projects: &Path) {
        write_test_project(
            projects,
            "scrna-seq",
            "scRNA",
            "active",
            json!([{
                "id": "T1",
                "title": "Manual",
                "status": "todo",
                "updatedAt": "2026-01-01T00:00:00Z",
            }]),
            &[],
        )
        .unwrap();
    }

    fn read_state(projects: &Path) -> Value {
        let raw = std::fs::read_to_string(projects.join(".aspis-agents.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn plan_submit_creates_pending_then_timeout() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");

        let out = plan_submit(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            "Refactor pipeline",
            "# Plan\n\n- step one\n",
            Some(&tok),
        )
        .unwrap();
        let plan_id = out["planId"].as_str().unwrap();
        assert_eq!(plan_id.len(), 32);
        assert!(plan_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(out["status"], "timeout");

        // Artifacts + queue camelCase.
        let md = projects
            .join(".aspis-plans")
            .join("scrna-seq")
            .join(format!("{plan_id}.md"));
        let side = projects
            .join(".aspis-plans")
            .join("scrna-seq")
            .join(format!("{plan_id}.json"));
        assert!(md.exists());
        assert!(side.exists());
        assert!(std::fs::read_to_string(&md).unwrap().contains("step one"));

        let state = read_state(&projects);
        let reqs = state["planApprovalRequests"].as_array().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0]["id"], plan_id);
        assert_eq!(reqs[0]["agentId"], "codex");
        assert_eq!(reqs[0]["projectId"], "scrna-seq");
        assert_eq!(reqs[0]["title"], "Refactor pipeline");
        assert_eq!(reqs[0]["status"], "timeout");
        assert!(reqs[0].get("project_id").is_none());
        assert!(reqs[0].get("plan_markdown").is_none());
    }

    #[test]
    fn plan_status_returns_queue_and_not_found() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");
        let out = plan_submit(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            "Title",
            "body text",
            Some(&tok),
        )
        .unwrap();
        let plan_id = out["planId"].as_str().unwrap();

        let st = plan_status(&projects, "codex", "coder", plan_id, Some(&tok)).unwrap();
        assert_eq!(st["planId"], plan_id);
        assert_eq!(st["status"], "timeout");

        let missing = plan_status(
            &projects,
            "codex",
            "coder",
            &"b".repeat(32),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(missing["status"], "not_found");

        let err = plan_status(&projects, "codex", "coder", "../etc/passwd", Some(&tok))
            .unwrap_err();
        assert!(err.message.contains("32 lowercase"), "{}", err.message);
    }

    #[test]
    fn plan_submit_rejects_paused_done_archived_allows_draft() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "codex", "coder");

        for status in ["paused", "done", "archived"] {
            write_test_project(
                &projects,
                "scrna-seq",
                "scRNA",
                status,
                json!([{
                    "id": "T1",
                    "title": "Manual",
                    "status": "todo",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }]),
                &[],
            )
            .unwrap();
            let err = plan_submit(
                &projects,
                "codex",
                "coder",
                "scrna-seq",
                "Nope",
                "# Plan\n\n- step\n",
                Some(&tok),
            )
            .unwrap_err();
            assert!(
                err.message.contains("active or draft") && err.message.contains(status),
                "status={status}: {}",
                err.message
            );
        }

        // Draft remains allowed (approval gate promotes draft→active).
        write_test_project(
            &projects,
            "scrna-seq",
            "scRNA",
            "draft",
            json!([{
                "id": "T1",
                "title": "Manual",
                "status": "todo",
                "updatedAt": "2026-01-01T00:00:00Z",
            }]),
            &[],
        )
        .unwrap();
        let out = plan_submit(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            "Draft plan",
            "# Plan\n\n- step\n",
            Some(&tok),
        )
        .unwrap();
        assert_eq!(out["status"], "timeout");
        assert!(out["planId"].as_str().unwrap().len() == 32);
    }

    #[test]
    fn plan_submit_rejects_verifier_and_mini() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let vtok = register(&projects, "vfx", "verifier");
        let err = plan_submit(
            &projects,
            "vfx",
            "verifier",
            "scrna-seq",
            "Nope",
            "x",
            Some(&vtok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("verifier") && err.message.contains("plan_submit"),
            "{}",
            err.message
        );
        assert!(!projects.join(".aspis-plans").exists());

        let mtok = register(&projects, "mini-1", "mini");
        let err = plan_submit(
            &projects,
            "mini-1",
            "mini",
            "scrna-seq",
            "Nope",
            "x",
            Some(&mtok),
        )
        .unwrap_err();
        assert!(err.message.contains("mini"), "{}", err.message);
    }

    #[test]
    fn timeout_does_not_clobber_human_verdict() {
        // Double-approve / spoof prevention: once human stamps approved, agent
        // timeout sweep must not overwrite.
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");

        // Manually inject a pending request then flip to approved before timeout sweep
        // by calling plan_submit after pre-seeding is hard; instead seed queue and
        // simulate timeout path via direct state + a zero-timeout submit that we
        // flip mid-flight is complex. Unit-test the sweep rule by writing state.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            let reqs = ensure_queue_array(&mut state, "planApprovalRequests")?;
            reqs.push(json!({
                "id": "a".repeat(32),
                "agentId": "codex",
                "projectId": "scrna-seq",
                "title": "t",
                "status": "approved",
                "note": "ship it",
                "createdAt": "2026-01-01T00:00:00Z",
            }));
            write_agents_state(&projects, state)?;
            Ok::<_, ToolError>(())
        })
        .unwrap();

        // Agent cannot "approve" — only read status.
        let st = plan_status(
            &projects,
            "codex",
            "coder",
            &"a".repeat(32),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(st["status"], "approved");
        assert_eq!(st["note"], "ship it");

        // Re-read raw: still approved (no agent write path for status).
        let state = read_state(&projects);
        assert_eq!(state["planApprovalRequests"][0]["status"], "approved");
    }

    #[test]
    fn request_git_push_queues_camel_case() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");

        let out = request_git_push(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            Some("main"),
            None,
            false,
            Some(&tok),
        )
        .unwrap();
        assert!(out.get("requestId").is_some());
        assert_eq!(out["result"]["status"], "timeout");

        let state = read_state(&projects);
        let r = &state["gitPushRequests"][0];
        assert_eq!(r["id"], out["requestId"]);
        assert_eq!(r["agentId"], "codex");
        assert_eq!(r["projectId"], "scrna-seq");
        assert_eq!(r["branch"], "main");
        assert_eq!(r["status"], "timeout");
        assert!(r.get("force").is_none());
        assert!(r.get("remote").is_none());
        assert!(r.get("project_id").is_none());
    }

    #[test]
    fn request_git_push_force_remote_and_role_denial() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");
        request_git_push(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            None,
            Some("up_stream-2/x.y"),
            true,
            Some(&tok),
        )
        .unwrap();
        let r = &read_state(&projects)["gitPushRequests"][0];
        assert_eq!(r["remote"], "up_stream-2/x.y");
        assert_eq!(r["force"], true);

        for bad in ["-origin", "https://evil/x.git", "ori gin", "a;b"] {
            let err = request_git_push(
                &projects,
                "codex",
                "coder",
                "scrna-seq",
                None,
                Some(bad),
                false,
                Some(&tok),
            )
            .unwrap_err();
            assert!(!err.message.is_empty(), "bad={bad}");
        }

        let vtok = register(&projects, "vfx", "verifier");
        let err = request_git_push(
            &projects,
            "vfx",
            "verifier",
            "scrna-seq",
            None,
            None,
            false,
            Some(&vtok),
        )
        .unwrap_err();
        assert!(err.message.contains("verifier"), "{}", err.message);

        let mtok = register(&projects, "mini-p", "mini");
        let err = request_git_push(
            &projects,
            "mini-p",
            "mini",
            "scrna-seq",
            None,
            None,
            false,
            Some(&mtok),
        )
        .unwrap_err();
        assert!(err.message.contains("mini"), "{}", err.message);
    }

    #[test]
    fn ask_user_queues_and_returns_reply() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");

        // Seed a matching reply after pending is set — run ask_user with short timeout
        // in a helper that pre-plants reply is hard; use zero timeout then plant reply
        // path: plant userReply while pending via threaded approve simulation.
        std::env::set_var("DEVBOULE_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS", "2");
        std::env::set_var("DEVBOULE_MCP_HUMAN_GATE_POLL_INTERVAL_SECS", "0.05");

        let projects2 = projects.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..80 {
                let ok = with_agents_lock(&projects2, || {
                    let mut state = read_agents_state(&projects2)?;
                    if let Some(session) = find_session_mut(&mut state, "codex") {
                        if let Some(pq) = session.get("pendingQuestion").and_then(|v| v.as_object())
                        {
                            let qid = pq.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            if !qid.is_empty() {
                                session.insert(
                                    "userReply".into(),
                                    json!({
                                        "questionId": qid,
                                        "text": "use option B",
                                        "createdAt": now_rfc3339(),
                                    }),
                                );
                                write_agents_state(&projects2, state)?;
                                return Ok::<_, ToolError>(true);
                            }
                        }
                    }
                    Ok(false)
                })
                .unwrap_or(false);
                if ok {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });

        let out = ask_user(
            &projects,
            "codex",
            "coder",
            "Which option should we ship?",
            Some(&tok),
        )
        .unwrap();
        handle.join().unwrap();
        assert_eq!(out["reply"], "use option B");
        assert!(out.get("timeout").is_none());

        std::env::remove_var("DEVBOULE_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS");
        std::env::remove_var("DEVBOULE_MCP_HUMAN_GATE_POLL_INTERVAL_SECS");
    }

    #[test]
    fn ask_user_timeout_and_verifier_allowed() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");
        let out = ask_user(&projects, "codex", "coder", "Anyone home?", Some(&tok)).unwrap();
        assert_eq!(out["timeout"], true);
        let state = read_state(&projects);
        let sess = state["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["agentId"] == "codex")
            .unwrap();
        assert!(sess.get("pendingQuestion").is_none() || sess["pendingQuestion"].is_null());

        let vtok = register(&projects, "vfx", "verifier");
        let out = ask_user(&projects, "vfx", "verifier", "Confirm risk?", Some(&vtok)).unwrap();
        assert_eq!(out["timeout"], true);
    }

    #[test]
    fn create_plan_tasks_requires_approved_and_remaps() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "orch", "orchestrator");
        let plan_id = "a".repeat(32);

        // Reject without approved plan in queue.
        let err = project_create_plan_tasks(
            &projects,
            "scrna-seq",
            &plan_id,
            &[json!({
                "id": "a",
                "title": "Scaffold",
                "scope": ["src/a.ts"],
                "acceptance": "builds",
                "dependsOn": [],
            })],
            "orch",
            "orchestrator",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("not_found") || err.message.contains("approved plan"),
            "{}",
            err.message
        );

        // Seed approved in queue.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            ensure_queue_array(&mut state, "planApprovalRequests")?.push(json!({
                "id": plan_id,
                "agentId": "orch",
                "projectId": "scrna-seq",
                "title": "Seeded",
                "status": "approved",
                "createdAt": "2026-01-01T00:00:00Z",
            }));
            write_agents_state(&projects, state)?;
            Ok::<_, ToolError>(())
        })
        .unwrap();

        let out = project_create_plan_tasks(
            &projects,
            "scrna-seq",
            &plan_id,
            &[
                json!({
                    "id": "a",
                    "title": "Scaffold module",
                    "scope": ["src/a.ts"],
                    "acceptance": "builds",
                    "dependsOn": [],
                }),
                json!({
                    "id": "b",
                    "title": "Wire it up",
                    "scope": ["src/b.ts"],
                    "acceptance": "tests pass",
                    "dependsOn": ["a"],
                    "weight": "main",
                }),
            ],
            "orch",
            "orchestrator",
            Some(&tok),
        )
        .unwrap();
        assert_eq!(out["planId"], plan_id);
        assert_eq!(out["idMap"]["a"], "T2");
        assert_eq!(out["idMap"]["b"], "T3");
        assert_eq!(out["tasks"][0]["status"], "todo");
        assert_eq!(out["tasks"][0]["planId"], plan_id);
        assert!(out["tasks"][0].get("dependsOn").is_none());
        assert_eq!(out["tasks"][1]["dependsOn"], json!(["T2"]));
        assert_eq!(out["tasks"][1]["weight"], "main");

        // One-shot flag stamped on the queue entry.
        let state = read_state(&projects);
        let req = state["planApprovalRequests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == plan_id)
            .unwrap();
        assert_eq!(req["tasksCreated"], true);
        assert!(req["tasksMaterializedAt"].as_str().unwrap().len() > 0);

        // Verifier denied.
        let vtok = register(&projects, "vfx2", "verifier");
        let err = project_create_plan_tasks(
            &projects,
            "scrna-seq",
            &plan_id,
            &[json!({"id": "z", "title": "x", "scope": [], "dependsOn": []})],
            "vfx2",
            "verifier",
            Some(&vtok),
        )
        .unwrap_err();
        assert!(err.message.contains("verifier"), "{}", err.message);
    }

    #[test]
    fn create_plan_tasks_rejects_sidecar_only_approved() {
        // BLOCKER #1: agent-writable sidecar must not authorize materialize.
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "orch", "orchestrator");
        let plan_id = "b".repeat(32);

        let base = projects.join(".aspis-plans").join("scrna-seq");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join(format!("{plan_id}.json")),
            serde_json::to_string_pretty(&json!({
                "id": plan_id,
                "projectId": "scrna-seq",
                "agentId": "orch",
                "title": "Spoofed",
                "status": "approved",
                "createdAt": "2026-01-01T00:00:00Z",
            }))
            .unwrap(),
        )
        .unwrap();
        // Queue has NO entry for this plan_id.

        let err = project_create_plan_tasks(
            &projects,
            "scrna-seq",
            &plan_id,
            &[json!({
                "id": "a",
                "title": "Scaffold",
                "scope": ["src/a.ts"],
                "dependsOn": [],
            })],
            "orch",
            "orchestrator",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("not_found") || err.message.contains("planApprovalRequests"),
            "sidecar-only must be rejected: {}",
            err.message
        );
    }

    #[test]
    fn create_plan_tasks_rejects_wrong_project_id() {
        // HIGH #2: request.projectId must match the project_id argument.
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        write_test_project(
            &projects,
            "other-proj",
            "Other",
            "active",
            json!([]),
            &[],
        )
        .unwrap();
        let tok = register(&projects, "orch", "orchestrator");
        let plan_id = "c".repeat(32);

        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            ensure_queue_array(&mut state, "planApprovalRequests")?.push(json!({
                "id": plan_id,
                "agentId": "orch",
                "projectId": "scrna-seq",
                "title": "Bound",
                "status": "approved",
                "createdAt": "2026-01-01T00:00:00Z",
            }));
            write_agents_state(&projects, state)?;
            Ok::<_, ToolError>(())
        })
        .unwrap();

        let err = project_create_plan_tasks(
            &projects,
            "other-proj",
            &plan_id,
            &[json!({
                "id": "a",
                "title": "Cross project",
                "scope": [],
                "dependsOn": [],
            })],
            "orch",
            "orchestrator",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("belongs to project") || err.message.contains("scrna-seq"),
            "{}",
            err.message
        );
    }

    #[test]
    fn create_plan_tasks_second_materialize_fails() {
        // HIGH #3: one-shot — second create on same plan is rejected.
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "orch", "orchestrator");
        let plan_id = "d".repeat(32);

        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            ensure_queue_array(&mut state, "planApprovalRequests")?.push(json!({
                "id": plan_id,
                "agentId": "orch",
                "projectId": "scrna-seq",
                "title": "Once",
                "status": "approved",
                "createdAt": "2026-01-01T00:00:00Z",
            }));
            write_agents_state(&projects, state)?;
            Ok::<_, ToolError>(())
        })
        .unwrap();

        let task = json!({
            "id": "a",
            "title": "First wave",
            "scope": ["src/a.ts"],
            "dependsOn": [],
        });
        project_create_plan_tasks(
            &projects,
            "scrna-seq",
            &plan_id,
            &[task.clone()],
            "orch",
            "orchestrator",
            Some(&tok),
        )
        .unwrap();

        let err = project_create_plan_tasks(
            &projects,
            "scrna-seq",
            &plan_id,
            &[json!({
                "id": "b",
                "title": "Second wave",
                "scope": ["src/b.ts"],
                "dependsOn": [],
            })],
            "orch",
            "orchestrator",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("already materialized") || err.message.contains("one-shot"),
            "{}",
            err.message
        );
    }

    #[test]
    fn create_plan_tasks_rejects_when_board_has_plan_id_even_if_flags_stripped() {
        // P3 BLOCKER defensive: if queue tasksCreated was stripped by a rewrite,
        // existing board tasks with this planId still block re-materialize.
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let plan_id = "e".repeat(32);
        write_test_project(
            &projects,
            "scrna-seq",
            "scRNA",
            "active",
            json!([{
                "id": "T1",
                "title": "Already from plan",
                "status": "todo",
                "updatedAt": "2026-01-01T00:00:00Z",
                "planId": plan_id,
            }]),
            &[],
        )
        .unwrap();
        let tok = register(&projects, "orch", "orchestrator");

        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            // Approved but NO tasksCreated flag (simulates Tauri strip).
            ensure_queue_array(&mut state, "planApprovalRequests")?.push(json!({
                "id": plan_id,
                "agentId": "orch",
                "projectId": "scrna-seq",
                "title": "Stripped flags",
                "status": "approved",
                "createdAt": "2026-01-01T00:00:00Z",
            }));
            write_agents_state(&projects, state)?;
            Ok::<_, ToolError>(())
        })
        .unwrap();

        let err = project_create_plan_tasks(
            &projects,
            "scrna-seq",
            &plan_id,
            &[json!({
                "id": "a",
                "title": "Duplicate wave",
                "scope": ["src/a.ts"],
                "dependsOn": [],
            })],
            "orch",
            "orchestrator",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("already materialized")
                || err.message.contains("existing board tasks"),
            "{}",
            err.message
        );
    }

    #[test]
    fn plan_status_sidecar_only_is_artifact_only_not_approved() {
        // Sidecar alone must not report authoritative approved.
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");
        let plan_id = "f".repeat(32);

        let base = projects.join(".aspis-plans").join("scrna-seq");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join(format!("{plan_id}.json")),
            serde_json::to_string_pretty(&json!({
                "id": plan_id,
                "projectId": "scrna-seq",
                "agentId": "codex",
                "title": "Sidecar only",
                "status": "approved",
                "createdAt": "2026-01-01T00:00:00Z",
                "note": "spoof",
            }))
            .unwrap(),
        )
        .unwrap();

        let st = plan_status(&projects, "codex", "coder", &plan_id, Some(&tok)).unwrap();
        assert_eq!(st["status"], "artifact_only");
        assert_ne!(st["status"], "approved");
    }

    #[test]
    fn cap_plan_protects_approved_without_tasks_created() {
        let mut reqs = Vec::new();
        // Fill with materializable-approved (no tasksCreated) — must not be dropped.
        for i in 0..MAX_PLAN_APPROVAL_REQUESTS {
            reqs.push(json!({
                "id": format!("pending-mat-{i:02}"),
                "status": "approved",
                "createdAt": format!("2026-06-06T01:00:{i:02}.000Z"),
            }));
        }
        // Plus older rejected/timeout that SHOULD be preferred for eviction.
        reqs.insert(
            0,
            json!({
                "id": "old-rejected",
                "status": "rejected",
                "createdAt": "2026-06-05T00:00:00Z",
            }),
        );
        reqs.insert(
            1,
            json!({
                "id": "old-timeout",
                "status": "timeout",
                "createdAt": "2026-06-05T01:00:00Z",
            }),
        );
        // And one approved that already materialized — free to drop.
        reqs.insert(
            2,
            json!({
                "id": "old-done",
                "status": "approved",
                "tasksCreated": true,
                "tasksMaterializedAt": "2026-06-05T02:00:00Z",
                "createdAt": "2026-06-05T02:00:00Z",
            }),
        );
        let capped = cap_plan_approval_requests(reqs);
        assert_eq!(capped.len(), MAX_PLAN_APPROVAL_REQUESTS);
        // Unmaterialized approved must survive.
        for i in 0..MAX_PLAN_APPROVAL_REQUESTS {
            let id = format!("pending-mat-{i:02}");
            assert!(
                capped.iter().any(|r| r["id"] == id),
                "missing protected {id}"
            );
        }
        // Evictable terminals (rejected/timeout/materialized-approved) drop first.
        assert!(!capped.iter().any(|r| r["id"] == "old-done"));
        assert!(!capped.iter().any(|r| r["id"] == "old-rejected"));
        assert!(!capped.iter().any(|r| r["id"] == "old-timeout"));
    }

    #[test]
    fn request_git_push_needs_user_conflicts_with_ask_user() {
        // HIGH #4: set_needs_user refuses a different outstanding reason.
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");

        // Plant an outstanding ask_user needsUser(reason=question).
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            let session = find_session_mut(&mut state, "codex").unwrap();
            session.insert(
                "needsUser".into(),
                json!({
                    "reason": "question",
                    "message": "Which option?",
                    "since": "2026-01-01T00:00:00Z",
                }),
            );
            write_agents_state(&projects, state)?;
            Ok::<_, ToolError>(())
        })
        .unwrap();

        let err = request_git_push(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            Some("main"),
            None,
            false,
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("outstanding needsUser")
                && err.message.contains("question"),
            "{}",
            err.message
        );

        // Queue must not have grown on the failed path.
        let state = read_state(&projects);
        let pushes = state
            .get("gitPushRequests")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        assert_eq!(pushes, 0);
    }

    #[test]
    fn push_timeout_returns_honest_status_when_approved_without_result() {
        // HIGH #6: approved/pushing without result must not report lying `timeout`.
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let tok = register(&projects, "codex", "coder");

        // Pre-plant a request that will be flipped to approved mid-flight is hard
        // with zero timeout; instead exercise the timeout-sweep branch by seeding
        // an approved request and calling the public path after enqueueing one that
        // times out. Simulate by writing state then re-running the sweep logic via
        // request_git_push after the request is already approved: the poll finds
        // present-without-result and times out immediately.
        //
        // Practical approach: enqueue via request_git_push (zero timeout → timeout
        // sweep). Then manually set status=approved without result and re-invoke
        // a zero-timeout poll is not exposed. Unit-test the sweep by writing an
        // approved request and calling request_git_push is wrong id. Instead:
        // start request_git_push after pre-seeding is complex; seed request id that
        // request_git_push will create is unknown.
        //
        // So: call request_git_push which times out to status=timeout, then rewrite
        // that entry to approved without result, and call a second request_git_push
        // that itself times out — not testing the first id.
        //
        // Direct approach: seed gitPushRequests with known id, plant needsUser,
        // then use internal path. Since sweep is private, we test via a short
        // poll by hijacking: submit push, immediately flip to approved in a
        // background thread before timeout (use non-zero short timeout).
        std::env::set_var("DEVBOULE_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS", "0.4");
        std::env::set_var("DEVBOULE_MCP_HUMAN_GATE_POLL_INTERVAL_SECS", "0.05");

        let projects2 = projects.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..40 {
                let ok = with_agents_lock(&projects2, || {
                    let mut state = read_agents_state(&projects2)?;
                    if let Some(arr) = state
                        .as_object_mut()
                        .and_then(|o| o.get_mut("gitPushRequests"))
                        .and_then(|v| v.as_array_mut())
                    {
                        if let Some(req) = arr.first_mut() {
                            if req.get("status").and_then(|v| v.as_str())
                                == Some("pending_approval")
                            {
                                if let Some(obj) = req.as_object_mut() {
                                    obj.insert("status".into(), json!("approved"));
                                    // deliberately no result
                                }
                                write_agents_state(&projects2, state)?;
                                return Ok::<_, ToolError>(true);
                            }
                        }
                    }
                    Ok(false)
                })
                .unwrap_or(false);
                if ok {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });

        let out = request_git_push(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            Some("main"),
            None,
            false,
            Some(&tok),
        )
        .unwrap();
        handle.join().unwrap();

        assert_eq!(out["result"]["status"], "approved");
        assert_eq!(out["result"]["inProgress"], true);
        assert_ne!(out["result"]["status"], "timeout");

        std::env::remove_var("DEVBOULE_MCP_HUMAN_GATE_POLL_TIMEOUT_SECS");
        std::env::remove_var("DEVBOULE_MCP_HUMAN_GATE_POLL_INTERVAL_SECS");
    }

    #[test]
    fn session_token_required_for_plan_submit() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        seed_active_project(&projects);
        let _tok = register(&projects, "codex", "coder");
        let err = plan_submit(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            "T",
            "body",
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("session_token") || err.message.contains("session token"),
            "{}",
            err.message
        );
        let err = plan_submit(
            &projects,
            "codex",
            "coder",
            "scrna-seq",
            "T",
            "body",
            Some("wrong"),
        )
        .unwrap_err();
        assert!(
            err.message.contains("invalid") || err.message.contains("session"),
            "{}",
            err.message
        );
    }

    #[test]
    fn scrub_push_result_redacts_github_tokens() {
        let scrubbed = scrub_push_result(&json!({
            "status": "push_failed",
            "error": "auth failed ghp_ABCDEFG1234567890 rest",
            "output": "token github_pat_xyz_abc used",
        }));
        assert!(!scrubbed["error"].as_str().unwrap().contains("ghp_"));
        assert!(scrubbed["error"]
            .as_str()
            .unwrap()
            .contains("[redacted-github-token]"));
        assert!(scrubbed["output"]
            .as_str()
            .unwrap()
            .contains("[redacted-github-token]"));
    }

    #[test]
    fn cap_keeps_pending_evicts_terminal() {
        let mut reqs = vec![json!({
            "id": "old",
            "status": "pushed",
            "createdAt": "2026-06-06T00:00:01Z",
        })];
        for i in 0..MAX_GIT_PUSH_REQUESTS {
            reqs.push(json!({
                "id": format!("t{i}"),
                "status": "denied",
                "createdAt": format!("2026-06-06T01:00:{i:02}.000Z"),
            }));
        }
        reqs.push(json!({
            "id": "active",
            "status": "pending_approval",
            "createdAt": "2026-06-06T02:00:00Z",
        }));
        let capped = cap_git_push_requests(reqs);
        assert!(capped.len() <= MAX_GIT_PUSH_REQUESTS + 1);
        assert!(capped.iter().any(|r| r["id"] == "active"));
        // Oldest terminal "old" should be among first dropped.
        assert!(!capped.iter().any(|r| r["id"] == "old"));
    }
}
