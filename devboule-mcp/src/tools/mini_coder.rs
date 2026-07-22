//! Mini / main coder tools (P4): `spawn_mini_coder`, `steer_mini_coder`,
//! `mini_coder_result`, `spawn_main_coder`.
//!
//! # Architecture
//!
//! The MCP process does **not** spawn LLM workers. It appends directives to
//! `miniCoderDirectives` in `.aspis-agents.json` (same co-owned queue the Tauri
//! `mini_coder_executor` drains). The **app** must be running for a directive
//! to leave `pending`. When the executor never claims a directive within the
//! poll window, the tool returns a synthesized `failed` outcome
//! ("executor did not start…") — fail-closed, not silent success.
//!
//! Pigeon transport (optional Python path when the app publishes a mailbox) is
//! **not** implemented in this Rust port; the file-queue path is the SSoT.
//!
//! # Security (parity with `oracle/server/aspis_mcp.py`)
//!
//! * Role allowlists from `role_rules.json` via `require_agent_tool`.
//! * Session token required for managed sessions.
//! * Path allowlist: project-relative, no `..` / absolute / `-`-leading components.
//! * Co-writer caps: files 64 (mini) / 10 (main + write), task 4000, steer 2000×8.
//! * Parent-child: directive `parentAgentId` = live parent session; steer/result
//!   ownership climbs `parentDirectiveId` to the root owner.
//! * `spawn_main_coder` is orchestrator-only; forces `write` + `agenticIterative`
//!   + `tier:"main"` without requiring a separate `spawn_mini_coder` grant.

use crate::project_file::load_project_locked;
use crate::state::{
    add_event, clean_text, find_session, now_rfc3339, read_agents_state, with_agents_lock,
    write_agents_state, ToolError, ToolResult,
};
use crate::tools::agent_lifecycle::require_agent_tool;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

// ── co-writer caps (MUST match Python aspis_mcp + Rust mini_coder.rs) ───────

/// Queue cap — co-writer parity with `MAX_DIRECTIVES` in mini_coder.rs.
/// Also the hard refuse threshold for non-terminal (active+pending) count.
const MAX_MINI_CODER_DIRECTIVES: usize = 50;
/// Per-parent hard refuse for non-terminal directives (anti-flood).
const MAX_MINI_CODER_NON_TERMINAL_PER_PARENT: usize = 16;
const MINI_CODER_MAX_TASK_LEN: usize = 4000;
const MINI_CODER_MAX_FILES: usize = 64;
/// Main tier + write-directive allowlist cap (main_coder.rs / MAX_MINI_ALLOWLIST_FILES).
const MAIN_CODER_MAX_FILES: usize = 10;
const MINI_CODER_MAX_STEER_LEN: usize = 2000;
const MINI_CODER_MAX_STEER_QUEUE: usize = 8;
const MINI_CODER_STEER_STOP_SENTINEL: &str = "stop";
const MINI_CODER_WRITE_MODE_DEFAULT: &str = "emitEdits";
const MINI_CODER_WRITE_MODES: &[&str] = &["emitEdits", "agenticIterative"];
const MINI_CHAIN_MAX_DEPTH: usize = 256;
/// Absolute max length for a project-relative censor/files path component string.
const MINI_REL_PATH_MAX_LEN: usize = 1024;

/// Default (and hard upper bound) for the wait=true poll window.
/// Override via `DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS` (clamped to this max).
/// Async MCP handlers run the poll inside `tokio::task::spawn_blocking` so the
/// runtime is not blocked for the full window.
const DEFAULT_POLL_TIMEOUT_SECS: f64 = 1800.0;
const DEFAULT_POLL_INTERVAL_SECS: f64 = 0.75;

const MINI_ACTIVE_STATUSES: &[&str] = &["pending", "launching", "running"];
const MINI_TERMINAL_STATUSES: &[&str] = &[
    "done",
    "needs_clarification",
    "aborted_by_human",
    "failed",
    "timeout",
    "escalated",
];

// ── poll timing (env-overridable for tests) ─────────────────────────────────

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
    // Hard-capped at DEFAULT_POLL_TIMEOUT_SECS (1800) so env cannot open an
    // unbounded wait; tests may still set 0 for instant fail-closed.
    env_f64(
        &[
            "DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS",
            "ASPIS_MCP_MINI_CODER_POLL_TIMEOUT_SECS",
        ],
        DEFAULT_POLL_TIMEOUT_SECS,
    )
    .min(DEFAULT_POLL_TIMEOUT_SECS)
}

fn poll_interval_secs() -> f64 {
    env_f64(
        &[
            "DEVBOULE_MCP_MINI_CODER_POLL_INTERVAL_SECS",
            "ASPIS_MCP_MINI_CODER_POLL_INTERVAL_SECS",
        ],
        DEFAULT_POLL_INTERVAL_SECS,
    )
}

// ── path / text helpers ─────────────────────────────────────────────────────

/// Project-relative path guard (Python `validate_censor_rel_path`).
/// Rejects absolute, `..`, `-`-leading components (argv-injection), control
/// characters, and paths longer than `MINI_REL_PATH_MAX_LEN` (1024).
pub fn validate_mini_rel_path(rel: &str) -> ToolResult<String> {
    let text = rel;
    if text.trim().is_empty() {
        return Err(ToolError::new("Censor file path is required."));
    }
    if text.len() > MINI_REL_PATH_MAX_LEN {
        return Err(ToolError::new(format!(
            "Censor rel path must be at most {MINI_REL_PATH_MAX_LEN} characters (got {}).",
            text.len()
        )));
    }
    if text.chars().any(|c| c.is_control()) {
        return Err(ToolError::new(format!(
            "Censor rel path must not contain control characters: {rel:?}"
        )));
    }
    if text.starts_with('/') || text.starts_with('\\') {
        return Err(ToolError::new(format!(
            "Censor rel path must be relative, got absolute: {rel}"
        )));
    }
    if text.len() >= 2 && text.as_bytes()[1] == b':' {
        return Err(ToolError::new(format!(
            "Censor rel path must be relative, got absolute: {rel}"
        )));
    }
    for component in text.split(['\\', '/']) {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(ToolError::new(format!(
                "Censor rel path must not contain '..': {rel}"
            )));
        }
        if component.starts_with('-') {
            return Err(ToolError::new(format!(
                "Censor rel path component must not start with '-': {rel}"
            )));
        }
    }
    Ok(text.to_string())
}

fn is_terminal_status(status: &str) -> bool {
    MINI_TERMINAL_STATUSES
        .iter()
        .any(|s| status.eq_ignore_ascii_case(s))
}

fn is_active_status(status: &str) -> bool {
    MINI_ACTIVE_STATUSES
        .iter()
        .any(|s| status.eq_ignore_ascii_case(s))
}

fn directive_status(d: &Value) -> &str {
    d.get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
}

fn count_non_terminal_directives(directives: &[Value]) -> usize {
    directives
        .iter()
        .filter(|d| !is_terminal_status(directive_status(d)))
        .count()
}

/// Non-terminal directives owned by `parent_agent_id` (direct `parentAgentId`).
fn count_non_terminal_for_parent(directives: &[Value], parent_agent_id: &str) -> usize {
    directives
        .iter()
        .filter(|d| {
            if is_terminal_status(directive_status(d)) {
                return false;
            }
            d.get("parentAgentId").and_then(|v| v.as_str()) == Some(parent_agent_id)
        })
        .count()
}

fn new_directive_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// Keep at most `MAX_MINI_CODER_DIRECTIVES`. Evicts oldest TERMINAL first;
/// prefers `collected: true` terminals (Python F-E). Active never dropped.
pub fn cap_mini_coder_directives(directives: Vec<Value>) -> Vec<Value> {
    let clean: Vec<Value> = directives
        .into_iter()
        .filter(|d| d.is_object())
        .collect();
    if clean.len() <= MAX_MINI_CODER_DIRECTIVES {
        return clean;
    }
    let drop_count = clean.len() - MAX_MINI_CODER_DIRECTIVES;
    let terminal: Vec<&Value> = clean
        .iter()
        .filter(|d| {
            let st = d
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            is_terminal_status(st)
        })
        .collect();
    if drop_count == 0 || terminal.is_empty() {
        return clean;
    }

    let sort_key = |d: &Value| -> (String, String) {
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
    };

    let mut collected: Vec<&Value> = terminal
        .iter()
        .copied()
        .filter(|d| d.get("collected") == Some(&Value::Bool(true)))
        .collect();
    collected.sort_by_key(|d| sort_key(d));
    let mut uncollected: Vec<&Value> = terminal
        .iter()
        .copied()
        .filter(|d| d.get("collected") != Some(&Value::Bool(true)))
        .collect();
    uncollected.sort_by_key(|d| sort_key(d));

    let mut eviction: Vec<&Value> = collected;
    eviction.extend(uncollected);
    let drop_ids: std::collections::HashSet<String> = eviction
        .into_iter()
        .take(drop_count)
        .filter_map(|d| d.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();

    clean
        .into_iter()
        .filter(|d| {
            let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("");
            !drop_ids.contains(id)
        })
        .collect()
}

// ── chain / ownership ───────────────────────────────────────────────────────

/// Climb `parentDirectiveId` to the root (or best ancestor on cycle/dangling).
fn resolve_mini_root_directive<'a>(
    directives: &'a [Value],
    directive_id: &str,
) -> Option<&'a Value> {
    let by_id: std::collections::HashMap<&str, &Value> = directives
        .iter()
        .filter_map(|d| {
            d.get("id")
                .and_then(|v| v.as_str())
                .map(|id| (id, d))
        })
        .collect();
    let mut node = by_id.get(directive_id).copied()?;
    let mut seen = std::collections::HashSet::new();
    let mut depth = 0;
    loop {
        let node_id = node.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if seen.contains(node_id) || depth >= MINI_CHAIN_MAX_DEPTH {
            return Some(node);
        }
        seen.insert(node_id.to_string());
        let parent_id = node
            .get("parentDirectiveId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if parent_id.is_empty() {
            return Some(node);
        }
        match by_id.get(parent_id) {
            Some(parent) => {
                node = parent;
                depth += 1;
            }
            None => return Some(node),
        }
    }
}

fn mini_directive_parent_agent_id(
    projects_dir: &Path,
    directive_id: &str,
) -> ToolResult<Option<String>> {
    with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let directives = state
            .get("miniCoderDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let root = resolve_mini_root_directive(&directives, directive_id);
        Ok(root.and_then(|r| {
            r.get("parentAgentId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        }))
    })
}

/// `(present, status, result)`.
fn mini_directive_result(
    projects_dir: &Path,
    directive_id: &str,
) -> ToolResult<(bool, String, Option<Value>)> {
    with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let directives = state
            .get("miniCoderDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for d in &directives {
            if d.get("id").and_then(|v| v.as_str()) == Some(directive_id) {
                let status = d
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let result = d.get("result").cloned().filter(|r| {
                    r.as_object().map(|o| !o.is_empty()).unwrap_or(false)
                });
                return Ok((true, status, result));
            }
        }
        Ok((false, String::new(), None))
    })
}

fn stamp_mini_directive_collected(projects_dir: &Path, directive_id: &str) {
    let _ = with_agents_lock(projects_dir, || -> ToolResult<()> {
        let mut state = read_agents_state(projects_dir)?;
        if let Some(dirs) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("miniCoderDirectives"))
            .and_then(|v| v.as_array_mut())
        {
            for d in dirs.iter_mut() {
                if d.get("id").and_then(|v| v.as_str()) == Some(directive_id) {
                    if let Some(obj) = d.as_object_mut() {
                        obj.insert("collected".into(), json!(true));
                    }
                    break;
                }
            }
        }
        write_agents_state(projects_dir, state)?;
        Ok(())
    });
}

// ── bounded poll ────────────────────────────────────────────────────────────

fn await_mini_directive(
    projects_dir: &Path,
    directive_id: &str,
    deadline: Instant,
    caller_tool: &str,
) -> ToolResult<Value> {
    let mut seen = false;
    let mut ever_ran = false;
    loop {
        let (present, status, result) = mini_directive_result(projects_dir, directive_id)?;
        if let Some(result) = result {
            return Ok(json!({
                "directiveId": directive_id,
                "result": result,
            }));
        }
        if present {
            seen = true;
            if status == "running" || status == "launching" {
                ever_ran = true;
            }
            // HIGH #4: already terminal without a result payload — return a
            // minimal synthesized outcome and NEVER stamp/overwrite.
            if is_terminal_status(&status) {
                return Ok(json!({
                    "directiveId": directive_id,
                    "result": {
                        "status": status,
                        "error": format!(
                            "mini-coder ended with status '{status}' without a result payload."
                        ),
                    },
                }));
            }
        } else if seen {
            return Ok(json!({
                "directiveId": directive_id,
                "result": {
                    "status": "failed",
                    "error": "mini-coder directive vanished before producing a result.",
                },
            }));
        }
        if Instant::now() >= deadline {
            break;
        }
        let interval = poll_interval_secs();
        if interval > 0.0 {
            // Sync poll: MCP async handlers wrap this in spawn_blocking so the
            // tokio runtime worker is not held for the full wait window.
            thread::sleep(Duration::from_secs_f64(interval));
        }
    }

    let mut synthesized = if ever_ran {
        json!({
            "status": "timeout",
            "error": format!("{caller_tool} poll timed out waiting for the mini result."),
        })
    } else {
        json!({
            "status": "failed",
            "error": "executor did not start this mini within the poll window.",
        })
    };

    // Best-effort stamp under lock (killRequested wins; prefer real result;
    // NEVER overwrite an already-terminal status).
    let _ = with_agents_lock(projects_dir, || -> ToolResult<()> {
        let mut state = read_agents_state(projects_dir)?;
        let mut changed = false;
        if let Some(dirs) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("miniCoderDirectives"))
            .and_then(|v| v.as_array_mut())
        {
            for d in dirs.iter_mut() {
                if d.get("id").and_then(|v| v.as_str()) != Some(directive_id) {
                    continue;
                }
                let live_status = d
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let existing = d.get("result").cloned().filter(|r| {
                    r.as_object().map(|o| !o.is_empty()).unwrap_or(false)
                });
                if let Some(existing) = existing {
                    // Prefer a real result written since our last read.
                    synthesized = existing;
                } else if is_terminal_status(&live_status) {
                    // HIGH #4: already terminal — synthesize for the caller but
                    // do not overwrite status/result on disk.
                    synthesized = json!({
                        "status": live_status,
                        "error": format!(
                            "mini-coder ended with status '{live_status}' without a result payload."
                        ),
                    });
                } else if d.get("killRequested") == Some(&Value::Bool(true)) {
                    synthesized = json!({
                        "status": "aborted_by_human",
                        "error": "stopped by human (Stop button) — do not retry, escalate.",
                    });
                    if let Some(obj) = d.as_object_mut() {
                        obj.insert("status".into(), json!("aborted_by_human"));
                        obj.insert("result".into(), synthesized.clone());
                        changed = true;
                    }
                } else {
                    synthesized = if matches!(
                        live_status.as_str(),
                        "running" | "launching" | "awaiting_retry"
                    ) {
                        json!({
                            "status": "timeout",
                            "error": format!(
                                "{caller_tool} poll timed out (mini still running / retry chain in progress)."
                            ),
                        })
                    } else {
                        json!({
                            "status": "failed",
                            "error": "executor did not start this mini within the poll window.",
                        })
                    };
                    if let Some(obj) = d.as_object_mut() {
                        obj.insert(
                            "status".into(),
                            synthesized
                                .get("status")
                                .cloned()
                                .unwrap_or(json!("failed")),
                        );
                        obj.insert("result".into(), synthesized.clone());
                        changed = true;
                    }
                }
                break;
            }
        }
        if changed {
            write_agents_state(projects_dir, state)?;
        }
        Ok(())
    });

    Ok(json!({
        "directiveId": directive_id,
        "result": synthesized,
    }))
}

// ── draft guard ─────────────────────────────────────────────────────────────

/// Fail-closed draft check against a live session object (call under agents lock).
/// When `currentProjectId` is set and not `"default"`, a load error refuses spawn
/// (cannot prove the project is not draft).
fn reject_if_session_on_draft(projects_dir: &Path, session: Option<&Value>) -> ToolResult<()> {
    let project_id = session
        .and_then(|s| s.get("currentProjectId"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if project_id.is_empty() || project_id == "default" {
        return Ok(());
    }
    match load_project_locked(projects_dir, project_id) {
        Ok(project) if project.metadata.status() == "draft" => Err(ToolError::new(
            "spawn_mini_coder: draft projects are read-only — activate the project first.",
        )),
        Ok(_) => Ok(()),
        Err(e) => Err(ToolError::new(format!(
            "spawn_mini_coder: cannot load project '{project_id}' to verify it is not draft \
             (fail-closed): {}",
            e.message
        ))),
    }
}

// ── spawn ───────────────────────────────────────────────────────────────────

/// Shared spawn implementation. When `preauthorized` is set (main coder path),
/// skips re-checking the `spawn_mini_coder` grant.
fn dispatch_spawn_mini_coder(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    task: &str,
    files: &[String],
    backend: Option<&str>,
    allow_oracle: bool,
    write: Option<bool>,
    write_mode: Option<&str>,
    wait: Option<bool>,
    tier: &str,
    preauthorized: Option<(String, String)>,
    // F07: optional Kanban task (Main path). NO-CHURN: omitted from directive when None.
    task_id: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = if let Some((aid, r)) = preauthorized {
        (aid, r)
    } else {
        require_agent_tool(
            projects_dir,
            agent_id,
            role,
            "spawn_mini_coder",
            session_token,
        )?
    };

    let task = clean_text(task, "Mini-coder task", MINI_CODER_MAX_TASK_LEN)?;
    if files.is_empty() {
        return Err(ToolError::new(
            "spawn_mini_coder requires a non-empty `files` list of project-relative paths.",
        ));
    }
    if files.len() > MINI_CODER_MAX_FILES {
        return Err(ToolError::new(format!(
            "spawn_mini_coder accepts at most {MINI_CODER_MAX_FILES} files."
        )));
    }
    let mut validated_files = Vec::with_capacity(files.len());
    for entry in files {
        validated_files.push(validate_mini_rel_path(entry)?);
    }

    let backend = match backend {
        Some(b) if !b.trim().is_empty() => {
            Some(clean_text(b, "Mini-coder backend", 40)?)
        }
        _ => None,
    };

    // STRICT bool: only explicit `false` skips wait (Python: `is not False`).
    let wait = wait != Some(false);
    let write_true = write == Some(true);

    let directive_id = new_directive_id();
    let created_at = now_rfc3339();

    let mut directive = Map::new();
    directive.insert("id".into(), json!(directive_id));
    directive.insert("parentAgentId".into(), json!(agent_id));
    directive.insert("status".into(), json!("pending"));
    directive.insert("task".into(), json!(task));
    directive.insert("files".into(), json!(validated_files));
    directive.insert("resultPath".into(), json!(format!("{directive_id}.json")));
    directive.insert("createdAt".into(), json!(created_at));

    if let Some(b) = backend {
        directive.insert("backend".into(), json!(b));
    }
    // NO-CHURN: only emit tier when non-default "main".
    if tier == "main" {
        directive.insert("tier".into(), json!("main"));
    }
    // F07: stamp taskId when the Main caller named a Kanban task.
    if let Some(tid) = task_id.map(str::trim).filter(|t| !t.is_empty()) {
        // Soft validate: keep the same shape as project_file::normalize_task_id
        // without hard-failing the whole spawn on a weird id (the finalize
        // promote is a no-op when the task is missing).
        if tid.len() <= 40
            && tid
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && tid
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            directive.insert("taskId".into(), json!(tid));
        }
    }
    if allow_oracle {
        directive.insert("allowOracle".into(), json!(true));
    }
    if write_true {
        if validated_files.len() > MAIN_CODER_MAX_FILES {
            return Err(ToolError::new(format!(
                "Write directives allow at most {MAIN_CODER_MAX_FILES} files in the allowlist \
                 (got {}). Split the task.",
                validated_files.len()
            )));
        }
        directive.insert("write".into(), json!(true));
    }

    let raw_write_mode = write_mode.unwrap_or(MINI_CODER_WRITE_MODE_DEFAULT);
    if !MINI_CODER_WRITE_MODES.contains(&raw_write_mode) {
        return Err(ToolError::new(format!(
            "spawn_mini_coder `write_mode` must be one of {}, \
             (got {raw_write_mode:?}).",
            MINI_CODER_WRITE_MODES.join(", ")
        )));
    }
    if raw_write_mode != MINI_CODER_WRITE_MODE_DEFAULT {
        if !write_true {
            return Err(ToolError::new(
                "spawn_mini_coder `write_mode` is only meaningful on a write \
                 directive — pass `write: true` (write_mode governs HOW the mini writes).",
            ));
        }
        directive.insert("writeMode".into(), json!(raw_write_mode));
    }

    // Append under lock: live parent session + draft re-check + hard non-terminal
    // queue caps (global + per-parent). Cap-evict only drops terminals; pure
    // pending growth is refused here so the queue cannot grow unbounded.
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
                "spawn_mini_coder requires a live parent session; register (and keep it active) before delegating.",
            ));
        }

        // MED #6: draft re-check under the same append lock (fail-closed on load).
        reject_if_session_on_draft(projects_dir, session)?;

        // BLOCKER #1: hard refuse before append so pending cannot grow unbounded
        // (cap_mini_coder_directives only evicts terminals).
        let (global_non_term, parent_non_term) = {
            let existing = state
                .get("miniCoderDirectives")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]);
            (
                count_non_terminal_directives(existing),
                count_non_terminal_for_parent(existing, &agent_id),
            )
        };
        if global_non_term >= MAX_MINI_CODER_DIRECTIVES {
            return Err(ToolError::new(format!(
                "spawn_mini_coder: too many non-terminal mini-coder directives \
                 (global cap {MAX_MINI_CODER_DIRECTIVES}); wait for some to finish."
            )));
        }
        if parent_non_term >= MAX_MINI_CODER_NON_TERMINAL_PER_PARENT {
            return Err(ToolError::new(format!(
                "spawn_mini_coder: too many non-terminal mini-coder directives for this parent \
                 (per-parent cap {MAX_MINI_CODER_NON_TERMINAL_PER_PARENT}); wait for some to finish."
            )));
        }

        let directives = state
            .as_object_mut()
            .and_then(|o| {
                o.entry("miniCoderDirectives".to_string())
                    .or_insert_with(|| json!([]));
                o.get_mut("miniCoderDirectives")
            })
            .and_then(|v| v.as_array_mut())
            .ok_or_else(|| ToolError::new("miniCoderDirectives missing after ensure."))?;
        directives.push(Value::Object(directive));
        let capped = cap_mini_coder_directives(std::mem::take(directives));
        *directives = capped;

        add_event(
            &mut state,
            &agent_id,
            &role,
            "mini_coder_spawn",
            &format!(
                "Delegated a mini-coder sub-task on {} file(s).",
                validated_files.len()
            ),
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })?;

    if !wait {
        // App/executor may be offline — caller supervises via steer/result.
        // Documented fail-closed: if the app never claims, mini_coder_result
        // synthesizes failed/timeout rather than inventing success.
        return Ok(json!({
            "directiveId": directive_id,
            "status": "running",
        }));
    }

    let timeout = poll_timeout_secs();
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    await_mini_directive(projects_dir, &directive_id, deadline, "spawn_mini_coder")
}

// ── public tools ────────────────────────────────────────────────────────────

pub fn spawn_mini_coder(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    task: &str,
    files: &[String],
    backend: Option<&str>,
    allow_oracle: bool,
    write: Option<bool>,
    write_mode: Option<&str>,
    wait: Option<bool>,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    dispatch_spawn_mini_coder(
        projects_dir,
        agent_id,
        role,
        session_token,
        task,
        files,
        backend,
        allow_oracle,
        write,
        write_mode,
        wait,
        "mini",
        None,
        None, // task_id is Main-path only
    )
}

pub fn spawn_main_coder(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    task: &str,
    files: &[String],
    backend: Option<&str>,
    allow_oracle: bool,
    wait: Option<bool>,
    session_token: Option<&str>,
    task_id: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "spawn_main_coder",
        session_token,
    )?;
    if files.len() > MAIN_CODER_MAX_FILES {
        return Err(ToolError::new(format!(
            "spawn_main_coder accepts at most {MAIN_CODER_MAX_FILES} files."
        )));
    }
    // Force write + agenticIterative + tier main; preauthorized so we do not
    // re-require the spawn_mini_coder grant. Placeholder agent/role strings are
    // ignored when preauthorized is set.
    dispatch_spawn_mini_coder(
        projects_dir,
        "",
        "",
        session_token,
        task,
        files,
        backend,
        allow_oracle,
        Some(true),
        Some("agenticIterative"),
        wait,
        "main",
        Some((agent_id, role)),
        task_id,
    )
}

pub fn steer_mini_coder(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    directive_id: &str,
    message: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "steer_mini_coder",
        session_token,
    )?;
    let directive_id = clean_text(directive_id, "Mini-coder directive id", 200)?;
    let message = clean_text(message, "Steer message", MINI_CODER_MAX_STEER_LEN)?;
    let is_stop = message.trim().eq_ignore_ascii_case(MINI_CODER_STEER_STOP_SENTINEL);

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let directives = state
            .get("miniCoderDirectives")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let in_chain_idxs: Vec<usize> = directives
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let parent = d
                    .get("parentDirectiveId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                id == directive_id || parent == directive_id
            })
            .map(|(i, _)| i)
            .collect();

        if in_chain_idxs.is_empty() {
            return Ok(json!({
                "directiveId": directive_id,
                "status": "not_found",
            }));
        }

        // Ownership from TRUE root (climb full list, not just in_chain).
        let root = resolve_mini_root_directive(&directives, &directive_id);
        let root_owner = root
            .and_then(|r| r.get("parentAgentId"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if root_owner != agent_id {
            return Err(ToolError::new(
                "mini-coder directive is not owned by this agent.",
            ));
        }

        let all_terminal = in_chain_idxs.iter().all(|&i| {
            let st = directives[i]
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            is_terminal_status(st)
        });
        if all_terminal {
            return Ok(json!({
                "directiveId": directive_id,
                "status": "terminal",
            }));
        }

        // Prefer active attempt in chain; else the root id match; else first.
        let target_idx = in_chain_idxs
            .iter()
            .copied()
            .find(|&i| {
                let st = directives[i]
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                is_active_status(st)
            })
            .or_else(|| {
                in_chain_idxs.iter().copied().find(|&i| {
                    directives[i].get("id").and_then(|v| v.as_str()) == Some(directive_id.as_str())
                })
            })
            .unwrap_or(in_chain_idxs[0]);

        // Mutate via live state array.
        let dirs = state
            .as_object_mut()
            .and_then(|o| o.get_mut("miniCoderDirectives"))
            .and_then(|v| v.as_array_mut())
            .ok_or_else(|| ToolError::new("miniCoderDirectives missing."))?;

        if is_stop {
            for &i in &in_chain_idxs {
                if i >= dirs.len() {
                    continue;
                }
                let st = dirs[i]
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !is_terminal_status(st) {
                    if let Some(obj) = dirs[i].as_object_mut() {
                        obj.insert("killRequested".into(), json!(true));
                    }
                }
            }
            add_event(
                &mut state,
                &agent_id,
                &role,
                "mini_coder_steer",
                "Sent a STOP steer to a mini-coder (kill path).",
                None,
                None,
                None,
                None,
            )?;
            write_agents_state(projects_dir, state)?;
            return Ok(json!({
                "directiveId": directive_id,
                "status": "stopped",
            }));
        }

        let target = dirs
            .get_mut(target_idx)
            .ok_or_else(|| ToolError::new("steer target index out of range."))?;
        let queue_len = {
            let queue = target
                .as_object_mut()
                .map(|o| {
                    o.entry("steerQueue".to_string())
                        .or_insert_with(|| json!([]));
                    o.get_mut("steerQueue")
                })
                .and_then(|v| v.and_then(|x| x.as_array_mut()));
            let Some(queue) = queue else {
                return Err(ToolError::new("steerQueue missing."));
            };
            if queue.len() >= MINI_CODER_MAX_STEER_QUEUE {
                return Ok(json!({
                    "directiveId": directive_id,
                    "status": "queue_full",
                    "queued": queue.len(),
                }));
            }
            queue.push(json!(message));
            queue.len()
        };

        add_event(
            &mut state,
            &agent_id,
            &role,
            "mini_coder_steer",
            &format!("Queued a steer correction for a mini-coder ({queue_len} pending)."),
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(json!({
            "directiveId": directive_id,
            "status": "queued",
            "queued": queue_len,
        }))
    })
}

pub fn mini_coder_result(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    directive_id: &str,
    wait: Option<bool>,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, _role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "mini_coder_result",
        session_token,
    )?;
    let directive_id = clean_text(directive_id, "Mini-coder directive id", 200)?;
    let wait = wait != Some(false);

    let (present, st, res) = mini_directive_result(projects_dir, &directive_id)?;
    if !present {
        return Ok(json!({
            "directiveId": directive_id,
            "status": "not_found",
        }));
    }
    // HIGH #3: ownership fail-closed — missing/empty parentAgentId is deny
    // (same as steer), and we never stamp collected for a non-owner.
    let owner = mini_directive_parent_agent_id(projects_dir, &directive_id)?;
    match owner {
        Some(ref o) if o == &agent_id => {}
        _ => {
            return Err(ToolError::new(
                "mini-coder directive is not owned by this agent.",
            ));
        }
    }

    if wait {
        if let Some(res) = res {
            stamp_mini_directive_collected(projects_dir, &directive_id);
            return Ok(json!({
                "directiveId": directive_id,
                "result": res,
            }));
        }
        let timeout = poll_timeout_secs();
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        let outcome =
            await_mini_directive(projects_dir, &directive_id, deadline, "mini_coder_result")?;
        stamp_mini_directive_collected(projects_dir, &directive_id);
        return Ok(outcome);
    }

    if let Some(res) = res {
        stamp_mini_directive_collected(projects_dir, &directive_id);
        return Ok(json!({
            "directiveId": directive_id,
            "result": res,
        }));
    }
    // MED #5: return the real directive status, not a hardcoded "running".
    let status = if st.is_empty() { "pending" } else { st.as_str() };
    Ok(json!({
        "directiveId": directive_id,
        "status": status,
    }))
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_file::write_test_project;
    use crate::state::seed_launch_pending;
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
            std::env::remove_var("DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS");
            std::env::remove_var("DEVBOULE_MCP_MINI_CODER_POLL_INTERVAL_SECS");
            std::env::remove_var("ASPIS_MCP_MINI_CODER_POLL_TIMEOUT_SECS");
            std::env::remove_var("ASPIS_MCP_MINI_CODER_POLL_INTERVAL_SECS");
        }
    }

    fn set_poll_timeout_zero() -> PollTimeoutGuard {
        std::env::set_var("DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS", "0");
        std::env::set_var("DEVBOULE_MCP_MINI_CODER_POLL_INTERVAL_SECS", "0");
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

    fn read_state(projects: &Path) -> Value {
        let raw = std::fs::read_to_string(projects.join(".aspis-agents.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn spawn_creates_pending_directive_wait_false() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");

        let out = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "Summarize the module",
            &["src/lib.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap();

        assert_eq!(out["status"], "running");
        let did = out["directiveId"].as_str().unwrap();
        assert_eq!(did.len(), 32);

        let state = read_state(&projects);
        let dirs = state["miniCoderDirectives"].as_array().unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0]["id"], did);
        assert_eq!(dirs[0]["parentAgentId"], "coder-1");
        assert_eq!(dirs[0]["status"], "pending");
        assert_eq!(dirs[0]["task"], "Summarize the module");
        assert_eq!(dirs[0]["files"][0], "src/lib.rs");
        assert_eq!(dirs[0]["resultPath"], format!("{did}.json"));
        // NO-CHURN: default tier/write/writeMode/allowOracle omitted.
        assert!(dirs[0].get("tier").is_none());
        assert!(dirs[0].get("write").is_none());
        assert!(dirs[0].get("writeMode").is_none());
        assert!(dirs[0].get("allowOracle").is_none());
    }

    #[test]
    fn path_outside_project_rejected() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");

        for bad in [
            "../escape.rs",
            "/etc/passwd",
            "a/../../b.rs",
            "-rf.ts",
            "C:\\Windows\\system32",
        ] {
            let err = spawn_mini_coder(
                &projects,
                "coder-1",
                "coder",
                "task",
                &[bad.into()],
                None,
                false,
                None,
                None,
                Some(false),
                Some(&tok),
            )
            .unwrap_err();
            assert!(
                err.message.contains("relative")
                    || err.message.contains("..")
                    || err.message.contains("'-'"),
                "path {bad:?} should fail: {}",
                err.message
            );
        }
    }

    #[test]
    fn caps_enforced_files_and_write() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");

        let many: Vec<String> = (0..65).map(|i| format!("f{i}.rs")).collect();
        let err = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &many,
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("at most 64"),
            "{}",
            err.message
        );

        let write_many: Vec<String> = (0..11).map(|i| format!("w{i}.rs")).collect();
        let err = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &write_many,
            None,
            false,
            Some(true),
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("at most 10"),
            "{}",
            err.message
        );
    }

    #[test]
    fn unauthorized_role_rejected() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "ver-1", "verifier");

        let err = spawn_mini_coder(
            &projects,
            "ver-1",
            "verifier",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("cannot use spawn_mini_coder"),
            "{}",
            err.message
        );

        let tok_c = register(&projects, "coder-1", "coder");
        let err = spawn_main_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            Some(false),
            Some(&tok_c),
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("cannot use spawn_main_coder"),
            "{}",
            err.message
        );
    }

    #[test]
    fn steer_and_result_require_owner() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok_a = register(&projects, "coder-a", "coder");
        let tok_b = register(&projects, "coder-b", "coder");

        let out = spawn_mini_coder(
            &projects,
            "coder-a",
            "coder",
            "task",
            &["src/a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok_a),
        )
        .unwrap();
        let did = out["directiveId"].as_str().unwrap();

        let err = steer_mini_coder(
            &projects,
            "coder-b",
            "coder",
            did,
            "please fix X",
            Some(&tok_b),
        )
        .unwrap_err();
        assert!(
            err.message.contains("not owned"),
            "{}",
            err.message
        );

        let err = mini_coder_result(
            &projects,
            "coder-b",
            "coder",
            did,
            Some(false),
            Some(&tok_b),
        )
        .unwrap_err();
        assert!(
            err.message.contains("not owned"),
            "{}",
            err.message
        );

        // Owner can steer and poll non-blocking.
        let ste = steer_mini_coder(
            &projects,
            "coder-a",
            "coder",
            did,
            "please fix X",
            Some(&tok_a),
        )
        .unwrap();
        assert_eq!(ste["status"], "queued");
        assert_eq!(ste["queued"], 1);

        let res = mini_coder_result(
            &projects,
            "coder-a",
            "coder",
            did,
            Some(false),
            Some(&tok_a),
        )
        .unwrap();
        // wait=false returns the real directive status (pending until claimed).
        assert_eq!(res["status"], "pending");
    }

    #[test]
    fn main_coder_forces_tier_write_and_caps() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "orch-1", "orchestrator");

        let out = spawn_main_coder(
            &projects,
            "orch-1",
            "orchestrator",
            "Implement the feature end-to-end",
            &["src/a.rs".into(), "src/b.rs".into()],
            None,
            false,
            Some(false),
            Some(&tok),
            None,
        )
        .unwrap();
        let did = out["directiveId"].as_str().unwrap();
        let state = read_state(&projects);
        let d = state["miniCoderDirectives"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == did)
            .unwrap();
        assert_eq!(d["tier"], "main");
        assert_eq!(d["write"], true);
        assert_eq!(d["writeMode"], "agenticIterative");
        assert_eq!(d["parentAgentId"], "orch-1");

        let many: Vec<String> = (0..11).map(|i| format!("m{i}.rs")).collect();
        let err = spawn_main_coder(
            &projects,
            "orch-1",
            "orchestrator",
            "too many",
            &many,
            None,
            false,
            Some(false),
            Some(&tok),
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("at most 10"),
            "{}",
            err.message
        );
    }

    #[test]
    fn write_mode_agentic_requires_write_flag() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");

        let err = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            Some("agenticIterative"),
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("write: true") || err.message.contains("write directive"),
            "{}",
            err.message
        );
    }

    #[test]
    fn steer_stop_sets_kill_requested() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");
        let out = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        let did = out["directiveId"].as_str().unwrap();

        let ste = steer_mini_coder(&projects, "coder-1", "coder", did, "STOP", Some(&tok))
            .unwrap();
        assert_eq!(ste["status"], "stopped");

        let state = read_state(&projects);
        let d = &state["miniCoderDirectives"][0];
        assert_eq!(d["killRequested"], true);
        assert!(d.get("steerQueue").is_none() || d["steerQueue"].as_array().unwrap().is_empty());
    }

    #[test]
    fn result_not_found_and_wait_timeout_fail_closed() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");

        let nf = mini_coder_result(
            &projects,
            "coder-1",
            "coder",
            "deadbeefdeadbeefdeadbeefdeadbeef",
            Some(false),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(nf["status"], "not_found");

        // wait=true with zero timeout: executor never started → failed.
        let out = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(true),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(out["result"]["status"], "failed");
        let err = out["result"]["error"].as_str().unwrap();
        assert!(
            err.contains("executor did not start"),
            "{err}"
        );
    }

    #[test]
    fn cap_prefers_collected_terminal() {
        let mut dirs = Vec::new();
        for i in 0..52 {
            let status = if i < 40 { "done" } else { "pending" };
            let collected = i < 5; // first 5 collected
            let mut m = Map::new();
            m.insert("id".into(), json!(format!("d{i:03}")));
            m.insert(
                "createdAt".into(),
                json!(format!("2026-01-01T00:00:{i:02}Z")),
            );
            m.insert("status".into(), json!(status));
            if collected {
                m.insert("collected".into(), json!(true));
            }
            dirs.push(Value::Object(m));
        }
        let capped = cap_mini_coder_directives(dirs);
        assert_eq!(capped.len(), 50);
        // Collected terminals should be gone first.
        for id in ["d000", "d001"] {
            assert!(
                !capped.iter().any(|d| d["id"] == id),
                "expected collected {id} evicted"
            );
        }
        // Pending must survive.
        assert!(capped.iter().any(|d| d["id"] == "d050"));
        assert!(capped.iter().any(|d| d["id"] == "d051"));
    }

    #[test]
    fn draft_project_spawn_rejected() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "drafty",
            "Draft",
            "draft",
            json!([]),
            &[],
        )
        .unwrap();
        let tok = register(&projects, "coder-1", "coder");
        // Bind session to draft project.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(s) = state
                .as_object_mut()
                .and_then(|o| o.get_mut("sessions"))
                .and_then(|v| v.as_array_mut())
                .and_then(|arr| arr.iter_mut().find(|s| s["agentId"] == "coder-1"))
            {
                s.as_object_mut()
                    .unwrap()
                    .insert("currentProjectId".into(), json!("drafty"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();

        let err = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.to_ascii_lowercase().contains("draft"),
            "{}",
            err.message
        );
    }

    #[test]
    fn validate_rel_path_accepts_normal() {
        assert_eq!(
            validate_mini_rel_path("src/app.ts").unwrap(),
            "src/app.ts"
        );
        assert!(validate_mini_rel_path("a/./b.rs").is_ok());
    }

    #[test]
    fn result_stamps_collected_when_terminal() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");
        let out = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        let did = out["directiveId"].as_str().unwrap().to_string();

        // Simulate executor terminal result.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(d) = state
                .as_object_mut()
                .and_then(|o| o.get_mut("miniCoderDirectives"))
                .and_then(|v| v.as_array_mut())
                .and_then(|arr| arr.iter_mut().find(|d| d["id"] == did))
            {
                let obj = d.as_object_mut().unwrap();
                obj.insert("status".into(), json!("done"));
                obj.insert(
                    "result".into(),
                    json!({"status": "done", "summary": "ok"}),
                );
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();

        let res = mini_coder_result(
            &projects,
            "coder-1",
            "coder",
            &did,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(res["result"]["status"], "done");

        let state = read_state(&projects);
        assert_eq!(state["miniCoderDirectives"][0]["collected"], true);
    }

    #[test]
    fn steer_queue_full_refuses() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");
        let out = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        let did = out["directiveId"].as_str().unwrap();

        for i in 0..MINI_CODER_MAX_STEER_QUEUE {
            let ste = steer_mini_coder(
                &projects,
                "coder-1",
                "coder",
                did,
                &format!("msg {i}"),
                Some(&tok),
            )
            .unwrap();
            assert_eq!(ste["status"], "queued");
        }
        let full = steer_mini_coder(
            &projects,
            "coder-1",
            "coder",
            did,
            "one more",
            Some(&tok),
        )
        .unwrap();
        assert_eq!(full["status"], "queue_full");
        assert_eq!(full["queued"], MINI_CODER_MAX_STEER_QUEUE);
    }

    #[test]
    fn spawn_refuses_per_parent_non_terminal_cap() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");

        for i in 0..MAX_MINI_CODER_NON_TERMINAL_PER_PARENT {
            spawn_mini_coder(
                &projects,
                "coder-1",
                "coder",
                &format!("task {i}"),
                &[format!("f{i}.rs")],
                None,
                false,
                None,
                None,
                Some(false),
                Some(&tok),
            )
            .unwrap();
        }
        let err = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "one more",
            &["overflow.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("per-parent") || err.message.contains("too many"),
            "{}",
            err.message
        );
        let state = read_state(&projects);
        let dirs = state["miniCoderDirectives"].as_array().unwrap();
        let pending = dirs
            .iter()
            .filter(|d| d["parentAgentId"] == "coder-1" && d["status"] == "pending")
            .count();
        assert_eq!(pending, MAX_MINI_CODER_NON_TERMINAL_PER_PARENT);
    }

    #[test]
    fn result_denies_missing_parent_agent_id() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");
        // Inject an orphan directive with empty parentAgentId.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            let dirs = state
                .as_object_mut()
                .unwrap()
                .entry("miniCoderDirectives".to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap();
            dirs.push(json!({
                "id": "orphanorphanorphanorphanorphanoo",
                "parentAgentId": "",
                "status": "pending",
                "task": "x",
                "files": ["a.rs"],
                "createdAt": "2026-01-01T00:00:00Z",
            }));
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();

        let err = mini_coder_result(
            &projects,
            "coder-1",
            "coder",
            "orphanorphanorphanorphanorphanoo",
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("not owned"),
            "{}",
            err.message
        );
        // Must not stamp collected on deny.
        let state = read_state(&projects);
        let d = state["miniCoderDirectives"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == "orphanorphanorphanorphanorphanoo")
            .unwrap();
        assert!(d.get("collected").is_none() || d["collected"] != true);
    }

    #[test]
    fn await_does_not_stamp_over_terminal_status() {
        let _g = env_lock();
        set_unmanaged(false);
        let _to = set_poll_timeout_zero();
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");
        let out = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        let did = out["directiveId"].as_str().unwrap().to_string();

        // Mark terminal without a result payload.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(d) = state
                .as_object_mut()
                .and_then(|o| o.get_mut("miniCoderDirectives"))
                .and_then(|v| v.as_array_mut())
                .and_then(|arr| arr.iter_mut().find(|d| d["id"] == did))
            {
                d.as_object_mut()
                    .unwrap()
                    .insert("status".into(), json!("failed"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();

        let res = mini_coder_result(
            &projects,
            "coder-1",
            "coder",
            &did,
            Some(true),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(res["result"]["status"], "failed");

        let state = read_state(&projects);
        let d = &state["miniCoderDirectives"][0];
        // Status must remain the original terminal value (not timeout overwrite).
        assert_eq!(d["status"], "failed");
        // No synthesized timeout result stamped over it.
        assert!(
            d.get("result").is_none()
                || d["result"]["status"] != "timeout",
            "must not stamp timeout over terminal: {:?}",
            d.get("result")
        );
    }

    #[test]
    fn wait_false_returns_actual_status() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");
        let out = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        let did = out["directiveId"].as_str().unwrap().to_string();

        let res = mini_coder_result(
            &projects,
            "coder-1",
            "coder",
            &did,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(res["status"], "pending");

        // Flip to launching and re-read.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(d) = state
                .as_object_mut()
                .and_then(|o| o.get_mut("miniCoderDirectives"))
                .and_then(|v| v.as_array_mut())
                .and_then(|arr| arr.iter_mut().find(|d| d["id"] == did))
            {
                d.as_object_mut()
                    .unwrap()
                    .insert("status".into(), json!("launching"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();

        let res = mini_coder_result(
            &projects,
            "coder-1",
            "coder",
            &did,
            Some(false),
            Some(&tok),
        )
        .unwrap();
        assert_eq!(res["status"], "launching");
    }

    #[test]
    fn path_rejects_control_chars_and_overlong() {
        assert!(validate_mini_rel_path("src/\nlib.rs").is_err());
        assert!(validate_mini_rel_path("src/\x00x.rs").is_err());
        let long = "a".repeat(MINI_REL_PATH_MAX_LEN + 1);
        let err = validate_mini_rel_path(&long).unwrap_err();
        assert!(
            err.message.contains("at most") || err.message.contains("1024"),
            "{}",
            err.message
        );
        let ok = "a".repeat(MINI_REL_PATH_MAX_LEN);
        assert!(validate_mini_rel_path(&ok).is_ok());
    }

    #[test]
    fn draft_load_error_fail_closed() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-1", "coder");
        // Bind session to a project id that does not exist on disk.
        with_agents_lock(&projects, || {
            let mut state = read_agents_state(&projects).unwrap();
            if let Some(s) = state
                .as_object_mut()
                .and_then(|o| o.get_mut("sessions"))
                .and_then(|v| v.as_array_mut())
                .and_then(|arr| arr.iter_mut().find(|s| s["agentId"] == "coder-1"))
            {
                s.as_object_mut()
                    .unwrap()
                    .insert("currentProjectId".into(), json!("missing-proj"));
            }
            write_agents_state(&projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();

        let err = spawn_mini_coder(
            &projects,
            "coder-1",
            "coder",
            "task",
            &["a.rs".into()],
            None,
            false,
            None,
            None,
            Some(false),
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("cannot load project")
                || err.message.contains("fail-closed")
                || err.message.contains("missing-proj"),
            "{}",
            err.message
        );
    }

    #[test]
    fn poll_timeout_clamped_to_default_max() {
        let _g = env_lock();
        std::env::set_var("DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS", "99999");
        assert!((poll_timeout_secs() - DEFAULT_POLL_TIMEOUT_SECS).abs() < f64::EPSILON);
        std::env::set_var("DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS", "0");
        assert!((poll_timeout_secs() - 0.0).abs() < f64::EPSILON);
        std::env::remove_var("DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS");
        std::env::remove_var("ASPIS_MCP_MINI_CODER_POLL_TIMEOUT_SECS");
    }
}
