//! `agent_register`, `agent_heartbeat`, `agent_state` — security parity with
//! `oracle/server/aspis_mcp.py`.

use crate::state::{
    add_event, compact_session_ack, find_session, find_session_mut, hash_session_token,
    normalize_agent_id, normalize_model, normalize_role, public_agents_state, read_agents_state,
    require_session_token, unmanaged_privileged_agents_allowed, upsert_session,
    validate_launch_token_for_registration, with_agents_lock, write_agents_state,
    generate_session_token, now_rfc3339, ToolError, ToolResult,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

/// SSoT role allowlists (same file as app / Python).
const ROLE_RULES_JSON: &str = include_str!("../../../oracle/server/role_rules.json");

fn role_allowed_tools() -> &'static HashMap<String, HashSet<String>> {
    static MAP: OnceLock<HashMap<String, HashSet<String>>> = OnceLock::new();
    MAP.get_or_init(|| {
        let v: Value = serde_json::from_str(ROLE_RULES_JSON.trim_start_matches('\u{feff}'))
            .expect("role_rules.json parse");
        let mut map = HashMap::new();
        if let Some(roles) = v.get("roles").and_then(|r| r.as_array()) {
            for role in roles {
                let name = role
                    .get("role")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let tools: HashSet<String> = role
                    .get("allowedTools")
                    .and_then(|t| t.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                map.insert(name, tools);
            }
        }
        map
    })
}

fn role_allows(role: &str, tool: &str) -> bool {
    role_allowed_tools()
        .get(role)
        .map(|set| set.contains(tool))
        .unwrap_or(false)
}

pub fn agent_register(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    model: Option<&str>,
    client: Option<&str>,
    message: Option<&str>,
    launch_token: Option<&str>,
) -> ToolResult<Value> {
    let role = normalize_role(role)?;
    let agent_id = normalize_agent_id(agent_id)?;
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let existing =
            validate_launch_token_for_registration(&state, &agent_id, &role, launch_token)?;
        if let Some(ref existing_session) = existing {
            let had_hash = existing_session
                .get("launchTokenHash")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .is_empty()
                == false;
            if let Some(session) = find_session_mut(&mut state, &agent_id) {
                if had_hash {
                    // SEC#7: consume launch credential under lock.
                    session.insert("launchConsumedAt".into(), json!(now_rfc3339()));
                }
                session.remove("launchTokenHash");
                session.remove("launchTokenIssuedAt");
            }
        }
        let managed_registration =
            existing.is_some() || !unmanaged_privileged_agents_allowed();
        let session_token = if managed_registration {
            generate_session_token()
        } else {
            String::new()
        };
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            model,
            "active",
            Some(message.unwrap_or("registered")),
            client,
            None,
            None,
            None,
            None,
        )?;
        {
            let session = find_session_mut(&mut state, &agent_id)
                .ok_or_else(|| ToolError::new("Session missing after register."))?;
            if managed_registration {
                session.insert(
                    "sessionTokenHash".into(),
                    json!(hash_session_token(&session_token)),
                );
                session.insert("sessionTokenIssuedAt".into(), json!(now_rfc3339()));
            } else {
                session.remove("sessionTokenHash");
                session.remove("sessionTokenIssuedAt");
            }
        }
        add_event(
            &mut state,
            &agent_id,
            &role,
            "register",
            message.unwrap_or("Agent registered."),
            None,
            None,
            None,
            None,
        )?;
        if normalize_model(model).is_empty() {
            add_event(
                &mut state,
                &agent_id,
                &role,
                "register_incomplete",
                "Agent registered without reporting a model; declare `model` at agent_register.",
                None,
                None,
                None,
                None,
            )?;
        }
        let written = write_agents_state(projects_dir, state)?;
        Ok(compact_session_ack(
            &written,
            &agent_id,
            if session_token.is_empty() {
                None
            } else {
                Some(session_token.as_str())
            },
        ))
    })
}

pub fn agent_heartbeat(
    projects_dir: &Path,
    agent_id: &str,
    status: Option<&str>,
    message: Option<&str>,
    session_token: Option<&str>,
    file_path: Option<&str>,
    subagents: Option<Value>,
) -> ToolResult<Value> {
    let agent_id = normalize_agent_id(agent_id)?;
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let session = find_session(&state, &agent_id)
            .cloned()
            .ok_or_else(|| {
                ToolError::new("Agent must call agent_register before heartbeat.")
            })?;
        let status_lc = session
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if status_lc == "launch_pending" {
            return Err(ToolError::new(
                "Agent launch is pending. Call agent_register before heartbeat.",
            ));
        }
        let role = normalize_role(
            session
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        )?;
        if !role_allows(&role, "agent_heartbeat") {
            return Err(ToolError::new(format!(
                "{role} agents cannot use agent_heartbeat."
            )));
        }
        require_session_token(&session, session_token)?;
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            status.unwrap_or("active"),
            message,
            None,
            file_path,
            subagents,
            None,
            None,
        )?;
        // Heartbeat NEVER echoes sessionToken.
        let written = write_agents_state(projects_dir, state)?;
        Ok(compact_session_ack(&written, &agent_id, None))
    })
}

/// Require a registered agent with matching role, session token, and tool allowlist.
///
/// Mirrors Python `require_agent_tool` / `require_registered_role`.
pub fn require_agent_tool(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    tool_name: &str,
    session_token: Option<&str>,
) -> ToolResult<(String, String)> {
    let agent_id = normalize_agent_id(agent_id)?;
    let role = normalize_role(role)?;
    with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let session = find_session(&state, &agent_id).ok_or_else(|| {
            ToolError::new(
                "Agent must call agent_register before using project or provider tools.",
            )
        })?;
        let status_lc = session
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if status_lc == "launch_pending" {
            return Err(ToolError::new(
                "Agent launch is pending. Call agent_register before using project or provider tools.",
            ));
        }
        let registered_role = normalize_role(
            session
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        )?;
        if registered_role != role {
            return Err(ToolError::new(format!(
                "Agent role mismatch: registered as {registered_role}, requested {role}."
            )));
        }
        if !role_allows(&role, tool_name) {
            return Err(ToolError::new(format!(
                "{role} agents cannot use {tool_name}."
            )));
        }
        require_session_token(session, session_token)?;
        Ok((agent_id.clone(), role.clone()))
    })
}

pub fn agent_state(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let agent_id = normalize_agent_id(agent_id)?;
    let role = normalize_role(role)?;
    if !role_allows(&role, "agent_state") {
        return Err(ToolError::new(format!(
            "{role} agents cannot use agent_state."
        )));
    }
    with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let session = find_session(&state, &agent_id).ok_or_else(|| {
            ToolError::new(
                "Agent must call agent_register before using project or provider tools.",
            )
        })?;
        let status_lc = session
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if status_lc == "launch_pending" {
            return Err(ToolError::new(
                "Agent launch is pending. Call agent_register before using project or provider tools.",
            ));
        }
        let registered_role = normalize_role(
            session
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        )?;
        if registered_role != role {
            return Err(ToolError::new(format!(
                "Agent role mismatch: registered as {registered_role}, requested {role}."
            )));
        }
        require_session_token(session, session_token)?;
        Ok(public_agents_state(&state, &agent_id))
    })
}
