//! Design + visual directive tools (P6): `visual_check`, `design_request`.
//!
//! # Architecture
//!
//! MCP appends directives to agents-state queues; the Tauri app executor drains
//! them (same pattern as mini_coder). Fail-closed when the executor never claims
//! the directive within the poll window.
//!
//! # Security
//!
//! * Role allowlists + session token + live session gate.
//! * design outcome path validation (F-02-013): relative only, no `..` / abs.
//! * visual html_path: relative project path only (no absolute / `..` / drive roots).
//! * Queue caps so agents cannot unbounded-grow state.

use crate::state::{
    add_event, clean_text, find_session, now_rfc3339, read_agents_state, with_agents_lock,
    write_agents_state, ToolError, ToolResult,
};
use crate::tools::agent_lifecycle::require_agent_tool;
use serde_json::{json, Value};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_VISUAL_CHECK_DIRECTIVES: usize = 50;
const MAX_DESIGN_REQUEST_DIRECTIVES: usize = 20;

const VISUAL_CHECK_POLL_TIMEOUT_SECS: f64 = 120.0;
const VISUAL_CHECK_POLL_INTERVAL_SECS: f64 = 0.75;
const VISUAL_CHECK_MAX_FOCUS_CHARS: usize = 500;
const VISUAL_CHECK_MAX_HTML_PATH_CHARS: usize = 1024;

const DESIGN_REQUEST_POLL_TIMEOUT_SECS: f64 = 300.0;
const DESIGN_REQUEST_POLL_INTERVAL_SECS: f64 = 2.0;

/// Synchronous grace: the MOST a single `design_request` / `design_result` call blocks
/// before returning a non-error `{directiveId, status:"running"}` body. Kept well under
/// the agent's MCP client request timeout (~60s for pi) so a slow design generation never
/// surfaces as a transport `-32001 Request timed out` that loses the directiveId. On expiry
/// we DO NOT stamp the directive — the frontend watcher is still generating; the agent
/// collects the outcome later with `design_result`. Env-overridable (tests set 0).
/// 20s stays under common MCP client request timeouts (pi ~30s) with margin for the
/// last poll read + lock contention.
const DESIGN_SYNC_GRACE_SECS: f64 = 20.0;

/// Absolute abandonment ceiling: if a directive has aged past this since `createdAt`
/// WITHOUT completing, `design_result` / `design_request` stamp a synthesized `timeout`
/// so the agent gets a definitive terminal result instead of polling `running` forever.
/// This is the backstop the frontend watcher can't guarantee (app closed / model stall /
/// generation hang mid-flight). Mirrors mini_coder's timeout stamp, but time-based rather
/// than tied to a single blocking call. Env-overridable.
const DESIGN_ABANDON_CEILING_SECS: f64 = 300.0;

const VISUAL_TERMINAL: &[&str] = &["done", "failed", "timeout"];

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

fn visual_poll_timeout() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_VISUAL_CHECK_POLL_TIMEOUT_SECS",
            "ASPIS_MCP_VISUAL_CHECK_POLL_TIMEOUT_SECS",
        ],
        VISUAL_CHECK_POLL_TIMEOUT_SECS,
    )
    .min(VISUAL_CHECK_POLL_TIMEOUT_SECS)
}

fn visual_poll_interval() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_VISUAL_CHECK_POLL_INTERVAL_SECS",
            "ASPIS_MCP_VISUAL_CHECK_POLL_INTERVAL_SECS",
        ],
        VISUAL_CHECK_POLL_INTERVAL_SECS,
    )
}

fn design_poll_interval() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS",
            "ASPIS_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS",
        ],
        DESIGN_REQUEST_POLL_INTERVAL_SECS,
    )
}

fn design_sync_grace() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_DESIGN_SYNC_GRACE_SECS",
            "ASPIS_MCP_DESIGN_SYNC_GRACE_SECS",
        ],
        DESIGN_SYNC_GRACE_SECS,
    )
    .min(DESIGN_REQUEST_POLL_TIMEOUT_SECS)
}

fn design_abandon_ceiling() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_DESIGN_ABANDON_CEILING_SECS",
            "ASPIS_MCP_DESIGN_ABANDON_CEILING_SECS",
        ],
        DESIGN_ABANDON_CEILING_SECS,
    )
}

/// Read the full design directive object by id (or None if absent). Used for ownership +
/// age checks that `directive_result` (status/result only) does not expose.
fn read_design_directive(projects_dir: &Path, directive_id: &str) -> Option<Value> {
    let read: ToolResult<Option<Value>> = with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let list = state
            .get("designRequestDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(list
            .into_iter()
            .find(|d| d.get("id").and_then(|v| v.as_str()) == Some(directive_id)))
    });
    read.unwrap_or(None)
}

/// True when the directive was created more than `ceiling_secs` ago (RFC3339 `createdAt`).
/// Fails safe to `false` on a missing/unparseable timestamp (never falsely abandons).
fn directive_age_exceeds(directive: &Value, ceiling_secs: f64) -> bool {
    let created = directive
        .get("createdAt")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match chrono::DateTime::parse_from_rfc3339(created) {
        Ok(dt) => {
            let age = chrono::Utc::now().signed_duration_since(dt.with_timezone(&chrono::Utc));
            age.num_seconds() as f64 > ceiling_secs
        }
        Err(_) => false,
    }
}

/// After the sync grace expires without a terminal result: if the directive has aged past
/// the abandonment ceiling, stamp a synthesized `timeout` (so a hung/abandoned generation
/// resolves definitively instead of polling `running` forever) and return that terminal
/// body; otherwise return the non-error "still working" body to poll again.
fn finalize_or_running(projects_dir: &Path, directive_id: &str) -> Value {
    if let Some(dir) = read_design_directive(projects_dir, directive_id) {
        let terminal_status = dir
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "done" | "failed" | "timeout"))
            .unwrap_or(false);
        if !terminal_status && directive_age_exceeds(&dir, design_abandon_ceiling()) {
            let synthesized = json!({
                "status": "timeout",
                "error": "design generation did not complete within the expected window (the app may have closed or the model stalled). Re-issue design_request.",
            });
            stamp_terminal_if_needed(
                projects_dir,
                "designRequestDirectives",
                directive_id,
                &synthesized,
            );
            // The watcher may have raced us to a real result; prefer it.
            let (_, _, result) =
                directive_result(projects_dir, "designRequestDirectives", directive_id);
            return design_tool_result(directive_id, result.as_ref().unwrap_or(&synthesized))
                .unwrap_or_else(|_| {
                    json!({ "directiveId": directive_id, "error": "design generation timed out." })
                });
        }
    }
    design_running_body(projects_dir, directive_id)
}

/// Poll the design directive until it reaches a terminal outcome or `deadline` passes.
/// Returns `Some(terminal tool result)` when the directive is done/failed/timeout (or
/// vanished after being seen); `None` if still pending/running at the deadline. NEVER
/// stamps the directive — the frontend watcher owns completion; the agent re-polls via
/// `design_result`. Runs inside `spawn_blocking` at the call sites.
fn poll_design_terminal(
    projects_dir: &Path,
    directive_id: &str,
    deadline: Instant,
) -> ToolResult<Option<Value>> {
    let mut seen = false;
    loop {
        let (present, status, result) =
            directive_result(projects_dir, "designRequestDirectives", directive_id);
        if let Some(result) = result {
            return Ok(Some(design_tool_result(directive_id, &result)?));
        }
        if present {
            seen = true;
            let st = status.trim().to_ascii_lowercase();
            if matches!(st.as_str(), "done" | "failed" | "timeout") {
                // Terminal status without a result payload — synthesize a terminal body.
                let syn = json!({
                    "status": st,
                    "error": format!("design ended with status '{st}' without a result payload."),
                });
                return Ok(Some(design_tool_result(directive_id, &syn)?));
            }
        } else if seen {
            return Ok(Some(json!({
                "directiveId": directive_id,
                "error": "design directive vanished before producing a result.",
            })));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_secs_f64(design_poll_interval()));
    }
}

/// Non-terminal "still working" body returned when the sync grace expires. Distinguishes
/// a not-yet-claimed directive (app maybe offline) from one actively generating, and tells
/// the agent to collect the outcome via `design_result` (which carries the registryId).
fn design_running_body(projects_dir: &Path, directive_id: &str) -> Value {
    let (present, status, _) =
        directive_result(projects_dir, "designRequestDirectives", directive_id);
    let st = status.trim().to_ascii_lowercase();
    let (report, note) = if !present {
        (
            "not_found".to_string(),
            "design directive not found (evicted?) — re-issue design_request.".to_string(),
        )
    } else if st == "pending" {
        (
            "pending".to_string(),
            "design not yet claimed by the app — if this persists the Devboule design executor may be offline. Call design_result to keep polling.".to_string(),
        )
    } else {
        (
            "running".to_string(),
            "design still generating — call design_result with this directiveId to collect the registryId, then design_request(refine_from=<registryId>) to iterate.".to_string(),
        )
    };
    json!({
        "directiveId": directive_id,
        "status": report,
        "note": note,
    })
}

/// F-02-013 — design outcome path must be relative project path only.
pub fn validate_design_outcome_path(path: &str) -> ToolResult<()> {
    let s = path.trim();
    if s.is_empty() {
        return Err(ToolError::new("design project path is empty"));
    }
    if s.len() > 1024 {
        return Err(ToolError::new("design project path is too long"));
    }
    if s.chars().any(|ch| (ch as u32) < 32 || ch == '\u{7f}') {
        return Err(ToolError::new(
            "design project path must not contain control characters",
        ));
    }
    let normalized = s.replace('\\', "/");
    if normalized.starts_with('/') || normalized.starts_with('~') {
        return Err(ToolError::new("design project path must be relative"));
    }
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(ToolError::new("design project path must be relative"));
    }
    for seg in normalized.split('/') {
        if seg == ".." {
            return Err(ToolError::new(
                "design project path must not contain '..'",
            ));
        }
        if seg.is_empty() {
            return Err(ToolError::new(
                "design project path must not contain empty segments",
            ));
        }
    }
    Ok(())
}

fn clean_visual_html_path(value: &str) -> ToolResult<String> {
    let text = value.trim();
    if text.is_empty() {
        return Err(ToolError::new("visual_check requires html_path."));
    }
    if text.len() > VISUAL_CHECK_MAX_HTML_PATH_CHARS {
        return Err(ToolError::new("visual_check html_path is too long."));
    }
    if text.chars().any(|ch| (ch as u32) < 32 || ch == '\u{7f}') {
        return Err(ToolError::new(
            "visual_check html_path must not contain control characters.",
        ));
    }
    // Same relative-path confinement as validate_design_outcome_path / validate_mini_rel_path.
    let normalized = text.replace('\\', "/");
    if normalized.starts_with('/') || normalized.starts_with('~') {
        return Err(ToolError::new(
            "visual_check html_path must be a project-relative path.",
        ));
    }
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(ToolError::new(
            "visual_check html_path must be a project-relative path.",
        ));
    }
    for seg in normalized.split('/') {
        if seg == ".." {
            return Err(ToolError::new(
                "visual_check html_path must not contain '..'.",
            ));
        }
        if seg.is_empty() {
            return Err(ToolError::new(
                "visual_check html_path must not contain empty segments.",
            ));
        }
    }
    Ok(normalized)
}

fn directive_sort_key(d: &Value) -> (String, String) {
    (
        d.get("createdAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        d.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    )
}

fn is_visual_terminal(d: &Value) -> bool {
    let st = d
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    VISUAL_TERMINAL.iter().any(|t| *t == st)
}

/// Bound the visual-check queue. Prefer dropping oldest terminal first; if still
/// over max, drop oldest pending/running so agents cannot unbounded-grow state.
fn cap_visual_check_directives(directives: Vec<Value>) -> Vec<Value> {
    let clean: Vec<Value> = directives.into_iter().filter(|d| d.is_object()).collect();
    if clean.len() <= MAX_VISUAL_CHECK_DIRECTIVES {
        return clean;
    }
    let mut drop_count = clean.len() - MAX_VISUAL_CHECK_DIRECTIVES;
    let mut drop_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut terminal: Vec<&Value> = clean.iter().filter(|d| is_visual_terminal(d)).collect();
    terminal.sort_by_key(|d| directive_sort_key(d));
    for d in terminal {
        if drop_count == 0 {
            break;
        }
        if let Some(id) = d.get("id").and_then(|v| v.as_str()) {
            drop_ids.insert(id.to_string());
            drop_count -= 1;
        }
    }

    if drop_count > 0 {
        // Still over max: drop oldest non-terminal (pending/running) by createdAt.
        let mut pending: Vec<&Value> = clean
            .iter()
            .filter(|d| {
                !is_visual_terminal(d)
                    && d.get("id")
                        .and_then(|v| v.as_str())
                        .map(|id| !drop_ids.contains(id))
                        .unwrap_or(true)
            })
            .collect();
        pending.sort_by_key(|d| directive_sort_key(d));
        for d in pending {
            if drop_count == 0 {
                break;
            }
            if let Some(id) = d.get("id").and_then(|v| v.as_str()) {
                drop_ids.insert(id.to_string());
                drop_count -= 1;
            }
        }
    }

    clean
        .into_iter()
        .filter(|d| {
            d.get("id")
                .and_then(|v| v.as_str())
                .map(|id| !drop_ids.contains(id))
                .unwrap_or(true)
        })
        .collect()
}

fn cap_design_request_directives(directives: Vec<Value>) -> Vec<Value> {
    let clean: Vec<Value> = directives.into_iter().filter(|d| d.is_object()).collect();
    if clean.len() <= MAX_DESIGN_REQUEST_DIRECTIVES {
        return clean;
    }
    clean
        .into_iter()
        .rev()
        .take(MAX_DESIGN_REQUEST_DIRECTIVES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn require_live_session(projects_dir: &Path, agent_id: &str, tool: &str) -> ToolResult<()> {
    with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let session = find_session(&state, agent_id);
        let status = session
            .and_then(|s| s.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if session.is_none() || status.is_empty() || status == "closed" || status == "launch_pending"
        {
            return Err(ToolError::new(format!(
                "{tool} requires a live registered session."
            )));
        }
        Ok(())
    })
}

fn directive_result(
    projects_dir: &Path,
    queue_key: &str,
    directive_id: &str,
) -> (bool, String, Option<Value>) {
    let read: ToolResult<(bool, String, Option<Value>)> = with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let list = state
            .get(queue_key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for d in list {
            if d.get("id").and_then(|v| v.as_str()) == Some(directive_id) {
                let status = d
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let result = d.get("result").cloned().filter(|r| {
                    r.is_object() && r.as_object().map(|o| !o.is_empty()).unwrap_or(false)
                });
                return Ok((true, status, result));
            }
        }
        Ok((false, String::new(), None))
    });
    read.unwrap_or((false, String::new(), None))
}

fn visual_tool_result(directive_id: &str, result: &Value) -> ToolResult<Value> {
    let status = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if status == "done" {
        let critique = clean_text(
            result
                .get("critique")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            "Visual critique",
            4000,
        )?;
        return Ok(json!({
            "directiveId": directive_id,
            "critique": critique,
        }));
    }
    let error = clean_text(
        result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("visual_check failed."),
        "Visual check error",
        1000,
    )?;
    Ok(json!({
        "directiveId": directive_id,
        "error": error,
    }))
}

fn design_tool_result(directive_id: &str, result: &Value) -> ToolResult<Value> {
    let status = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if status == "done" {
        let path = clean_text(
            result
                .get("designProjectPath")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            "Design path",
            1000,
        )?;
        // F-02-013: never surface absolute / escaping paths to agents.
        validate_design_outcome_path(&path)?;
        let registry_id = clean_text(
            result
                .get("registryId")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            "Registry id",
            200,
        )?;
        return Ok(json!({
            "directiveId": directive_id,
            "designProjectPath": path,
            "registryId": registry_id,
        }));
    }
    let error = clean_text(
        result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("design_request failed."),
        "Design error",
        1000,
    )?;
    Ok(json!({
        "directiveId": directive_id,
        "error": error,
    }))
}

fn stamp_terminal_if_needed(
    projects_dir: &Path,
    queue_key: &str,
    directive_id: &str,
    synthesized: &Value,
) {
    let _: ToolResult<()> = with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let list = state
            .as_object_mut()
            .and_then(|o| o.get_mut(queue_key))
            .and_then(|v| v.as_array_mut());
        if let Some(list) = list {
            for d in list.iter_mut() {
                if d.get("id").and_then(|v| v.as_str()) != Some(directive_id) {
                    continue;
                }
                let existing = d.get("result").cloned();
                if existing
                    .as_ref()
                    .and_then(|r| r.as_object())
                    .map(|o| !o.is_empty())
                    .unwrap_or(false)
                {
                    break;
                }
                if let Some(obj) = d.as_object_mut() {
                    let live = obj
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut syn = synthesized.clone();
                    if live == "running" {
                        if let Some(s) = syn.as_object_mut() {
                            s.insert("status".into(), json!("timeout"));
                        }
                    }
                    let st = syn
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("failed")
                        .to_string();
                    obj.insert("status".into(), json!(st));
                    obj.insert("result".into(), syn);
                }
                break;
            }
        }
        write_agents_state(projects_dir, state)?;
        Ok(())
    });
}

pub fn visual_check(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    html_path: &str,
    focus: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "visual_check", session_token)?;
    let html_path = clean_visual_html_path(html_path)?;
    let focus = match focus.map(str::trim).filter(|s| !s.is_empty()) {
        Some(f) => Some(clean_text(f, "Visual check focus", VISUAL_CHECK_MAX_FOCUS_CHARS)?),
        None => None,
    };
    let directive_id = Uuid::new_v4().simple().to_string();
    let created_at = now_rfc3339();
    let mut directive = json!({
        "id": directive_id,
        "parentAgentId": agent_id,
        "status": "pending",
        "htmlPath": html_path,
        "resultPath": format!("{directive_id}.json"),
        "createdAt": created_at,
    });
    if let Some(f) = focus {
        directive
            .as_object_mut()
            .unwrap()
            .insert("focus".into(), json!(f));
    }

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let session = find_session(&state, &agent_id);
        let status = session
            .and_then(|s| s.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if session.is_none() || status.is_empty() || status == "closed" || status == "launch_pending"
        {
            return Err(ToolError::new(
                "visual_check requires a live registered session.",
            ));
        }
        let mut directives = state
            .get("visualCheckDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        directives.push(directive);
        directives = cap_visual_check_directives(directives);
        if let Some(obj) = state.as_object_mut() {
            obj.insert("visualCheckDirectives".into(), json!(directives));
        }
        add_event(
            &mut state,
            &agent_id,
            &role,
            "visual_check",
            "Requested a local visual critique for one HTML artifact.",
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })?;

    let deadline = Instant::now() + Duration::from_secs_f64(visual_poll_timeout());
    let mut seen = false;
    let mut ever_ran = false;
    loop {
        let (present, status, result) =
            directive_result(projects_dir, "visualCheckDirectives", &directive_id);
        if let Some(result) = result {
            return visual_tool_result(&directive_id, &result);
        }
        if present {
            seen = true;
            if status == "running" {
                ever_ran = true;
            }
        } else if seen {
            return Ok(json!({
                "directiveId": directive_id,
                "error": "visual_check directive vanished before producing a result.",
            }));
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_secs_f64(visual_poll_interval()));
    }

    let synthesized = if ever_ran {
        json!({
            "status": "timeout",
            "error": "visual_check timed out waiting for the local critique.",
        })
    } else {
        json!({
            "status": "failed",
            "error": "visual-check executor did not start this request within the poll window.",
        })
    };
    stamp_terminal_if_needed(
        projects_dir,
        "visualCheckDirectives",
        &directive_id,
        &synthesized,
    );
    // Re-read in case executor wrote concurrently
    let (_, _, result) = directive_result(projects_dir, "visualCheckDirectives", &directive_id);
    visual_tool_result(
        &directive_id,
        result.as_ref().unwrap_or(&synthesized),
    )
}

pub fn design_request(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    prompt: &str,
    context: Option<&str>,
    mode: Option<&str>,
    frame: Option<&str>,
    refine_from: Option<&str>,
    refine: Option<bool>,
    wait: Option<bool>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "design_request", session_token)?;
    let prompt = clean_text(prompt, "Design prompt", 4000)?;
    let plan_context = match context.map(str::trim).filter(|s| !s.is_empty()) {
        Some(c) => Some(clean_text(c, "Design context", 4000)?),
        None => None,
    };
    let mode = mode
        .map(str::trim)
        .filter(|m| *m == "static" || *m == "interactive");
    let frame = frame
        .map(str::trim)
        .filter(|f| matches!(*f, "android" | "ios" | "web" | "component"));
    // Iteration (Phase 8): `refine_from` targets a specific registry id, `refine`
    // targets the project's CURRENT design. Both are hints the frontend watcher honors;
    // the registry id is a client-opaque token (clean_text-bounded, no path semantics).
    let refine_from = match refine_from.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => Some(clean_text(id, "Design refine registry id", 256)?),
        None => None,
    };

    let directive_id = Uuid::new_v4().simple().to_string();
    let mut directive = json!({
        "id": directive_id,
        "parentAgentId": agent_id,
        "status": "pending",
        "prompt": prompt,
        "resultPath": format!("{directive_id}.json"),
        "createdAt": now_rfc3339(),
    });
    {
        let obj = directive.as_object_mut().unwrap();
        if let Some(c) = plan_context {
            obj.insert("planContext".into(), json!(c));
        }
        if let Some(m) = mode {
            obj.insert("mode".into(), json!(m));
        }
        if let Some(f) = frame {
            obj.insert("frame".into(), json!(f));
        }
        if let Some(id) = refine_from {
            obj.insert("refineFrom".into(), json!(id));
        } else if refine == Some(true) {
            // Only honor `refine` when no explicit `refineFrom`: refineFrom already
            // pins the base, and the watcher treats a bare `refine:true` as "the
            // project's current design".
            obj.insert("refine".into(), json!(true));
        }
    }

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let session = find_session(&state, &agent_id);
        let status = session
            .and_then(|s| s.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if session.is_none() || status.is_empty() || status == "closed" || status == "launch_pending"
        {
            return Err(ToolError::new(
                "design_request requires a live registered session.",
            ));
        }
        let mut directives = state
            .get("designRequestDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        directives.push(directive);
        directives = cap_design_request_directives(directives);
        if let Some(obj) = state.as_object_mut() {
            obj.insert("designRequestDirectives".into(), json!(directives));
        }
        add_event(
            &mut state,
            &agent_id,
            &role,
            "design_request",
            "Requested a design from the designer AI.",
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })?;

    // wait=false → return the directiveId immediately (poll with design_result). Interactive
    // designs take ~1min to generate; blocking the whole tool call for that long trips the
    // agent's MCP client request timeout (-32001) and LOSES the directiveId, which blocks the
    // iterate loop (the agent can't get the registryId to refine). Mirrors spawn_mini_coder.
    if wait == Some(false) {
        return Ok(json!({
            "directiveId": directive_id,
            "status": "pending",
            "note": "design queued — call design_result with this directiveId to collect the registryId, then design_request(refine_from=<registryId>) to iterate.",
        }));
    }
    // wait=true: block only a bounded grace (well under the client timeout). If the design
    // finishes in time, return the full result; otherwise return a non-error running body so
    // the agent collects the outcome via design_result. NEVER stamp the directive here.
    let deadline = Instant::now() + Duration::from_secs_f64(design_sync_grace());
    if let Some(terminal) = poll_design_terminal(projects_dir, &directive_id, deadline)? {
        return Ok(terminal);
    }
    Ok(finalize_or_running(projects_dir, &directive_id))
}

/// Collect the outcome of a `design_request` by its directiveId (mirrors
/// `mini_coder_result`). `wait=true` (default) blocks a bounded grace then returns the
/// terminal result (`designProjectPath` + `registryId`, or an error) or a non-error
/// `{directiveId, status:"running"}` body to poll again. `wait=false` does a single read.
/// NEVER stamps the directive — the frontend watcher owns completion.
pub fn design_result(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    directive_id: &str,
    wait: Option<bool>,
) -> ToolResult<Value> {
    let (agent_id, _role) =
        require_agent_tool(projects_dir, agent_id, role, "design_result", session_token)?;
    // require_agent_tool does not reject a `closed` session (only launch_pending); mirror the
    // explicit live-session gate design_request applies so a stale-but-tokened session can't read.
    require_live_session(projects_dir, &agent_id, "design_result")?;
    let directive_id = directive_id.trim();
    if directive_id.is_empty() {
        return Err(ToolError::new("design_result requires a directiveId."));
    }
    if directive_id.len() > 128 || directive_id.chars().any(|c| !c.is_ascii_alphanumeric()) {
        return Err(ToolError::new("design_result directiveId is malformed."));
    }
    // Ownership fail-closed (mirrors mini_coder_result HIGH #3): only the agent that queued
    // the directive may collect its result (registryId / designProjectPath). A missing owner
    // or a foreign owner is denied; an unknown id reports not_found (no leak).
    match read_design_directive(projects_dir, directive_id) {
        None => {
            return Ok(json!({ "directiveId": directive_id, "status": "not_found" }));
        }
        Some(dir) => {
            let owner = dir
                .get("parentAgentId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if owner.is_empty() || owner != agent_id {
                return Err(ToolError::new(
                    "design directive is not owned by this agent.",
                ));
            }
        }
    }
    let deadline = if wait == Some(false) {
        Instant::now()
    } else {
        Instant::now() + Duration::from_secs_f64(design_sync_grace())
    };
    if let Some(terminal) = poll_design_terminal(projects_dir, directive_id, deadline)? {
        return Ok(terminal);
    }
    Ok(finalize_or_running(projects_dir, directive_id))
}

// silence unused helper in some builds
#[allow(dead_code)]
fn _require_live(projects_dir: &Path, agent_id: &str) -> ToolResult<()> {
    require_live_session(projects_dir, agent_id, "tool")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::seed_launch_pending;
    use crate::tools::agent_lifecycle::agent_register;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
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

    #[test]
    fn design_outcome_path_f02_013() {
        assert!(validate_design_outcome_path("designs/foo").is_ok());
        assert!(validate_design_outcome_path("/etc/passwd").is_err());
        assert!(validate_design_outcome_path("~/secrets").is_err());
        assert!(validate_design_outcome_path("a/../../b").is_err());
        assert!(validate_design_outcome_path("C:\\Windows\\system32").is_err());
        assert!(validate_design_outcome_path("").is_err());
        assert!(validate_design_outcome_path("a\nb").is_err());
        assert!(validate_design_outcome_path("a\0b").is_err());
        assert!(validate_design_outcome_path("a//b").is_err());
    }

    #[test]
    fn visual_html_path_guards() {
        assert!(clean_visual_html_path("").is_err());
        assert!(clean_visual_html_path("out/x.html").is_ok());
        assert!(clean_visual_html_path("a\nb").is_err());
        let long = "x".repeat(2000);
        assert!(clean_visual_html_path(&long).is_err());
        // Relative-path confinement (absolute / .. / Windows drive).
        assert!(clean_visual_html_path("/etc/passwd").is_err());
        assert!(clean_visual_html_path("~/secrets.html").is_err());
        assert!(clean_visual_html_path("a/../../b.html").is_err());
        assert!(clean_visual_html_path("C:\\Windows\\system32\\x.html").is_err());
        assert!(clean_visual_html_path("out//x.html").is_err());
        assert_eq!(
            clean_visual_html_path("out\\nested\\x.html").unwrap(),
            "out/nested/x.html"
        );
    }

    #[test]
    fn design_rejects_absolute_outcome_on_done() {
        let result = json!({
            "status": "done",
            "designProjectPath": "/etc/passwd",
            "registryId": "r1",
        });
        let err = design_tool_result("d1", &result).unwrap_err();
        assert!(
            err.message.contains("relative") || err.message.contains("path"),
            "{}",
            err.message
        );
    }

    #[test]
    fn visual_check_fail_closed_without_executor() {
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_VISUAL_CHECK_POLL_TIMEOUT_SECS", "0");
        std::env::set_var("DEVBOULE_MCP_VISUAL_CHECK_POLL_INTERVAL_SECS", "0");
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-vis", "coder");
        let out = visual_check(
            &projects,
            "coder-vis",
            "coder",
            Some(&tok),
            "artifacts/demo.html",
            None,
        )
        .unwrap();
        assert_eq!(out.get("error").is_some(), true, "{out}");
        assert!(
            out["error"]
                .as_str()
                .unwrap()
                .contains("did not start")
                || out["error"].as_str().unwrap().contains("timed out"),
            "{out}"
        );
        std::env::remove_var("DEVBOULE_MCP_VISUAL_CHECK_POLL_TIMEOUT_SECS");
        std::env::remove_var("DEVBOULE_MCP_VISUAL_CHECK_POLL_INTERVAL_SECS");
    }

    #[test]
    fn design_request_returns_pending_without_executor() {
        // With no app executor claiming the directive, the bounded grace expires and the
        // tool returns a NON-error {directiveId, status:"pending"} body (never a transport
        // timeout that loses the id). The agent polls design_result to collect the outcome.
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_DESIGN_SYNC_GRACE_SECS", "0");
        std::env::set_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS", "0");
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-des", "orchestrator");
        let out = design_request(
            &projects,
            "orch-des",
            "orchestrator",
            Some(&tok),
            "Design a settings screen",
            None,
            Some("interactive"),
            Some("web"),
            None,
            None,
            None,
        )
        .unwrap();
        assert!(out.get("error").is_none(), "should not error: {out}");
        assert_eq!(out.get("status").and_then(|v| v.as_str()), Some("pending"), "{out}");
        assert!(out.get("directiveId").and_then(|v| v.as_str()).is_some(), "{out}");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_SYNC_GRACE_SECS");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS");
    }

    #[test]
    fn design_request_wait_false_returns_directive_id_immediately() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-nowait", "orchestrator");
        let out = design_request(
            &projects,
            "orch-nowait",
            "orchestrator",
            Some(&tok),
            "A landing page",
            None,
            Some("interactive"),
            None,
            None,
            None,
            Some(false),
        )
        .unwrap();
        assert_eq!(out.get("status").and_then(|v| v.as_str()), Some("pending"), "{out}");
        let id = out.get("directiveId").and_then(|v| v.as_str()).expect("directiveId");
        // The directive is queued and collectable via design_result (single read → running/pending).
        let res = design_result(&projects, "orch-nowait", "orchestrator", Some(&tok), id, Some(false)).unwrap();
        assert!(
            res.get("status").and_then(|v| v.as_str()).is_some(),
            "design_result should report a status: {res}"
        );
    }

    #[test]
    fn design_result_reads_completed_directive() {
        // Simulate the watcher completing the design: stamp a done result on the directive,
        // then design_result must return the registryId (the payload the agent needs to refine).
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-res", "orchestrator");
        let out = design_request(
            &projects, "orch-res", "orchestrator", Some(&tok),
            "A hero", None, Some("interactive"), None, None, None, Some(false),
        )
        .unwrap();
        let id = out["directiveId"].as_str().unwrap().to_string();
        // Frontend watcher completes it: set status=done + result on the directive.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            let list = state
                .as_object_mut()
                .and_then(|o| o.get_mut("designRequestDirectives"))
                .and_then(|v| v.as_array_mut())
                .unwrap();
            for d in list.iter_mut() {
                if d.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
                    let obj = d.as_object_mut().unwrap();
                    obj.insert("status".into(), json!("done"));
                    obj.insert(
                        "result".into(),
                        json!({"status":"done","designProjectPath":".aspis-design/x","registryId":"reg-99"}),
                    );
                }
            }
            write_agents_state(&projects, state)?;
            Ok::<(), ToolError>(())
        })
        .unwrap();
        let res = design_result(&projects, "orch-res", "orchestrator", Some(&tok), &id, Some(false)).unwrap();
        assert_eq!(res.get("registryId").and_then(|v| v.as_str()), Some("reg-99"), "{res}");
        assert_eq!(res.get("designProjectPath").and_then(|v| v.as_str()), Some(".aspis-design/x"), "{res}");
    }

    #[test]
    fn design_result_stamps_timeout_after_abandon_ceiling() {
        // A directive that never completes and has aged past the abandonment ceiling must
        // resolve to a definitive `timeout` (not poll `running` forever).
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-old", "orchestrator");
        let out = design_request(
            &projects, "orch-old", "orchestrator", Some(&tok),
            "A hero", None, Some("interactive"), None, None, None, Some(false),
        )
        .unwrap();
        let id = out["directiveId"].as_str().unwrap().to_string();
        // Backdate createdAt so it is well past the ceiling, and mark it running (claimed but
        // never completed — simulating a hung generation / closed app).
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects)?;
            let list = state
                .as_object_mut()
                .and_then(|o| o.get_mut("designRequestDirectives"))
                .and_then(|v| v.as_array_mut())
                .unwrap();
            for d in list.iter_mut() {
                if d.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
                    let obj = d.as_object_mut().unwrap();
                    obj.insert("createdAt".into(), json!("2020-01-01T00:00:00Z"));
                    obj.insert("status".into(), json!("running"));
                }
            }
            write_agents_state(&projects, state)?;
            Ok::<(), ToolError>(())
        })
        .unwrap();
        // wait=false → grace deadline is now; finalize_or_running sees the aged directive.
        let res = design_result(&projects, "orch-old", "orchestrator", Some(&tok), &id, Some(false)).unwrap();
        assert_eq!(res.get("error").is_some() || res.get("status").and_then(|v| v.as_str()) == Some("timeout") || res.get("status").is_none(), true, "should be terminal: {res}");
        // The directive must now carry a terminal result on disk (no longer stuck running).
        let dir = read_design_directive(&projects, &id).unwrap();
        let st = dir.get("status").and_then(|v| v.as_str()).unwrap_or("");
        assert!(matches!(st, "timeout" | "failed" | "done"), "stamped terminal, got {st}");
    }

    #[test]
    fn design_result_rejects_foreign_owner() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok_a = register(&projects, "orch-a", "orchestrator");
        let tok_b = register(&projects, "orch-b", "orchestrator");
        let out = design_request(
            &projects, "orch-a", "orchestrator", Some(&tok_a),
            "A hero", None, Some("interactive"), None, None, None, Some(false),
        )
        .unwrap();
        let id = out["directiveId"].as_str().unwrap();
        // orch-b must NOT be able to collect orch-a's design result.
        let err = design_result(&projects, "orch-b", "orchestrator", Some(&tok_b), id, Some(false)).unwrap_err();
        assert!(err.message.contains("not owned"), "{}", err.message);
    }

    #[test]
    fn design_result_rejected_for_verifier() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "ver-res", "verifier");
        let err = design_result(&projects, "ver-res", "verifier", Some(&tok), "abc123", Some(false)).unwrap_err();
        assert!(err.message.contains("cannot use"), "{}", err.message);
    }

    #[test]
    fn mini_cannot_design_or_visual() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "mini-dv", "mini");
        let err = visual_check(
            &projects,
            "mini-dv",
            "mini",
            Some(&tok),
            "x.html",
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("cannot use"), "{}", err.message);
        let err = design_request(
            &projects,
            "mini-dv",
            "mini",
            Some(&tok),
            "prompt",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("cannot use"), "{}", err.message);
    }

    #[test]
    fn verifier_cannot_design_request() {
        let _g = env_lock();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "ver-des", "verifier");
        let err = design_request(
            &projects,
            "ver-des",
            "verifier",
            Some(&tok),
            "prompt",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("cannot use"), "{}", err.message);
    }

    #[test]
    fn design_request_directive_carries_refine_from() {
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_DESIGN_SYNC_GRACE_SECS", "0");
        std::env::set_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS", "0");
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-ref", "orchestrator");
        // refine_from wins over refine; the directive must carry `refineFrom` and NOT `refine`.
        let _ = design_request(
            &projects,
            "orch-ref",
            "orchestrator",
            Some(&tok),
            "Make the header blue",
            None,
            Some("interactive"),
            None,
            Some("reg-abc"),
            Some(true),
            None,
        )
        .unwrap();
        let state = read_agents_state(&projects).unwrap();
        let dirs = state
            .get("designRequestDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let d = dirs.last().expect("one directive queued");
        assert_eq!(d.get("refineFrom").and_then(|v| v.as_str()), Some("reg-abc"));
        assert!(d.get("refine").is_none(), "refineFrom must suppress bare refine");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_SYNC_GRACE_SECS");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS");
    }

    #[test]
    fn design_request_directive_carries_bare_refine() {
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_DESIGN_SYNC_GRACE_SECS", "0");
        std::env::set_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS", "0");
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-ref2", "orchestrator");
        let _ = design_request(
            &projects,
            "orch-ref2",
            "orchestrator",
            Some(&tok),
            "Darker theme",
            None,
            None,
            None,
            None,
            Some(true),
            None,
        )
        .unwrap();
        let state = read_agents_state(&projects).unwrap();
        let dirs = state
            .get("designRequestDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let d = dirs.last().expect("one directive queued");
        assert_eq!(d.get("refine").and_then(|v| v.as_bool()), Some(true));
        assert!(d.get("refineFrom").is_none());
        std::env::remove_var("DEVBOULE_MCP_DESIGN_SYNC_GRACE_SECS");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS");
    }
}
