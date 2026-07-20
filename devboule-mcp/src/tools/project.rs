//! Project / Kanban tools (P2): list/get/next/claim/update/note/title/followup.
//!
//! Parity with `oracle/server/aspis_mcp.py` handlers.

use crate::project_file::{
    clean_description, ensure_inside_projects, load_project_locked, next_task_id,
    normalize_project_id, normalize_task_category, normalize_task_id, normalize_task_status,
    note_id, project_lock_path, project_path, public_project, read_project_file, summarize_project,
    write_project_file,
};
use crate::state::{
    add_event, clean_text, compact_session_ack, normalize_role, now_rfc3339, parse_iso_timestamp,
    read_agents_state, upsert_session, with_agents_lock, with_file_lock, write_agents_state,
    ToolError, ToolResult,
};
use crate::tools::agent_lifecycle::require_agent_tool;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

const CODER_LIKE_ROLES: &[&str] = &["coder", "orchestrator"];
const CLAIM_LEASE_MINUTES: i64 = 45;
const LEASELESS_CLAIM_WINDOW_MINUTES: i64 = 15;

fn is_coder_like(role: &str) -> bool {
    CODER_LIKE_ROLES.contains(&role)
}

fn claim_is_active(claim: &Value) -> bool {
    let status = claim.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if matches!(status, "done" | "review" | "blocked") {
        return false;
    }
    if let Some(lease) = parse_iso_timestamp(claim.get("leaseUntil").and_then(|v| v.as_str())) {
        return lease > Utc::now();
    }
    let updated = parse_iso_timestamp(
        claim
            .get("updatedAt")
            .or_else(|| claim.get("claimedAt"))
            .and_then(|v| v.as_str()),
    );
    match updated {
        Some(ts) => Utc::now() - ts <= Duration::minutes(LEASELESS_CLAIM_WINDOW_MINUTES),
        None => false,
    }
}

fn active_claim_for_task<'a>(
    state: &'a Value,
    project_id: &str,
    task_id: &str,
) -> Option<&'a Value> {
    state.get("claims")?.as_array()?.iter().find(|claim| {
        claim.get("projectId").and_then(|v| v.as_str()) == Some(project_id)
            && claim.get("taskId").and_then(|v| v.as_str()) == Some(task_id)
            && claim_is_active(claim)
    })
}

fn require_claim_for_status_update(
    state: &mut Value,
    agent_id: &str,
    role: &str,
    project_id: &str,
    task_id: &str,
    target_status: &str,
) -> ToolResult<()> {
    // Active claim path.
    if let Some(claims) = state.get("claims").and_then(|c| c.as_array()) {
        if let Some(claim) = claims.iter().find(|claim| {
            claim.get("projectId").and_then(|v| v.as_str()) == Some(project_id)
                && claim.get("taskId").and_then(|v| v.as_str()) == Some(task_id)
                && claim_is_active(claim)
        }) {
            if claim.get("agentId").and_then(|v| v.as_str()) != Some(agent_id) {
                return Err(ToolError::new(format!(
                    "Task is claimed by {} until {}.",
                    claim.get("agentId").and_then(|v| v.as_str()).unwrap_or("?"),
                    claim
                        .get("leaseUntil")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                )));
            }
            let claim_role = normalize_role(
                claim.get("role").and_then(|v| v.as_str()).unwrap_or(""),
            )?;
            if claim_role != role {
                return Err(ToolError::new(
                    "Claim role does not match the registered agent role.",
                ));
            }
            return Ok(());
        }
    }

    // WARNING 3: owner reopen to todo after review (claim inactive).
    if target_status == "todo" {
        if let Some(claims) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("claims"))
            .and_then(|c| c.as_array_mut())
        {
            for claim in claims.iter_mut() {
                let matches = claim.get("projectId").and_then(|v| v.as_str()) == Some(project_id)
                    && claim.get("taskId").and_then(|v| v.as_str()) == Some(task_id)
                    && claim.get("agentId").and_then(|v| v.as_str()) == Some(agent_id)
                    && claim.get("status").and_then(|v| v.as_str()) != Some("done");
                if !matches {
                    continue;
                }
                let claim_role = claim
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if normalize_role(&claim_role)? != role {
                    continue;
                }
                if let Some(obj) = claim.as_object_mut() {
                    // Owner reactivating claim: renew lease + refresh timestamps.
                    let lease_until =
                        (Utc::now() + Duration::minutes(CLAIM_LEASE_MINUTES)).to_rfc3339();
                    obj.insert("status".into(), json!("wip"));
                    obj.insert("updatedAt".into(), json!(now_rfc3339()));
                    obj.insert("leaseUntil".into(), json!(lease_until));
                    if obj.contains_key("claimedAt") {
                        obj.insert("claimedAt".into(), json!(now_rfc3339()));
                    }
                }
                return Ok(());
            }
        }
    }

    Err(ToolError::new(
        "Agent must claim the task before updating status.",
    ))
}

pub fn validate_transition(
    role: &str,
    status: &str,
    evidence: &str,
    confidence: f64,
    current_status: Option<&str>,
) -> ToolResult<()> {
    if current_status == Some("done") {
        return Err(ToolError::new(
            "Done tasks cannot be changed through project_update_status.",
        ));
    }
    if is_coder_like(role) && !matches!(status, "todo" | "wip" | "review" | "blocked") {
        return Err(ToolError::new(
            "Coder can only set todo, wip, review or blocked.",
        ));
    }
    if role == "verifier" && !matches!(status, "done" | "blocked") {
        return Err(ToolError::new("Verifier can only set done or blocked."));
    }
    if matches!(status, "review" | "blocked") && evidence.trim().chars().count() < 12 {
        let cap = status
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('?');
        return Err(ToolError::new(format!(
            "{cap}{} requires concrete evidence.",
            &status[1..]
        )));
    }
    if status == "done" {
        if role != "verifier" {
            return Err(ToolError::new("Only verifier agents can set done."));
        }
        if evidence.trim().chars().count() < 12 {
            return Err(ToolError::new("Done requires concrete evidence."));
        }
        if confidence < 0.70 {
            return Err(ToolError::new("Done requires confidence >= 0.70."));
        }
        if current_status != Some("review") {
            return Err(ToolError::new(
                "Done requires the task to be in review first.",
            ));
        }
    }
    Ok(())
}

fn audit_agent_read(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    event_type: &str,
    message: &str,
    project_id: Option<&str>,
    task_id: Option<&str>,
) -> ToolResult<()> {
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        upsert_session(
            &mut state,
            agent_id,
            role,
            None,
            event_type,
            Some(message),
            None,
            None,
            None,
            project_id,
            task_id,
        )?;
        add_event(
            &mut state,
            agent_id,
            role,
            event_type,
            message,
            project_id,
            task_id,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })
}

// ── tools ───────────────────────────────────────────────────────────────────

pub fn project_list(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "project_list", session_token)?;
    let mut projects = Vec::new();
    if projects_dir.is_dir() {
        let entries = std::fs::read_dir(projects_dir).map_err(|e| {
            ToolError::new(format!("Could not list projects: {e}"))
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            // Reject symlink / resolve escapes before reading (path confinement).
            let path = match ensure_inside_projects(projects_dir, &path) {
                Ok(p) => p,
                Err(err) => {
                    projects.push(json!({
                        "id": path.file_stem().and_then(|s| s.to_str()).unwrap_or(""),
                        "path": path.to_string_lossy(),
                        "error": err.message,
                    }));
                    continue;
                }
            };
            let lock_path = path.with_extension("md.lock");
            match with_file_lock(&lock_path, || read_project_file(&path)) {
                Ok(doc) => projects.push(summarize_project(&doc)),
                Err(err) => {
                    projects.push(json!({
                        "id": path.file_stem().and_then(|s| s.to_str()).unwrap_or(""),
                        "path": path.to_string_lossy(),
                        "error": err.message,
                    }));
                }
            }
        }
    }
    projects.sort_by(|a, b| {
        let ka = (
            a.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""),
            a.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        );
        let kb = (
            b.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""),
            b.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        );
        kb.cmp(&ka)
    });
    let n = projects.len();
    audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "project_read",
        &format!("Listed {n} projects."),
        None,
        None,
    )?;
    Ok(json!({
        "projectsDir": projects_dir.to_string_lossy(),
        "projects": projects,
    }))
}

pub fn project_get(
    projects_dir: &Path,
    project_id: &str,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, "project_get", session_token)?;
    let project = load_project_locked(projects_dir, project_id)?;
    let pid = project.metadata.id().to_string();
    audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "project_read",
        &format!("Read project {pid}."),
        Some(&pid),
        None,
    )?;
    Ok(public_project(&project))
}

pub fn project_next_task(
    projects_dir: &Path,
    project_id: &str,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_next_task",
        session_token,
    )?;
    let project = load_project_locked(projects_dir, project_id)?;
    let pid = project.metadata.id().to_string();
    let tasks = project
        .state
        .get("tasks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let claimed_by_others: HashSet<String> = with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        let set = state
            .get("claims")
            .and_then(|c| c.as_array())
            .map(|claims| {
                claims
                    .iter()
                    .filter(|claim| {
                        claim.get("projectId").and_then(|v| v.as_str()) == Some(pid.as_str())
                            && claim.get("agentId").and_then(|v| v.as_str()) != Some(agent_id.as_str())
                            && claim_is_active(claim)
                    })
                    .filter_map(|c| c.get("taskId").and_then(|v| v.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Ok::<_, ToolError>(set)
    })?;

    let preferred: &[&str] = if role == "verifier" {
        &["review", "blocked"]
    } else {
        &["todo", "wip", "blocked"]
    };
    for status in preferred {
        for task in &tasks {
            let tid = task.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if claimed_by_others.contains(tid) {
                continue;
            }
            if task.get("status").and_then(|v| v.as_str()) == Some(*status) {
                audit_agent_read(
                    projects_dir,
                    &agent_id,
                    &role,
                    "project_next",
                    &format!("Selected next task {tid}."),
                    Some(&pid),
                    Some(tid),
                )?;
                return Ok(json!({
                    "project": summarize_project(&project),
                    "task": task,
                }));
            }
        }
    }
    audit_agent_read(
        projects_dir,
        &agent_id,
        &role,
        "project_next",
        "No next task available.",
        Some(&pid),
        None,
    )?;
    Ok(json!({
        "project": summarize_project(&project),
        "task": null,
    }))
}

pub fn project_claim_task(
    projects_dir: &Path,
    project_id: &str,
    task_id: &str,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_claim_task",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    let task_id = normalize_task_id(task_id)?;

    // Agents lock FIRST, then project lock (Python parity / deadlock avoidance).
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        if let Some(existing) = active_claim_for_task(&state, &project_id, &task_id) {
            if existing.get("agentId").and_then(|v| v.as_str()) != Some(agent_id.as_str()) {
                return Err(ToolError::new(format!(
                    "Task is already claimed by {} until {}.",
                    existing
                        .get("agentId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                    existing
                        .get("leaseUntil")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                )));
            }
        }

        let lock = project_lock_path(projects_dir, &project_id)?;
        let path = project_path(projects_dir, &project_id)?;
        let (task_status_after, task_title, project_title, claim_status, lease_until) =
            with_file_lock(&lock, || {
                if !path.exists() {
                    return Err(ToolError::new("Project not found."));
                }
                let mut project = read_project_file(&path)?;
                let status = project.metadata.status();
                if matches!(status, "draft" | "paused" | "archived" | "done") {
                    return Err(ToolError::new(
                        "Cannot claim tasks on draft, paused, done or archived projects.",
                    ));
                }
                let tasks = project
                    .state
                    .get_mut("tasks")
                    .and_then(|t| t.as_array_mut())
                    .ok_or_else(|| ToolError::new("Project state tasks must be a list."))?;
                let task = tasks
                    .iter_mut()
                    .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(task_id.as_str()))
                    .ok_or_else(|| ToolError::new("Task not found."))?;
                let task_status = task
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if task_status == "done" {
                    return Err(ToolError::new("Done tasks cannot be claimed."));
                }
                if role == "verifier" && !matches!(task_status.as_str(), "review" | "blocked") {
                    return Err(ToolError::new(
                        "Verifier agents can only claim review or blocked tasks.",
                    ));
                }
                if is_coder_like(&role)
                    && !matches!(task_status.as_str(), "todo" | "wip" | "blocked")
                {
                    return Err(ToolError::new(
                        "Coder agents can only claim todo, wip or blocked tasks.",
                    ));
                }
                if is_coder_like(&role) && task_status == "todo" {
                    if let Some(obj) = task.as_object_mut() {
                        obj.insert("status".into(), json!("wip"));
                        obj.insert("updatedAt".into(), json!(now_rfc3339()));
                    }
                    project.metadata.set("status", "active".into());
                    project.metadata.set("updatedAt", now_rfc3339());
                    // keep updated_at snake key
                    project.metadata.set("updated_at", now_rfc3339());
                    if let Some(notes) = project
                        .state
                        .as_object_mut()
                        .and_then(|o| o.get_mut("notes"))
                        .and_then(|n| n.as_array_mut())
                    {
                        notes.push(json!({
                            "id": note_id(),
                            "text": format!(
                                "{agent_id} ({role}) claimed {task_id} and moved it to wip."
                            ),
                            "source": format!("agent:{agent_id}"),
                            "createdAt": now_rfc3339(),
                        }));
                    }
                    project = write_project_file(projects_dir, project)?;
                }
                let task_after = project
                    .state
                    .get("tasks")
                    .and_then(|t| t.as_array())
                    .and_then(|arr| {
                        arr.iter()
                            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(task_id.as_str()))
                    })
                    .cloned()
                    .ok_or_else(|| ToolError::new("Task not found."))?;
                let task_status_after = task_after
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let task_title = task_after
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let project_title = project.metadata.title().to_string();
                let claim_status = if is_coder_like(&role) && task_status_after == "wip" {
                    "wip"
                } else {
                    "claimed"
                }
                .to_string();
                let lease_until = (Utc::now() + Duration::minutes(CLAIM_LEASE_MINUTES)).to_rfc3339();
                Ok((
                    task_status_after,
                    task_title,
                    project_title,
                    claim_status,
                    lease_until,
                ))
            })?;

        // Replace any existing claim rows for this task.
        if let Some(claims) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("claims"))
            .and_then(|c| c.as_array_mut())
        {
            claims.retain(|item| {
                !(item.get("projectId").and_then(|v| v.as_str()) == Some(project_id.as_str())
                    && item.get("taskId").and_then(|v| v.as_str()) == Some(task_id.as_str()))
            });
            claims.push(json!({
                "projectId": project_id,
                "projectTitle": project_title,
                "taskId": task_id,
                "taskTitle": task_title,
                "agentId": agent_id,
                "role": role,
                "status": claim_status,
                "claimedAt": now_rfc3339(),
                "updatedAt": now_rfc3339(),
                "leaseUntil": lease_until,
            }));
        }
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            &claim_status,
            None,
            None,
            None,
            None,
            Some(&project_id),
            Some(&task_id),
        )?;
        let msg = if claim_status == "claimed" {
            format!("Claimed {task_id}.")
        } else {
            format!("Claimed {task_id} and moved it to wip.")
        };
        add_event(
            &mut state,
            &agent_id,
            &role,
            "claim",
            &msg,
            Some(&project_id),
            Some(&task_id),
            Some(&task_status_after),
            None,
        )?;
        let saved = write_agents_state(projects_dir, state)?;
        let mut ack = compact_session_ack(&saved, &agent_id, None);
        if let Some(obj) = ack.as_object_mut() {
            obj.insert(
                "claim".into(),
                json!({
                    "projectId": project_id,
                    "taskId": task_id,
                    "status": claim_status,
                    "leaseUntil": lease_until,
                }),
            );
        }
        Ok(ack)
    })
}

pub fn project_update_status(
    projects_dir: &Path,
    project_id: &str,
    task_id: &str,
    status: &str,
    agent_id: &str,
    role: &str,
    evidence: Option<&str>,
    confidence: Option<f64>,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_update_status",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    let task_id = normalize_task_id(task_id)?;
    let status = normalize_task_status(status)?;
    let evidence = evidence.unwrap_or("").trim().to_string();
    let confidence = confidence.unwrap_or(0.0);

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        require_claim_for_status_update(
            &mut state,
            &agent_id,
            &role,
            &project_id,
            &task_id,
            &status,
        )?;

        let lock = project_lock_path(projects_dir, &project_id)?;
        let path = project_path(projects_dir, &project_id)?;
        let saved = with_file_lock(&lock, || {
            if !path.exists() {
                return Err(ToolError::new("Project not found."));
            }
            let mut project = read_project_file(&path)?;
            let pstatus = project.metadata.status();
            if matches!(pstatus, "draft" | "paused" | "archived") {
                return Err(ToolError::new(
                    "Cannot update tasks on draft, paused or archived projects.",
                ));
            }
            let tasks = project
                .state
                .get_mut("tasks")
                .and_then(|t| t.as_array_mut())
                .ok_or_else(|| ToolError::new("Project state tasks must be a list."))?;
            let task = tasks
                .iter_mut()
                .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(task_id.as_str()))
                .ok_or_else(|| ToolError::new("Task not found."))?;
            let current = task
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            validate_transition(
                &role,
                &status,
                &evidence,
                confidence,
                current.as_deref(),
            )?;
            if let Some(obj) = task.as_object_mut() {
                obj.insert("status".into(), json!(status));
                obj.insert("updatedAt".into(), json!(now_rfc3339()));
            }
            let mut note_text = format!("{agent_id} ({role}) set {task_id} to {status}.");
            if !evidence.is_empty() {
                let ev: String = evidence.chars().take(1200).collect();
                note_text = format!("{note_text} Evidence: {ev}");
            }
            if let Some(notes) = project
                .state
                .as_object_mut()
                .and_then(|o| o.get_mut("notes"))
                .and_then(|n| n.as_array_mut())
            {
                notes.push(json!({
                    "id": note_id(),
                    "text": note_text,
                    "source": format!("agent:{agent_id}"),
                    "createdAt": now_rfc3339(),
                }));
            }
            let all_done = project
                .state
                .get("tasks")
                .and_then(|t| t.as_array())
                .map(|arr| {
                    arr.iter()
                        .all(|t| t.get("status").and_then(|v| v.as_str()) == Some("done"))
                })
                .unwrap_or(false);
            if all_done {
                project.metadata.set("status", "done".into());
            } else if project.metadata.status() == "done" && status != "done" {
                project.metadata.set("status", "active".into());
            }
            project.metadata.set("updated_at", now_rfc3339());
            write_project_file(projects_dir, project)
        })?;

        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            &status,
            Some(if evidence.is_empty() {
                format!("{task_id} -> {status}")
            } else {
                evidence.clone()
            })
            .as_deref(),
            None,
            None,
            None,
            Some(&project_id),
            Some(&task_id),
        )?;
        if let Some(claims) = state
            .as_object_mut()
            .and_then(|o| o.get_mut("claims"))
            .and_then(|c| c.as_array_mut())
        {
            for claim in claims.iter_mut() {
                let matches = claim.get("projectId").and_then(|v| v.as_str())
                    == Some(project_id.as_str())
                    && claim.get("taskId").and_then(|v| v.as_str()) == Some(task_id.as_str())
                    && claim.get("agentId").and_then(|v| v.as_str()) == Some(agent_id.as_str());
                if !matches {
                    continue;
                }
                let claim_role = claim
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if normalize_role(&claim_role).ok().as_deref() != Some(role.as_str()) {
                    continue;
                }
                if let Some(obj) = claim.as_object_mut() {
                    obj.insert("status".into(), json!(status));
                    obj.insert("updatedAt".into(), json!(now_rfc3339()));
                    if !evidence.is_empty() {
                        obj.insert("evidence".into(), json!(evidence));
                    }
                }
            }
        }
        add_event(
            &mut state,
            &agent_id,
            &role,
            "status",
            &format!("{task_id} -> {status}"),
            Some(&project_id),
            Some(&task_id),
            Some(&status),
            if evidence.is_empty() {
                None
            } else {
                Some(evidence.as_str())
            },
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(public_project(&saved))
    })
}

pub fn project_append_note(
    projects_dir: &Path,
    project_id: &str,
    text: &str,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_append_note",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    let text = clean_text(text, "Note", 4000)?;

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let lock = project_lock_path(projects_dir, &project_id)?;
        let path = project_path(projects_dir, &project_id)?;
        let saved = with_file_lock(&lock, || {
            if !path.exists() {
                return Err(ToolError::new("Project not found."));
            }
            let mut project = read_project_file(&path)?;
            if project.metadata.status() == "draft" {
                return Err(ToolError::new(
                    "project_append_note: draft projects are read-only — activate the project first.",
                ));
            }
            if let Some(notes) = project
                .state
                .as_object_mut()
                .and_then(|o| o.get_mut("notes"))
                .and_then(|n| n.as_array_mut())
            {
                notes.push(json!({
                    "id": note_id(),
                    "text": text,
                    "source": format!("agent:{agent_id}"),
                    "createdAt": now_rfc3339(),
                }));
            }
            project.metadata.set("updated_at", now_rfc3339());
            write_project_file(projects_dir, project)
        })?;
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            "noted",
            Some(&text),
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
            "note",
            &text,
            Some(&project_id),
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(public_project(&saved))
    })
}

pub fn project_set_title(
    projects_dir: &Path,
    project_id: &str,
    title: &str,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_set_title",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    let title = clean_text(title, "Project title", 200)?;

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let lock = project_lock_path(projects_dir, &project_id)?;
        let path = project_path(projects_dir, &project_id)?;
        let saved = with_file_lock(&lock, || {
            if !path.exists() {
                return Err(ToolError::new("Project not found."));
            }
            let mut project = read_project_file(&path)?;
            if project.metadata.status() == "draft" {
                return Err(ToolError::new(
                    "project_set_title: draft projects are read-only — activate the project first.",
                ));
            }
            project.metadata.set("title", title.clone());
            project.metadata.set("updated_at", now_rfc3339());
            write_project_file(projects_dir, project)
        })?;
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            "renamed",
            Some(&title),
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
            "rename",
            &title,
            Some(&project_id),
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(public_project(&saved))
    })
}

pub fn project_create_followup(
    projects_dir: &Path,
    project_id: &str,
    title: &str,
    reason: &str,
    agent_id: &str,
    role: &str,
    category: Option<&str>,
    description: Option<&str>,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "project_create_followup",
        session_token,
    )?;
    let project_id = normalize_project_id(project_id)?;
    let title = clean_text(title, "Task title", 500)?;
    let reason = clean_text(reason, "Reason", 2000)?;
    let category = normalize_task_category(category)?;
    let description = clean_description(description);

    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        let lock = project_lock_path(projects_dir, &project_id)?;
        let path = project_path(projects_dir, &project_id)?;
        let (saved, task) = with_file_lock(&lock, || {
            if !path.exists() {
                return Err(ToolError::new("Project not found."));
            }
            let mut project = read_project_file(&path)?;
            let pstatus = project.metadata.status();
            if matches!(pstatus, "draft" | "paused" | "archived" | "done") {
                return Err(ToolError::new(
                    "Cannot create follow-up tasks on draft, paused, done or archived projects.",
                ));
            }
            let tasks = project
                .state
                .as_object_mut()
                .and_then(|o| o.get_mut("tasks"))
                .and_then(|t| t.as_array_mut())
                .ok_or_else(|| ToolError::new("Project state tasks must be a list."))?;
            let new_id = next_task_id(tasks);
            let mut task = json!({
                "id": new_id,
                "title": title,
                "status": "todo",
                "priority": "medium",
                "assignee": null,
                "due": null,
                "linkedResources": [],
                "updatedAt": now_rfc3339(),
                "category": category,
                "suspectFileIds": [],
            });
            if let Some(desc) = &description {
                task.as_object_mut()
                    .unwrap()
                    .insert("description".into(), json!(desc));
            }
            tasks.push(task.clone());
            if let Some(notes) = project
                .state
                .as_object_mut()
                .and_then(|o| o.get_mut("notes"))
                .and_then(|n| n.as_array_mut())
            {
                notes.push(json!({
                    "id": note_id(),
                    "text": format!("Follow-up created by {agent_id} ({role}): {reason}"),
                    "source": format!("agent:{agent_id}"),
                    "createdAt": now_rfc3339(),
                }));
            }
            project.metadata.set("status", "active".into());
            project.metadata.set("updated_at", now_rfc3339());
            let saved = write_project_file(projects_dir, project)?;
            Ok((saved, task))
        })?;

        let tid = task.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            "followup",
            Some(&title),
            None,
            None,
            None,
            Some(&project_id),
            Some(&tid),
        )?;
        add_event(
            &mut state,
            &agent_id,
            &role,
            "followup",
            &reason,
            Some(&project_id),
            Some(&tid),
            Some("todo"),
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(json!({
            "project": public_project(&saved),
            "task": task,
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_file::write_test_project;
    use crate::state::seed_launch_pending;
    use crate::tools::agent_lifecycle::agent_register;
    use serde_json::json;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
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

    fn task(id: &str, title: &str, status: &str) -> Value {
        json!({
            "id": id,
            "title": title,
            "status": status,
            "updatedAt": "2026-01-01T00:00:00Z",
        })
    }

    fn register(
        projects: &Path,
        agent_id: &str,
        role: &str,
    ) -> String {
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
    fn claim_race_second_coder_blocked() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "race-proj",
            "Race",
            "active",
            json!([task("T1", "Work", "todo")]),
            &[],
        )
        .unwrap();
        let tok_a = register(&projects, "coder-a", "coder");
        let tok_b = register(&projects, "coder-b", "coder");
        let ack = project_claim_task(
            &projects,
            "race-proj",
            "T1",
            "coder-a",
            "coder",
            Some(&tok_a),
        )
        .unwrap();
        assert_eq!(ack["claim"]["status"], "wip");
        assert!(ack["claim"]["leaseUntil"].as_str().unwrap().len() > 10);
        // Compact claim ack: no full fleet dumps.
        assert!(ack.get("sessions").is_none());
        assert!(ack.get("claims").is_none());
        assert!(ack.get("events").is_none());
        assert!(ack.get("session").is_some());

        let err = project_claim_task(
            &projects,
            "race-proj",
            "T1",
            "coder-b",
            "coder",
            Some(&tok_b),
        )
        .unwrap_err();
        assert!(
            err.message.contains("already claimed") && err.message.contains("coder-a"),
            "{}",
            err.message
        );
    }

    #[test]
    fn coder_cannot_set_done_verifier_can_from_review() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "done-proj",
            "Done rules",
            "active",
            json!([task("T1", "Ship", "todo")]),
            &[],
        )
        .unwrap();
        let coder_tok = register(&projects, "coder-done", "coder");
        let ver_tok = register(&projects, "ver-done", "verifier");

        project_claim_task(
            &projects,
            "done-proj",
            "T1",
            "coder-done",
            "coder",
            Some(&coder_tok),
        )
        .unwrap();
        // Coder cannot set done while claim is still active (wip).
        let err = project_update_status(
            &projects,
            "done-proj",
            "T1",
            "done",
            "coder-done",
            "coder",
            Some("I finished everything ok"),
            Some(0.99),
            Some(&coder_tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("todo, wip, review or blocked")
                || err.message.contains("Only verifier"),
            "{}",
            err.message
        );

        // Coder moves to review with evidence, then verifier finishes.
        project_update_status(
            &projects,
            "done-proj",
            "T1",
            "review",
            "coder-done",
            "coder",
            Some("ready for final reviewer now"),
            None,
            Some(&coder_tok),
        )
        .unwrap();

        // Verifier claims review and sets done.
        project_claim_task(
            &projects,
            "done-proj",
            "T1",
            "ver-done",
            "verifier",
            Some(&ver_tok),
        )
        .unwrap();
        let done = project_update_status(
            &projects,
            "done-proj",
            "T1",
            "done",
            "ver-done",
            "verifier",
            Some("verified acceptance criteria met"),
            Some(0.85),
            Some(&ver_tok),
        )
        .unwrap();
        let tasks = done["state"]["tasks"].as_array().unwrap();
        assert_eq!(tasks[0]["status"], "done");
    }

    #[test]
    fn draft_get_ok_mutations_fail() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "draft-proj",
            "Drafty",
            "draft",
            json!([task("T1", "Later", "todo")]),
            &[],
        )
        .unwrap();
        let tok = register(&projects, "coder-draft", "coder");

        let got = project_get(
            &projects,
            "draft-proj",
            "coder-draft",
            "coder",
            Some(&tok),
        )
        .unwrap();
        assert_eq!(got["metadata"]["status"], "draft");

        let claim_err = project_claim_task(
            &projects,
            "draft-proj",
            "T1",
            "coder-draft",
            "coder",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            claim_err.message.to_ascii_lowercase().contains("draft"),
            "{}",
            claim_err.message
        );

        // Seed a fake claim so update path can reach draft guard (status update
        // requires claim first). Write agents claim manually after active claim
        // is impossible on draft — so update fails either at claim or draft.
        // Append note / set title fail with draft in message.
        let note_err = project_append_note(
            &projects,
            "draft-proj",
            "should not stick",
            "coder-draft",
            "coder",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            note_err.message.contains("draft"),
            "{}",
            note_err.message
        );

        let title_err = project_set_title(
            &projects,
            "draft-proj",
            "New Title",
            "coder-draft",
            "coder",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            title_err.message.contains("draft"),
            "{}",
            title_err.message
        );
    }

    #[test]
    fn must_claim_before_status() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "claim-first",
            "Claim first",
            "active",
            json!([task("T1", "Work", "todo")]),
            &[],
        )
        .unwrap();
        let tok = register(&projects, "coder-cf", "coder");
        let err = project_update_status(
            &projects,
            "claim-first",
            "T1",
            "wip",
            "coder-cf",
            "coder",
            None,
            None,
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("must claim"),
            "{}",
            err.message
        );
    }

    #[test]
    fn next_task_skips_others_claims() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "next-proj",
            "Next",
            "active",
            json!([
                task("T1", "First", "todo"),
                task("T2", "Second", "todo"),
            ]),
            &[],
        )
        .unwrap();
        let tok_a = register(&projects, "coder-n1", "coder");
        let tok_b = register(&projects, "coder-n2", "coder");
        project_claim_task(
            &projects,
            "next-proj",
            "T1",
            "coder-n1",
            "coder",
            Some(&tok_a),
        )
        .unwrap();
        let next = project_next_task(
            &projects,
            "next-proj",
            "coder-n2",
            "coder",
            Some(&tok_b),
        )
        .unwrap();
        assert_eq!(next["task"]["id"], "T2");
    }

    #[test]
    fn set_title_preserves_censor_trusted() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let path = write_test_project(
            &projects,
            "title-proj",
            "Old Title",
            "active",
            json!([task("T1", "Work", "todo")]),
            &[("censor_trusted", "true")],
        )
        .unwrap();
        let tok = register(&projects, "coder-title", "coder");
        let out = project_set_title(
            &projects,
            "title-proj",
            "Fresh Title",
            "coder-title",
            "coder",
            Some(&tok),
        )
        .unwrap();
        assert_eq!(out["metadata"]["title"], "Fresh Title");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("censor_trusted: true"),
            "frontmatter lost trust flag: {text}"
        );
        assert!(text.contains("title: Fresh Title"), "{text}");
    }

    #[test]
    fn followup_creates_tn_verifier_cannot() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "fu-proj",
            "Follow",
            "active",
            json!([task("T1", "Existing", "todo")]),
            &[],
        )
        .unwrap();
        let coder_tok = register(&projects, "coder-fu", "coder");
        let ver_tok = register(&projects, "ver-fu", "verifier");

        let out = project_create_followup(
            &projects,
            "fu-proj",
            "New follow-up",
            "found during review work",
            "coder-fu",
            "coder",
            Some("bug"),
            None,
            Some(&coder_tok),
        )
        .unwrap();
        assert_eq!(out["task"]["id"], "T2");
        assert_eq!(out["task"]["status"], "todo");
        assert_eq!(out["task"]["category"], "bug");

        let err = project_create_followup(
            &projects,
            "fu-proj",
            "Verifier should fail",
            "nope not allowed here",
            "ver-fu",
            "verifier",
            None,
            None,
            Some(&ver_tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("verifier")
                && err.message.contains("project_create_followup"),
            "{}",
            err.message
        );
    }

    #[test]
    fn path_escape_rejected() {
        let _g = env_lock();
        set_unmanaged(false);
        let (tmp, projects) = temp_projects();
        let tok = register(&projects, "coder-path", "coder");
        // Malformed project id is rejected before any filesystem touch.
        let err = project_get(
            &projects,
            "../escape",
            "coder-path",
            "coder",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            err.message.contains("Project id") || err.message.contains("escapes"),
            "{}",
            err.message
        );
        assert!(!tmp.path().join("escape.md").exists());

        // Sibling dir sharing string prefix must not pass confinement.
        let projects_backup = tmp.path().join("projects_backup");
        std::fs::create_dir_all(&projects_backup).unwrap();
        let backup_md = projects_backup.join("sneaky.md");
        std::fs::write(
            &backup_md,
            "---\nid: sneaky\ntitle: S\nstatus: active\nupdated_at: 2026-01-01T00:00:00Z\n---\n\n```aspis-project\n{\"version\":1,\"tasks\":[],\"notes\":[]}\n```\n",
        )
        .unwrap();
        let confine_err =
            crate::project_file::ensure_inside_projects(&projects, &backup_md).unwrap_err();
        assert!(
            confine_err.message.contains("escapes"),
            "{}",
            confine_err.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_blocks_get_claim_and_list() {
        let _g = env_lock();
        set_unmanaged(false);
        let (tmp, projects) = temp_projects();
        let outside = tmp.path().join("outside.md");
        std::fs::write(
            &outside,
            "---\nid: legit\ntitle: Outside\nstatus: active\nupdated_at: 2026-01-01T00:00:00Z\n---\n\n```aspis-project\n{\"version\":1,\"tasks\":[{\"id\":\"T1\",\"title\":\"X\",\"status\":\"todo\",\"updatedAt\":\"2026-01-01T00:00:00Z\"}],\"notes\":[]}\n```\n",
        )
        .unwrap();
        let link = projects.join("legit.md");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let tok = register(&projects, "coder-sym", "coder");

        let get_err = project_get(&projects, "legit", "coder-sym", "coder", Some(&tok))
            .unwrap_err();
        assert!(
            get_err.message.contains("escapes") || get_err.message.contains("not found"),
            "get: {}",
            get_err.message
        );

        let claim_err = project_claim_task(
            &projects,
            "legit",
            "T1",
            "coder-sym",
            "coder",
            Some(&tok),
        )
        .unwrap_err();
        assert!(
            claim_err.message.contains("escapes") || claim_err.message.contains("not found"),
            "claim: {}",
            claim_err.message
        );

        let listed = project_list(&projects, "coder-sym", "coder", Some(&tok)).unwrap();
        let arr = listed["projects"].as_array().unwrap();
        // Symlink entry must surface as error, never as a clean summary of outside content.
        let symlink_entry = arr.iter().find(|p| {
            p.get("id").and_then(|v| v.as_str()) == Some("legit")
                || p.get("path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.contains("legit.md"))
                    .unwrap_or(false)
        });
        if let Some(entry) = symlink_entry {
            assert!(
                entry.get("error").is_some(),
                "symlink escape must not list cleanly: {entry}"
            );
            let err = entry["error"].as_str().unwrap_or("");
            assert!(err.contains("escapes"), "list error: {err}");
        }
        // Outside payload must not appear as a successful project title.
        for p in arr {
            assert_ne!(p.get("title").and_then(|v| v.as_str()), Some("Outside"));
        }
    }

    #[test]
    fn evidence_min_length_is_chars_not_bytes() {
        // 11 combining-capable unicode chars can be >12 bytes; still too short by char count.
        // "é" is 2 bytes in UTF-8; 6 of them = 12 bytes but only 6 chars.
        let short_multibyte = "éééééé"; // 6 chars, 12 bytes
        assert!(short_multibyte.len() >= 12);
        assert!(short_multibyte.chars().count() < 12);
        let err = validate_transition("coder", "review", short_multibyte, 0.0, Some("wip"))
            .unwrap_err();
        assert!(
            err.message.contains("evidence") || err.message.contains("Evidence"),
            "{}",
            err.message
        );
        // 12 unicode chars must pass the length gate (may still fail other rules).
        let ok_len = "éééééééééééé"; // 12 chars
        assert_eq!(ok_len.chars().count(), 12);
        assert!(validate_transition("coder", "review", ok_len, 0.0, Some("wip")).is_ok());
    }

    #[test]
    fn reopen_to_todo_renews_lease() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "reopen-proj",
            "Reopen",
            "active",
            json!([task("T1", "Work", "todo")]),
            &[],
        )
        .unwrap();
        let tok = register(&projects, "coder-re", "coder");
        project_claim_task(
            &projects,
            "reopen-proj",
            "T1",
            "coder-re",
            "coder",
            Some(&tok),
        )
        .unwrap();
        project_update_status(
            &projects,
            "reopen-proj",
            "T1",
            "review",
            "coder-re",
            "coder",
            Some("ready for review now"),
            None,
            Some(&tok),
        )
        .unwrap();

        // Expire the claim lease so claim_is_active is false (owner reopen path).
        {
            let mut state = crate::state::read_agents_state(&projects).unwrap();
            if let Some(claims) = state
                .as_object_mut()
                .and_then(|o| o.get_mut("claims"))
                .and_then(|c| c.as_array_mut())
            {
                for claim in claims.iter_mut() {
                    if let Some(obj) = claim.as_object_mut() {
                        obj.insert(
                            "leaseUntil".into(),
                            json!("2020-01-01T00:00:00+00:00"),
                        );
                        obj.insert(
                            "claimedAt".into(),
                            json!("2020-01-01T00:00:00+00:00"),
                        );
                        obj.insert("status".into(), json!("review"));
                    }
                }
            }
            crate::state::write_agents_state(&projects, state).unwrap();
        }

        project_update_status(
            &projects,
            "reopen-proj",
            "T1",
            "todo",
            "coder-re",
            "coder",
            None,
            None,
            Some(&tok),
        )
        .unwrap();

        let state = crate::state::read_agents_state(&projects).unwrap();
        let claim = state["claims"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c.get("taskId").and_then(|v| v.as_str()) == Some("T1"))
            .unwrap();
        let lease = claim["leaseUntil"].as_str().unwrap();
        let lease_ts = crate::state::parse_iso_timestamp(Some(lease)).unwrap();
        assert!(
            lease_ts > Utc::now() + Duration::minutes(40),
            "lease should be renewed ~45m out, got {lease}"
        );
        let claimed_at = claim["claimedAt"].as_str().unwrap();
        assert!(
            !claimed_at.starts_with("2020-"),
            "claimedAt should be refreshed on reopen, got {claimed_at}"
        );
    }

    #[test]
    fn compact_claim_ack_shape() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        write_test_project(
            &projects,
            "ack-proj",
            "Ack",
            "active",
            json!([task("T9", "Work", "todo")]),
            &[],
        )
        .unwrap();
        let tok = register(&projects, "coder-ack", "coder");
        let ack = project_claim_task(
            &projects,
            "ack-proj",
            "T9",
            "coder-ack",
            "coder",
            Some(&tok),
        )
        .unwrap();
        let claim = &ack["claim"];
        assert_eq!(claim["projectId"], "ack-proj");
        assert_eq!(claim["taskId"], "T9");
        assert_eq!(claim["status"], "wip");
        assert!(claim.get("leaseUntil").and_then(|v| v.as_str()).is_some());
        // Only the four keys on claim.
        let obj = claim.as_object().unwrap();
        assert_eq!(obj.len(), 4);
        assert!(ack.get("session").is_some());
        assert!(ack.get("fleet").is_some());
        assert!(ack.get("sessions").is_none());
        assert!(ack.get("claims").is_none());
        assert!(ack.get("events").is_none());
        assert!(ack.get("rules").is_none());
    }

}

