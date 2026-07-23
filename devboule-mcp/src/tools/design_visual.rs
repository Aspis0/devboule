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
//! * visual html_path: control-char / length bounds.
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

fn design_poll_timeout() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS",
            "ASPIS_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS",
        ],
        DESIGN_REQUEST_POLL_TIMEOUT_SECS,
    )
    .min(DESIGN_REQUEST_POLL_TIMEOUT_SECS)
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
    Ok(text.replace('\\', "/"))
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

    let deadline = Instant::now() + Duration::from_secs_f64(design_poll_timeout());
    let mut seen = false;
    let mut ever_ran = false;
    loop {
        let (present, status, result) =
            directive_result(projects_dir, "designRequestDirectives", &directive_id);
        if let Some(result) = result {
            return design_tool_result(&directive_id, &result);
        }
        if present {
            seen = true;
            if status == "running" {
                ever_ran = true;
            }
        } else if seen {
            return Ok(json!({
                "directiveId": directive_id,
                "error": "directive vanished",
            }));
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_secs_f64(design_poll_interval()));
    }

    let synthesized = if ever_ran {
        json!({
            "status": "timeout",
            "error": "design_request timed out waiting for the designer.",
        })
    } else {
        json!({
            "status": "failed",
            "error": "design executor did not start this request within the poll window.",
        })
    };
    stamp_terminal_if_needed(
        projects_dir,
        "designRequestDirectives",
        &directive_id,
        &synthesized,
    );
    let (_, _, result) = directive_result(projects_dir, "designRequestDirectives", &directive_id);
    design_tool_result(
        &directive_id,
        result.as_ref().unwrap_or(&synthesized),
    )
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
    fn design_request_fail_closed_without_executor() {
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS", "0");
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
        )
        .unwrap();
        assert!(out.get("error").is_some(), "{out}");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS");
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
        )
        .unwrap_err();
        assert!(err.message.contains("cannot use"), "{}", err.message);
    }

    #[test]
    fn design_request_directive_carries_refine_from() {
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS", "0");
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
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS");
    }

    #[test]
    fn design_request_directive_carries_bare_refine() {
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS", "0");
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
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_TIMEOUT_SECS");
        std::env::remove_var("DEVBOULE_MCP_DESIGN_REQUEST_POLL_INTERVAL_SECS");
    }
}
