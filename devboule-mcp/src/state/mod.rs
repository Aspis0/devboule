//! Agents state file co-owned with the Tauri app (`{projects_dir}/.aspis-agents.json`).
//!
//! Value-based RMW so unknown keys from Python/app round-trip without stripping.

mod lock;
mod paths;
mod tokens;

pub use lock::{with_agents_lock, with_file_lock, AgentStateError};
pub use paths::{agents_state_path, resolve_projects_dir, AGENTS_STATE_FILE};
pub use tokens::{
    generate_session_token, hash_launch_token, hash_session_token,
    unmanaged_privileged_agents_allowed, LAUNCH_TOKEN_WINDOW, SESSION_TOKEN_WINDOW,
};

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const AGENTS_STATE_VERSION: u32 = 2;
const MAX_EVENTS: usize = 300;
const MAX_SESSIONS: usize = 200;

const VALID_ROLES: &[&str] = &["coder", "verifier", "mini", "orchestrator"];

/// Tool-level error (maps to MCP error text).
#[derive(Debug, Clone)]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<AgentStateError> for ToolError {
    fn from(e: AgentStateError) -> Self {
        ToolError::new(e.0)
    }
}

pub type ToolResult<T> = Result<T, ToolError>;

// ── time / ids ──────────────────────────────────────────────────────────────

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn event_id() -> String {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("E{ns}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

pub fn parse_iso_timestamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    let normalized = if raw.ends_with('Z') {
        format!("{}+00:00", &raw[..raw.len() - 1])
    } else {
        raw.to_string()
    };
    DateTime::parse_from_rfc3339(&normalized)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

// ── validation ──────────────────────────────────────────────────────────────

pub fn normalize_agent_id(value: &str) -> ToolResult<String> {
    let agent_id = value.trim();
    if agent_id.is_empty() {
        return Err(ToolError::new("Agent id is required."));
    }
    if agent_id.len() > 64
        || !agent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(ToolError::new(
            "Agent id must use only letters, numbers, '.', '_' or '-' and be 1-64 characters.",
        ));
    }
    Ok(agent_id.to_string())
}

pub fn normalize_role(value: &str) -> ToolResult<String> {
    let mut role = value.trim().to_ascii_lowercase();
    role = match role.as_str() {
        "architect" | "code" => "coder".to_string(),
        other => other.to_string(),
    };
    if !VALID_ROLES.contains(&role.as_str()) {
        return Err(ToolError::new(format!(
            "Role must be one of: {} (aliases: architect, code).",
            VALID_ROLES.join(", ")
        )));
    }
    Ok(role)
}

/// Non-raising role normalize for STORED data (unknown -> coder).
pub fn coerce_role(value: &str) -> String {
    let mut role = value.trim().to_ascii_lowercase();
    role = match role.as_str() {
        "architect" | "code" => "coder".to_string(),
        other => other.to_string(),
    };
    if VALID_ROLES.contains(&role.as_str()) {
        role
    } else {
        "coder".to_string()
    }
}

fn roles_same_canonical(a: &str, b: &str) -> bool {
    coerce_role(a) == coerce_role(b)
}

const MODEL_MAX_LEN: usize = 64;
const MODEL_FAMILIES: &[&str] = &["opus", "sonnet", "haiku"];

pub fn normalize_model(value: Option<&str>) -> String {
    let Some(v) = value else {
        return String::new();
    };
    let cleaned: String = v.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim().to_ascii_lowercase();
    if cleaned.is_empty() {
        return String::new();
    }
    let cleaned: String = cleaned.chars().take(MODEL_MAX_LEN).collect();
    for family in MODEL_FAMILIES {
        if cleaned.contains(family) {
            return (*family).to_string();
        }
    }
    cleaned
}

/// Collapse whitespace, require non-empty, cap length (Python `clean_text`).
pub fn clean_text(value: &str, label: &str, limit: usize) -> ToolResult<String> {
    let text: String = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim();
    if text.is_empty() {
        return Err(ToolError::new(format!("{label} is required.")));
    }
    Ok(text.chars().take(limit).collect())
}

fn normalize_current_file_path(value: &str) -> ToolResult<Option<String>> {
    let text = value.trim();
    if text.is_empty() {
        return Ok(None);
    }
    // Match Python: reject C0 control characters (ord < 32).
    if text.chars().any(|ch| (ch as u32) < 32) {
        return Err(ToolError::new(
            "Current file path contains control characters.",
        ));
    }
    if text.len() > 1024 {
        return Err(ToolError::new("Current file path exceeds 1024 characters."));
    }
    let mut normalized = text.replace('\\', "/");
    while normalized.starts_with("./") {
        normalized = normalized[2..].to_string();
    }
    if normalized.is_empty() {
        Ok(None)
    } else {
        Ok(Some(normalized))
    }
}

const NEEDS_USER_STATUS: &str = "needs_user";
const NEEDS_USER_ALIASES: &[&str] = &["awaiting_user", "blocked_on_user"];

/// Statuses only the app may write (seed launch, close session). Agents cannot
/// self-set these via register/heartbeat/`upsert_session`.
const RESERVED_AGENT_STATUSES: &[&str] = &["launch_pending", "closed"];

fn normalize_agent_status(value: &str) -> String {
    let cleaned: String = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        return String::new();
    }
    if cleaned == NEEDS_USER_STATUS || NEEDS_USER_ALIASES.contains(&cleaned.as_str()) {
        return NEEDS_USER_STATUS.to_string();
    }
    cleaned
}

fn reject_reserved_agent_status(status: &str) -> ToolResult<()> {
    let lc = status.trim().to_ascii_lowercase();
    if RESERVED_AGENT_STATUSES.contains(&lc.as_str()) {
        return Err(ToolError::new(format!(
            "Status '{lc}' is reserved for the Devboule app and cannot be set by agents."
        )));
    }
    Ok(())
}

// ── subagents (Python `normalize_subagents` 1:1) ────────────────────────────

const SUBAGENT_LABEL_MAX_LEN: usize = 80;
const SUBAGENT_COUNT_MIN: i64 = 1;
const SUBAGENT_COUNT_MAX: i64 = 9999;
const SUBAGENTS_MAX: usize = 32;

/// Coerce a subagent count to an int in [1, 9999] or None when invalid.
///
/// Accepts int, a clean float (integral value), or a clean numeric string.
/// Out-of-range values are clamped (above max) or rejected (below min);
/// non-numeric / fractional / bool values are rejected.
fn coerce_subagent_count(value: &Value) -> Option<i64> {
    let number = match value {
        Value::Bool(_) => return None,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i
            } else if let Some(u) = n.as_u64() {
                if u > i64::MAX as u64 {
                    return Some(SUBAGENT_COUNT_MAX);
                }
                u as i64
            } else if let Some(f) = n.as_f64() {
                if !f.is_finite() || f.fract() != 0.0 {
                    return None;
                }
                // Safe cast for integral floats in i64 range; clamp extremes.
                if f > i64::MAX as f64 {
                    return Some(SUBAGENT_COUNT_MAX);
                }
                if f < i64::MIN as f64 {
                    return None;
                }
                f as i64
            } else {
                return None;
            }
        }
        Value::String(s) => {
            let text = s.trim();
            if text.is_empty() {
                return None;
            }
            // Match Python `re.fullmatch(r"-?\d+", text)`.
            let bytes = text.as_bytes();
            let digits = if bytes[0] == b'-' { &bytes[1..] } else { bytes };
            if digits.is_empty() || !digits.iter().all(|b| b.is_ascii_digit()) {
                return None;
            }
            match text.parse::<i64>() {
                Ok(n) => n,
                Err(_) => {
                    // Overflow: positive huge → clamp; negative huge → reject.
                    if text.starts_with('-') {
                        return None;
                    }
                    return Some(SUBAGENT_COUNT_MAX);
                }
            }
        }
        _ => return None,
    };
    if number < SUBAGENT_COUNT_MIN {
        return None;
    }
    Some(number.min(SUBAGENT_COUNT_MAX))
}

/// Normalize a self-reported subagent breakdown (Python parity).
///
/// Returns `None` when `value` is not an array ("not provided" — leave stored
/// value untouched). An empty array is valid and means "no subagents now".
/// Invalid entries are dropped; the list is capped at 32.
pub fn normalize_subagents(value: &Value) -> Option<Vec<Value>> {
    let arr = value.as_array()?;
    let mut result: Vec<Value> = Vec::new();
    for entry in arr {
        let obj = match entry.as_object() {
            Some(o) => o,
            None => continue,
        };
        let label_raw = obj.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let label: String = label_raw
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
        if label.is_empty() {
            continue;
        }
        let label: String = label.chars().take(SUBAGENT_LABEL_MAX_LEN).collect();
        let raw_count = match obj.get("count") {
            None | Some(Value::Null) => json!(1),
            Some(v) => v.clone(),
        };
        let Some(count) = coerce_subagent_count(&raw_count) else {
            continue;
        };
        // Optional role: invalid / blank → null (Python sets role=None).
        let role: Value = match obj.get("role") {
            Some(r) if !r.is_null() => {
                let role_str = match r {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    other => other.to_string(),
                };
                if role_str.trim().is_empty() {
                    Value::Null
                } else {
                    match normalize_role(&role_str) {
                        Ok(norm) => json!(norm),
                        Err(_) => Value::Null,
                    }
                }
            }
            _ => Value::Null,
        };
        // Python `normalize_model`: non-str → "".
        let model = match obj.get("model") {
            Some(Value::String(s)) => normalize_model(Some(s.as_str())),
            _ => normalize_model(None),
        };
        result.push(json!({
            "label": label,
            "model": model,
            "count": count,
            "role": role,
        }));
        if result.len() >= SUBAGENTS_MAX {
            break;
        }
    }
    Some(result)
}

// ── state file RMW ──────────────────────────────────────────────────────────

pub fn default_agents_state() -> Value {
    json!({
        "version": AGENTS_STATE_VERSION,
        "updatedAt": now_rfc3339(),
        "sessions": [],
        "claims": [],
        "events": [],
        "miniCoderDirectives": [],
    })
}

pub fn read_agents_state(projects_dir: &Path) -> ToolResult<Value> {
    let path = agents_state_path(projects_dir);
    if !path.exists() {
        return Ok(default_agents_state());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| {
        ToolError::new(format!("Could not read agent state file: {e}"))
    })?;
    let mut state: Value = serde_json::from_str(&content).map_err(|e| {
        ToolError::new(format!("Agents state is invalid JSON: {e}"))
    })?;
    normalize_agents_state(&mut state);
    // P1: skip deep claim-md reconcile against project markdown (Python
    // `reconcile_agents_state_with_projects`). agent_state still returns
    // sessions/claims/events after secret scrub. Full reconcile is P2+.
    Ok(state)
}

pub fn write_agents_state(projects_dir: &Path, mut state: Value) -> ToolResult<Value> {
    normalize_agents_state(&mut state);
    let ts = now_rfc3339();
    if let Some(obj) = state.as_object_mut() {
        obj.insert("updatedAt".into(), json!(ts));
        if let Some(events) = obj.get_mut("events").and_then(|e| e.as_array_mut()) {
            if events.len() > MAX_EVENTS {
                let drop_n = events.len() - MAX_EVENTS;
                events.drain(0..drop_n);
            }
        }
    }
    let path = agents_state_path(projects_dir);
    let content = serde_json::to_string_pretty(&state).map_err(|e| {
        ToolError::new(format!("Could not serialize agent state: {e}"))
    })?;
    write_text_crash_safe(&path, &content, "agent state file")?;
    Ok(state)
}

/// Crash-safe write: temp → flush+fsync → atomic replace (os.replace / MoveFileExW).
///
/// Matches Python `write_text_crash_safe` + app `fs_replace.rs` Windows semantics
/// (`std::fs::rename` does NOT replace existing files on Windows).
/// Temp/bak paths append `.{pid}-{ns}.tmp` to the full filename so `.md` and
/// `.json` targets keep their real suffix (Python `path.with_suffix(suffix+".tmp")`).
pub fn write_text_crash_safe(path: &Path, content: &str, label: &str) -> ToolResult<()> {
    let pid = std::process::id();
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let temp_path = path.with_file_name(format!("{file_name}.{pid}-{ns}.tmp"));
    let backup_path = path.with_file_name(format!("{file_name}.{pid}-{ns}.bak"));
    let write_result = (|| -> std::io::Result<()> {
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temp_path)?;
            file.write_all(content.as_bytes())?;
            file.flush()?;
            // fsync file data (Unix: fsync; Windows: FlushFileBuffers).
            file.sync_all()?;
        }
        if path.exists() {
            let _ = std::fs::copy(path, &backup_path);
        }
        replace_existing(&temp_path, path)?;
        if backup_path.exists() {
            let _ = std::fs::remove_file(&backup_path);
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        if backup_path.exists() && !path.exists() {
            let _ = std::fs::copy(&backup_path, path);
        }
        // Best-effort: drop leftover bak after restore attempt.
        let _ = std::fs::remove_file(&backup_path);
        return Err(ToolError::new(format!("Could not save {label}: {e}")));
    }
    Ok(())
}

/// Atomic replace of `target` with `temp` (Python `os.replace` / app MoveFileExW).
#[cfg(windows)]
fn replace_existing(temp_path: &Path, target_path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(
            lp_existing_file_name: *const u16,
            lp_new_file_name: *const u16,
            dw_flags: u32,
        ) -> i32;
    }

    // MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    let source: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = target_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_existing(temp_path: &Path, target_path: &Path) -> std::io::Result<()> {
    std::fs::rename(temp_path, target_path)
}

fn normalize_agents_state(state: &mut Value) {
    let obj = match state.as_object_mut() {
        Some(o) => o,
        None => {
            *state = default_agents_state();
            return;
        }
    };
    match obj.get("version") {
        Some(Value::Number(n)) if n.as_u64().is_some() => {
            if n.as_u64().unwrap() < AGENTS_STATE_VERSION as u64 {
                obj.insert("version".into(), json!(AGENTS_STATE_VERSION));
            }
        }
        _ => {
            obj.insert("version".into(), json!(AGENTS_STATE_VERSION));
        }
    }
    obj.entry("updatedAt".to_string())
        .or_insert_with(|| json!(now_rfc3339()));
    obj.entry("sessions".to_string())
        .or_insert_with(|| json!([]));
    obj.entry("claims".to_string()).or_insert_with(|| json!([]));
    obj.entry("events".to_string()).or_insert_with(|| json!([]));

    if let Some(sessions) = obj.get_mut("sessions").and_then(|s| s.as_array_mut()) {
        for session in sessions.iter_mut() {
            if let Some(s) = session.as_object_mut() {
                s.entry("subagents".to_string())
                    .or_insert_with(|| json!([]));
                if !s.contains_key("needsUser") {
                    s.insert("needsUser".into(), Value::Null);
                }
                let stored = s
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                let role = if VALID_ROLES.contains(&stored.as_str())
                    || matches!(stored.as_str(), "architect" | "code")
                {
                    stored
                } else {
                    "coder".to_string()
                };
                s.insert("role".into(), json!(role));
            }
        }
        if sessions.len() > MAX_SESSIONS {
            let mut closed_idx: Vec<usize> = sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.get("status")
                        .and_then(|v| v.as_str())
                        .map(|st| st.eq_ignore_ascii_case("closed"))
                        .unwrap_or(false)
                })
                .map(|(i, _)| i)
                .collect();
            closed_idx.sort_by(|&a, &b| {
                let ka = sessions[a]
                    .get("lastSeenAt")
                    .or_else(|| sessions[a].get("firstSeenAt"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let kb = sessions[b]
                    .get("lastSeenAt")
                    .or_else(|| sessions[b].get("firstSeenAt"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                ka.cmp(kb)
            });
            let drop_count = sessions.len() - MAX_SESSIONS;
            let to_drop: std::collections::HashSet<usize> =
                closed_idx.into_iter().take(drop_count).collect();
            let mut i = 0;
            sessions.retain(|_| {
                let keep = !to_drop.contains(&i);
                i += 1;
                keep
            });
        }
    }
}

// ── secrets scrub / acks ────────────────────────────────────────────────────

const SECRET_SESSION_KEYS: &[&str] = &[
    "launchTokenHash",
    "launchTokenIssuedAt",
    "sessionTokenHash",
    "sessionTokenIssuedAt",
    "launchConsumedAt",
];

fn scrub_session_secrets(session: &mut Value) {
    if let Some(obj) = session.as_object_mut() {
        for key in SECRET_SESSION_KEYS {
            obj.remove(*key);
        }
    }
}

/// Deep-clone state and scrub token fields from every session.
pub fn public_agents_state(state: &Value) -> Value {
    let mut public = state.clone();
    if let Some(sessions) = public
        .as_object_mut()
        .and_then(|o| o.get_mut("sessions"))
        .and_then(|s| s.as_array_mut())
    {
        for session in sessions.iter_mut() {
            scrub_session_secrets(session);
        }
    }
    public
}

/// Compact register/heartbeat ack (own session + fleet summary). Never dumps fleet.
pub fn compact_session_ack(
    state: &Value,
    agent_id: &str,
    session_token: Option<&str>,
) -> Value {
    let public = state.clone();
    let sessions = public
        .get("sessions")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let mut own: Option<Value> = sessions
        .iter()
        .find(|s| s.get("agentId").and_then(|v| v.as_str()) == Some(agent_id))
        .cloned();
    if let Some(ref mut session) = own {
        scrub_session_secrets(session);
    }
    let active = sessions
        .iter()
        .filter(|s| {
            s.get("status")
                .and_then(|v| v.as_str())
                .map(|st| st.eq_ignore_ascii_case("active"))
                .unwrap_or(false)
        })
        .count();
    let mut ack = json!({
        "version": public.get("version"),
        "updatedAt": public.get("updatedAt"),
        "session": own,
        "fleet": { "sessions": sessions.len(), "active": active },
        "note": (
            "Full fleet state available via agent_state. \
             Heartbeat acks do not echo sessionToken -- keep using the token \
             from agent_register; a missing token in the ack is NOT an error."
        ),
    });
    if let Some(token) = session_token {
        if !token.is_empty() {
            ack.as_object_mut()
                .unwrap()
                .insert("sessionToken".into(), json!(token));
        }
    }
    ack
}

// ── session mutation ────────────────────────────────────────────────────────

pub fn find_session_mut<'a>(
    state: &'a mut Value,
    agent_id: &str,
) -> Option<&'a mut Map<String, Value>> {
    let sessions = state
        .as_object_mut()?
        .get_mut("sessions")?
        .as_array_mut()?;
    for session in sessions.iter_mut() {
        if session.get("agentId").and_then(|v| v.as_str()) == Some(agent_id) {
            return session.as_object_mut();
        }
    }
    None
}

pub fn find_session<'a>(state: &'a Value, agent_id: &str) -> Option<&'a Value> {
    state
        .get("sessions")?
        .as_array()?
        .iter()
        .find(|s| s.get("agentId").and_then(|v| v.as_str()) == Some(agent_id))
}

pub fn add_event(
    state: &mut Value,
    agent_id: &str,
    role: &str,
    event_type: &str,
    message: &str,
    project_id: Option<&str>,
    task_id: Option<&str>,
    status: Option<&str>,
    evidence: Option<&str>,
) -> ToolResult<()> {
    let msg = clean_text(message, "Event message", 1000)?;
    let evidence_val = match evidence {
        Some(e) if !e.trim().is_empty() => json!(clean_text(e, "Evidence", 2000)?),
        _ => Value::Null,
    };
    let event = json!({
        "id": event_id(),
        "timestamp": now_rfc3339(),
        "agentId": agent_id,
        "role": role,
        "eventType": event_type,
        "projectId": project_id,
        "taskId": task_id,
        "status": status,
        "message": msg,
        "evidence": evidence_val,
    });
    if let Some(events) = state
        .as_object_mut()
        .and_then(|o| o.get_mut("events"))
        .and_then(|e| e.as_array_mut())
    {
        events.push(event);
    }
    Ok(())
}

pub fn upsert_session(
    state: &mut Value,
    agent_id: &str,
    role: &str,
    model: Option<&str>,
    status: &str,
    message: Option<&str>,
    client: Option<&str>,
    file_path: Option<&str>,
    subagents: Option<Value>,
    project_id: Option<&str>,
    task_id: Option<&str>,
) -> ToolResult<()> {
    let clean_agent_id = normalize_agent_id(agent_id)?;
    let raw_status = clean_text(status, "Agent status", 80)?;
    let clean_status = {
        let n = normalize_agent_status(&raw_status);
        if n.is_empty() {
            raw_status.clone()
        } else {
            n
        }
    };
    // App-only lifecycle statuses cannot be self-attested by agents.
    reject_reserved_agent_status(&clean_status)?;
    let normalized_file_path = match file_path {
        Some(fp) if !fp.trim().is_empty() => normalize_current_file_path(fp)?,
        _ => None,
    };
    let normalized_model = normalize_model(model);
    // `subagents`: None / non-array → leave stored untouched; [] clears;
    // valid list replaces. Matches Python `_UNSET` / normalize_subagents.
    let normalized_subagents: Option<Value> = match subagents {
        None => None,
        Some(raw) => normalize_subagents(&raw).map(|entries| Value::Array(entries)),
    };

    {
        let obj = state
            .as_object_mut()
            .ok_or_else(|| ToolError::new("Agents state is not an object."))?;
        obj.entry("sessions".to_string())
            .or_insert_with(|| json!([]));
    }

    let exists = find_session(state, &clean_agent_id).is_some();
    if !exists {
        if let Some(sessions) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("sessions"))
            .and_then(|s| s.as_array_mut())
        {
            sessions.push(json!({
                "agentId": clean_agent_id,
                "firstSeenAt": now_rfc3339(),
            }));
        }
    }

    let mut role_to_store = role.to_string();
    {
        let session = find_session_mut(state, &clean_agent_id)
            .ok_or_else(|| ToolError::new("Failed to upsert session."))?;
        if let Some(stored) = session.get("role").and_then(|v| v.as_str()) {
            if !stored.is_empty() && roles_same_canonical(stored, role) {
                role_to_store = stored.to_string();
            }
        }
        session.insert("role".into(), json!(role_to_store));
        if !normalized_model.is_empty() {
            session.insert("model".into(), json!(normalized_model));
        } else if !session.contains_key("model") {
            session.insert("model".into(), json!(""));
        }
        session.insert("status".into(), json!(clean_status));
        if let Some(msg) = message {
            if !msg.trim().is_empty() {
                session.insert("message".into(), json!(clean_text(msg, "Message", 1000)?));
            }
        }
        if let Some(c) = client {
            if !c.trim().is_empty() {
                session.insert("client".into(), json!(clean_text(c, "Client", 40)?));
            }
        }
        if let Some(fp) = normalized_file_path {
            session.insert("currentFilePath".into(), json!(fp));
        }
        if let Some(pid) = project_id {
            session.insert("currentProjectId".into(), json!(pid));
        }
        if let Some(tid) = task_id {
            session.insert("currentTaskId".into(), json!(tid));
        }
        session.insert("lastSeenAt".into(), json!(now_rfc3339()));
        if let Some(subs) = normalized_subagents {
            session.insert("subagents".into(), subs);
        } else {
            session
                .entry("subagents".to_string())
                .or_insert_with(|| json!([]));
        }

        if clean_status == NEEDS_USER_STATUS {
            let previous_since = session
                .get("needsUser")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("since"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let needs_message = match message {
                Some(m) if !m.trim().is_empty() => clean_text(m, "Message", 1000)?,
                _ => NEEDS_USER_STATUS.to_string(),
            };
            session.insert(
                "needsUser".into(),
                json!({
                    "reason": raw_status,
                    "message": needs_message,
                    "since": previous_since.unwrap_or_else(now_rfc3339),
                }),
            );
        } else {
            session.insert("needsUser".into(), Value::Null);
        }
    }
    Ok(())
}

// ── launch / session token gates ────────────────────────────────────────────

pub fn validate_launch_token_for_registration(
    state: &Value,
    agent_id: &str,
    role: &str,
    launch_token: Option<&str>,
) -> ToolResult<Option<Value>> {
    let session = match find_session(state, agent_id) {
        Some(s) => s.clone(),
        None => {
            if !unmanaged_privileged_agents_allowed() {
                return Err(ToolError::new(
                    "Agent registration requires an app-issued launch token from Devboule.",
                ));
            }
            return Ok(None);
        }
    };
    let existing_role = coerce_role(
        session
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    if existing_role != role {
        return Err(ToolError::new(format!(
            "Agent {agent_id} is already registered as {existing_role}."
        )));
    }
    let expected_hash = session
        .get("launchTokenHash")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !expected_hash.is_empty() {
        let token = launch_token.unwrap_or("").trim();
        if token.is_empty() {
            return Err(ToolError::new(
                "Agent registration requires the app-issued launch_token from the launch prompt.",
            ));
        }
        let issued_at = parse_iso_timestamp(
            session
                .get("launchTokenIssuedAt")
                .and_then(|v| v.as_str()),
        );
        if issued_at.is_none() || Utc::now() - issued_at.unwrap() > LAUNCH_TOKEN_WINDOW {
            return Err(ToolError::new(
                "Agent launch token expired. Relaunch the agent from Devboule.",
            ));
        }
        let got = hash_launch_token(token);
        if !constant_time_eq_hex(&got, &expected_hash) {
            return Err(ToolError::new(
                "Agent launch token is invalid for this agent id and role.",
            ));
        }
        return Ok(Some(session));
    }
    let status = session
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if status == "launch_pending" {
        return Err(ToolError::new(
            "Pending agent session is missing a launch token. Relaunch the agent from Devboule.",
        ));
    }
    // SEC#7
    let consumed = session
        .get("launchConsumedAt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !consumed.is_empty() {
        return Err(ToolError::new(
            "Agent launch credential already consumed; relaunch the agent from Devboule to register again.",
        ));
    }
    Ok(Some(session))
}

pub fn require_session_token(session: &Value, session_token: Option<&str>) -> ToolResult<()> {
    let expected_hash = session
        .get("sessionTokenHash")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    // SECURITY: unmanaged kill switch must NOT skip verification when a hash exists.
    if expected_hash.is_empty() {
        if unmanaged_privileged_agents_allowed() && session_token.unwrap_or("").trim().is_empty()
        {
            return Ok(());
        }
        return Err(ToolError::new(
            "Agent session is missing a session token. Relaunch the agent from Devboule.",
        ));
    }
    let token = session_token.unwrap_or("").trim();
    if token.is_empty() {
        return Err(ToolError::new(
            "Tool call requires the session_token returned by agent_register.",
        ));
    }
    let issued_at = parse_iso_timestamp(
        session
            .get("sessionTokenIssuedAt")
            .and_then(|v| v.as_str()),
    );
    if issued_at.is_none() || Utc::now() - issued_at.unwrap() > SESSION_TOKEN_WINDOW {
        return Err(ToolError::new(
            "Agent session token expired. Relaunch the agent from Devboule.",
        ));
    }
    let got = hash_session_token(token);
    if !constant_time_eq_hex(&got, &expected_hash) {
        return Err(ToolError::new(
            "Agent session token is invalid for this agent id and role.",
        ));
    }
    Ok(())
}

fn constant_time_eq_hex(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        let _ = Sha256::digest(a_bytes).ct_eq(&Sha256::digest(b_bytes));
        return false;
    }
    bool::from(a_bytes.ct_eq(b_bytes))
}

/// Seed a managed launch_pending session (tests / app-shaped fixtures).
pub fn seed_launch_pending(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    launch_token: &str,
) -> ToolResult<()> {
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let hash = hash_launch_token(launch_token);
        let session = json!({
            "agentId": agent_id,
            "role": role,
            "status": "launch_pending",
            "lastSeenAt": "2099-01-01T00:00:00+00:00",
            "launchTokenHash": hash,
            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
        });
        if let Some(sessions) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("sessions"))
            .and_then(|s| s.as_array_mut())
        {
            sessions.retain(|s| s.get("agentId").and_then(|v| v.as_str()) != Some(agent_id));
            sessions.push(session);
        }
        write_agents_state(projects_dir, state)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn write_text_crash_safe_overwrites_existing() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-mcp-write-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");

        write_text_crash_safe(&path, "first", "test file").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");

        write_text_crash_safe(&path, "second", "test file").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");

        // No leftover temp/bak next to the target.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "state.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "expected no temp/bak leftovers, got {leftovers:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_subagents_valid_list_and_model_family() {
        let result = normalize_subagents(&json!([
            {"label": "search", "model": "claude-haiku-3-5", "count": 3},
            {"label": "review", "model": "claude-opus-4-8", "count": 1},
        ]))
        .unwrap();
        assert_eq!(
            result,
            vec![
                json!({"label": "search", "model": "haiku", "count": 3, "role": null}),
                json!({"label": "review", "model": "opus", "count": 1, "role": null}),
            ]
        );
    }

    #[test]
    fn normalize_subagents_caps_and_drops_invalid() {
        // Cap at 32.
        let entries: Vec<Value> = (0..50)
            .map(|i| json!({"label": format!("a{i}"), "model": "opus", "count": 1}))
            .collect();
        let capped = normalize_subagents(&Value::Array(entries)).unwrap();
        assert_eq!(capped.len(), 32);

        // Label max 80.
        let long = normalize_subagents(&json!([{"label": "z".repeat(200), "count": 1}])).unwrap();
        assert_eq!(long[0]["label"].as_str().unwrap().len(), 80);

        // Bad counts dropped; clean string/float coerced; default 1; clamp 9999.
        let mixed = normalize_subagents(&json!([
            {"label": "ok", "model": "opus", "count": 2},
            {"label": "nan", "model": "opus", "count": "abc"},
            {"label": "zero", "model": "opus", "count": 0},
            {"label": "neg", "model": "opus", "count": -5},
            {"label": "str", "model": "opus", "count": "4"},
            {"label": "flt", "model": "opus", "count": 5.0},
            {"label": "default", "model": "opus"},
            {"label": "huge", "model": "opus", "count": 99999},
            {"label": "   "},
            "garbage",
            5,
            null,
        ]))
        .unwrap();
        let labels: Vec<&str> = mixed
            .iter()
            .map(|e| e["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, vec!["ok", "str", "flt", "default", "huge"]);
        assert_eq!(mixed[0]["count"], 2);
        assert_eq!(mixed[1]["count"], 4);
        assert_eq!(mixed[2]["count"], 5);
        assert_eq!(mixed[3]["count"], 1);
        assert_eq!(mixed[4]["count"], 9999);
    }

    #[test]
    fn normalize_subagents_role_and_none_vs_empty() {
        let alias = normalize_subagents(&json!([
            {"label": "plan", "model": "opus", "count": 1, "role": "architect"}
        ]))
        .unwrap();
        assert_eq!(alias[0]["role"], "coder");

        let orch = normalize_subagents(&json!([
            {"label": "plan", "model": "opus", "count": 1, "role": "orchestrator"}
        ]))
        .unwrap();
        assert_eq!(orch[0]["role"], "orchestrator");

        let bad = normalize_subagents(&json!([
            {"label": "x", "model": "opus", "count": 1, "role": "wizard"}
        ]))
        .unwrap();
        assert!(bad[0]["role"].is_null());

        assert!(normalize_subagents(&Value::Null).is_none());
        assert!(normalize_subagents(&json!("not a list")).is_none());
        assert!(normalize_subagents(&json!(42)).is_none());
        assert_eq!(normalize_subagents(&json!([])).unwrap(), Vec::<Value>::new());
    }

    #[test]
    fn normalize_subagents_strips_unknown_keys() {
        let result = normalize_subagents(&json!([
            {"label": "ok", "model": "opus", "count": 1, "secret": "leak", "extra": 1}
        ]))
        .unwrap();
        let obj = result[0].as_object().unwrap();
        let keys: std::collections::HashSet<&str> = obj.keys().map(|k| k.as_str()).collect();
        assert_eq!(
            keys,
            ["label", "model", "count", "role"]
                .into_iter()
                .collect::<std::collections::HashSet<_>>()
        );
    }

    #[test]
    fn current_file_path_rejects_control_chars() {
        let err = normalize_current_file_path("src/\nmain.rs").unwrap_err();
        assert!(
            err.message.contains("control characters"),
            "{}",
            err.message
        );
        let err = normalize_current_file_path("src/\x00evil.rs").unwrap_err();
        assert!(err.message.contains("control characters"));
        // Normal path still works.
        let ok = normalize_current_file_path("src/main.rs").unwrap();
        assert_eq!(ok.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn upsert_rejects_reserved_statuses() {
        let mut state = default_agents_state();
        for status in ["launch_pending", "closed", "LAUNCH_PENDING", "Closed"] {
            let err = upsert_session(
                &mut state,
                "agent-1",
                "coder",
                None,
                status,
                Some("nope"),
                None,
                None,
                None,
            None,
            None
        )
            .unwrap_err();
            assert!(
                err.message.contains("reserved"),
                "status={status}: {}",
                err.message
            );
        }
        // Allowed status still works.
        upsert_session(
            &mut state,
            "agent-1",
            "coder",
            Some("opus"),
            "active",
            Some("ok"),
            None,
            None,
            None,
            None,
            None
        )
        .unwrap();
        assert_eq!(
            find_session(&state, "agent-1")
                .unwrap()
                .get("status")
                .and_then(|v| v.as_str()),
            Some("active")
        );
    }

    #[test]
    fn upsert_normalizes_subagents() {
        let mut state = default_agents_state();
        upsert_session(
            &mut state,
            "agent-1",
            "coder",
            Some("opus"),
            "active",
            None,
            None,
            None,
            Some(json!([
                {"label": "search", "model": "claude-haiku-3-5", "count": 3, "secret": "x"},
                {"label": "", "count": 1},
                {"label": "ok", "count": 0},
            ])),
            None,
            None
        )
        .unwrap();
        let subs = find_session(&state, "agent-1")
            .unwrap()
            .get("subagents")
            .cloned()
            .unwrap();
        assert_eq!(
            subs,
            json!([{"label": "search", "model": "haiku", "count": 3, "role": null}])
        );

        // Non-array leaves stored untouched.
        upsert_session(
            &mut state,
            "agent-1",
            "coder",
            None,
            "active",
            None,
            None,
            None,
            Some(json!("not-a-list")),
            None,
            None
        )
        .unwrap();
        assert_eq!(
            find_session(&state, "agent-1").unwrap().get("subagents"),
            Some(&json!([{"label": "search", "model": "haiku", "count": 3, "role": null}]))
        );

        // Empty list clears.
        upsert_session(
            &mut state,
            "agent-1",
            "coder",
            None,
            "active",
            None,
            None,
            None,
            Some(json!([])),
            None,
            None
        )
        .unwrap();
        assert_eq!(
            find_session(&state, "agent-1").unwrap().get("subagents"),
            Some(&json!([]))
        );
    }
}
