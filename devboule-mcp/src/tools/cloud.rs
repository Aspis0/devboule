//! Cloud provider tools (P5): credentials status, Cloudflare list/rotate,
//! Scaleway list/action.
//!
//! # Security (parity with Python MCP + Tauri commands)
//!
//! * Role allowlists from `role_rules.json` via `require_agent_tool`.
//! * Orchestrator / verifier: **read-only** (list + status). Mutate is
//!   coder-only (`require_provider_mutation_role`) — defense-in-depth on top
//!   of the allowlist (audit F-04-020).
//! * Mutate requires: session token + active claim + live wip/blocked task +
//!   non-coder `approvedBy` marker (unless self-attested opt-out) + evidence.
//! * Cloudflare pin: account name/id resolution; workers filtered to Aspis Bio
//!   scope (name allowlist / `aspis-bio-*` prefix / host-boundary routes).
//! * Scaleway pin: project must normalize to `aspis-bio`; destructive
//!   `delete` requires exact resource-name confirmation; `terminate` rejected
//!   (Tauri semantics — use `delete`).
//! * Tokens from **env only** (app injects at launch). Never logged.
//! * Errors sanitized (no Bearer / SCW access-key leakage).

use crate::project_file::{
    note_id, normalize_project_id, normalize_task_id, project_lock_path, project_path,
    read_project_file, write_project_file,
};
use crate::state::{
    add_event, clean_text, normalize_role, now_rfc3339, parse_iso_timestamp, read_agents_state,
    upsert_session, with_agents_lock, with_file_lock, write_agents_state, ToolError, ToolResult,
};
use crate::tools::agent_lifecycle::require_agent_tool;
use chrono::{Duration, Utc};
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration as StdDuration;

// ── constants (match Python aspis_mcp + Tauri providers.rs) ─────────────────

const CF_API: &str = "https://api.cloudflare.com/client/v4";
const SCW_API: &str = "https://api.scaleway.com";
const CF_TARGET_ACCOUNT_NAME: &str = "aspis-bio";
const SCW_TARGET_PROJECT_NAME: &str = "aspis-bio";
const APP_VAULT_SERVICE: &str = "Devboule";

const CF_ASPIS_BIO_WORKERS: &[&str] = &[
    "aspis-bio-api",
    "aspis-biovision-worker",
    "orasis-worker",
    "aspis-bio-rnaseq-api",
    "aspis-bio-papers",
    "aspis-bio-oauth",
    "aspis-bio-mta-sts",
    "aspis-bio-resend-webhooks",
];

const SCW_ZONES: &[&str] = &[
    "fr-par-1", "fr-par-2", "fr-par-3", "nl-ams-1", "nl-ams-2", "nl-ams-3", "pl-waw-1",
    "pl-waw-2", "pl-waw-3",
];
const SCW_REGIONS: &[&str] = &["fr-par", "nl-ams", "pl-waw"];

/// Agent-facing allowlist (Tauri `SCW_ALLOWED_ACTIONS`). `terminate` is listed
/// only so we can return a precise "use delete" error.
const SCW_ALLOWED_ACTIONS: &[&str] = &[
    "start", "stop", "reboot", "poweron", "poweroff", "deploy", "delete", "terminate",
];

const LEASELESS_CLAIM_WINDOW_MINUTES: i64 = 15;
const HTTP_TIMEOUT_SECS: u64 = 20;

// Env name groups (DEVBOULE_* preferred; ASPIS_* legacy; vendor defaults last).
const CF_TOKEN_ENVS: &[&str] = &[
    "DEVBOULE_CLOUDFLARE_API_TOKEN",
    "ASPIS_CLOUDFLARE_API_TOKEN",
    "CLOUDFLARE_API_TOKEN",
];
const CF_READONLY_TOKEN_ENVS: &[&str] = &[
    "DEVBOULE_CLOUDFLARE_VERIFIER_TOKEN",
    "ASPIS_CLOUDFLARE_VERIFIER_TOKEN",
];
const CF_CODER_TOKEN_ENVS: &[&str] = &[
    "DEVBOULE_CLOUDFLARE_CODER_WORKER_WRITE_TOKEN",
    "ASPIS_CLOUDFLARE_CODER_WORKER_WRITE_TOKEN",
];
const CF_SECRET_ROTATOR_TOKEN_ENVS: &[&str] = &[
    "DEVBOULE_CLOUDFLARE_SECRETS_ROTATOR_TOKEN",
    "ASPIS_CLOUDFLARE_SECRETS_ROTATOR_TOKEN",
];
const CF_ACCOUNT_ENVS: &[&str] = &[
    "DEVBOULE_CLOUDFLARE_ACCOUNT_ID",
    "ASPIS_CLOUDFLARE_ACCOUNT_ID",
    "CLOUDFLARE_ACCOUNT_ID",
];
const SCW_TOKEN_ENVS: &[&str] = &[
    "DEVBOULE_SCALEWAY_API_TOKEN",
    "ASPIS_SCALEWAY_API_TOKEN",
    "SCW_SECRET_KEY",
    "SCALEWAY_API_TOKEN",
];
const SCW_PROJECT_ENVS: &[&str] = &[
    "DEVBOULE_SCALEWAY_PROJECT_ID",
    "ASPIS_SCALEWAY_PROJECT_ID",
    "SCW_DEFAULT_PROJECT_ID",
];
const SCW_OBJECT_ACCESS_KEY_ENVS: &[&str] = &[
    "DEVBOULE_SCALEWAY_OBJECT_ACCESS_KEY",
    "ASPIS_SCALEWAY_OBJECT_ACCESS_KEY",
    "SCW_ACCESS_KEY",
];
const SCW_OBJECT_SECRET_KEY_ENVS: &[&str] = &[
    "DEVBOULE_SCALEWAY_OBJECT_SECRET_KEY",
    "ASPIS_SCALEWAY_OBJECT_SECRET_KEY",
    "SCW_S3_SECRET_KEY",
];

// ── pure validation ─────────────────────────────────────────────────────────

/// Normalize provider account/project names for pin comparison.
/// `"Aspis Bio"` / `"aspis_bio"` → `"aspis-bio"`.
pub fn normalize_provider_name(value: &str) -> String {
    value
        .trim()
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase()
}

/// Fail closed unless the visible provider account/project name pins to `target`.
pub fn require_pinned_provider_name(name: &str, target: &str, label: &str) -> ToolResult<()> {
    if normalize_provider_name(name) != target {
        return Err(ToolError::new(format!(
            "Pinned {label} is visible, but it is not {target}."
        )));
    }
    Ok(())
}

/// Extract attached instance volume ids from a Scaleway server GET payload.
pub fn scaleway_volume_ids_from_server_payload(payload: &Value) -> Vec<String> {
    payload
        .get("server")
        .and_then(|server| server.get("volumes"))
        .and_then(Value::as_object)
        .map(|volumes| {
            volumes
                .values()
                .filter_map(|volume| volume.get("id").and_then(Value::as_str))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Tauri C2 wording: a failed pre-delete volume inventory is not "no volumes".
pub fn refuse_delete_on_volume_lookup_failure(resource_label: &str, cause: &str) -> ToolError {
    ToolError::new(format!(
        "Refusing to delete {resource_label}: could not confirm its volumes before deletion ({cause}). \
Retry once Scaleway inventory is reachable so attached volumes are not orphaned."
    ))
}

/// HTTP success or already-gone (404) — used by Scaleway delete cascade.
pub fn provider_http_status_ok(status: u16) -> bool {
    (200..300).contains(&status) || status == 404
}

/// Name-only Aspis Bio worker scope (Tauri `cloudflare_worker_name_in_aspis_bio_scope`).
pub fn cloudflare_worker_name_in_scope(worker_name: &str) -> bool {
    let name = worker_name.trim().to_ascii_lowercase();
    CF_ASPIS_BIO_WORKERS.iter().any(|allowed| *allowed == name) || name.starts_with("aspis-bio-")
}

/// Host-boundary route match (Tauri C5): exactly `aspis-bio.com` or `*.aspis-bio.com`.
/// Lookalikes like `aspis-bio.com.evil.tld` are NOT in scope.
pub fn route_pattern_in_aspis_bio_host(pattern: &str) -> bool {
    let lowered = pattern.trim().to_ascii_lowercase();
    let without_scheme = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
        .unwrap_or(&lowered);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .trim_end_matches('*')
        .trim_end_matches('.');
    host == "aspis-bio.com" || host.ends_with(".aspis-bio.com")
}

/// Worker in Aspis Bio scope by name or route pattern.
pub fn cloudflare_worker_in_scope(name: &str, routes: &[Value]) -> bool {
    if cloudflare_worker_name_in_scope(name) {
        return true;
    }
    routes.iter().any(|route| {
        route
            .get("pattern")
            .and_then(|p| p.as_str())
            .map(route_pattern_in_aspis_bio_host)
            .unwrap_or(false)
    })
}

/// JS identifier for Cloudflare secret binding names (Tauri `is_valid_js_identifier`).
pub fn is_valid_js_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

/// Validate CF secret rotation inputs (no secret value echoed).
pub fn validate_cloudflare_secret_rotation(
    worker_name: &str,
    secret_name: &str,
    secret_value: &str,
) -> ToolResult<(String, String, String)> {
    let worker_name = clean_text(worker_name, "Worker name", 128)?;
    if worker_name.contains('/') || worker_name.contains('\\') {
        return Err(ToolError::new("Worker name is invalid."));
    }
    if !cloudflare_worker_name_in_scope(&worker_name) {
        return Err(ToolError::new(
            "Worker is not in the Devboule Cloudflare Aspis Bio scope.",
        ));
    }
    let secret_name = clean_text(secret_name, "Secret name", 128)?;
    if !is_valid_js_identifier(&secret_name) {
        return Err(ToolError::new(
            "Cloudflare secret name must be a valid binding identifier.",
        ));
    }
    let secret_value = secret_value.trim();
    if secret_value.len() < 8 {
        return Err(ToolError::new("Cloudflare secret value is too short."));
    }
    // Bound secret size (defense vs huge body).
    if secret_value.len() > 64 * 1024 {
        return Err(ToolError::new("Cloudflare secret value is too large."));
    }
    Ok((
        worker_name,
        secret_name,
        secret_value.to_string(),
    ))
}

/// Scaleway action validation matching Tauri `validate_scaleway_action_request`.
///
/// * Strict allowlist
/// * `terminate` rejected → use `delete` + confirm-by-name
/// * `delete` requires exact resource name match
pub fn validate_scaleway_action(
    resource_name: &str,
    action: &str,
    confirm_resource_name: Option<&str>,
) -> ToolResult<String> {
    let action = action.trim().to_ascii_lowercase();
    if !SCW_ALLOWED_ACTIONS.contains(&action.as_str()) {
        return Err(ToolError::new("Unsupported Scaleway resource action."));
    }
    if action == "terminate" {
        return Err(ToolError::new(
            "Use delete with exact resource-name confirmation for destructive Scaleway actions.",
        ));
    }
    if action == "delete" {
        let confirmed = confirm_resource_name
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ToolError::new(
                    "Deleting Scaleway resources requires exact resource-name confirmation.",
                )
            })?;
        if confirmed != resource_name {
            return Err(ToolError::new(
                "Scaleway delete confirmation does not match the resource name.",
            ));
        }
    }
    Ok(action)
}

/// Map agent-facing action → Scaleway instance API action.
pub fn scaleway_api_action(action: &str) -> &str {
    match action {
        "start" => "poweron",
        "stop" => "poweroff",
        "delete" => "terminate",
        other => other,
    }
}

/// Redact tokens/keys from provider error messages before surfacing to agents.
pub fn sanitize_provider_error(message: &str) -> String {
    static SCW_RE: OnceLock<Regex> = OnceLock::new();
    static BEARER_RE: OnceLock<Regex> = OnceLock::new();
    static AUTH_RE: OnceLock<Regex> = OnceLock::new();
    let scw = SCW_RE.get_or_init(|| Regex::new(r"SCW[A-Za-z0-9]{8,}").expect("scw re"));
    let bearer = BEARER_RE
        .get_or_init(|| Regex::new(r"Bearer\s+[^\s,;]+").expect("bearer re"));
    let auth = AUTH_RE
        .get_or_init(|| Regex::new(r"X-Auth-Token\s+[^\s,;]+").expect("auth re"));
    let text = scw.replace_all(message, "SCW[redacted]");
    let text = bearer.replace_all(&text, "Bearer [redacted]");
    auth.replace_all(&text, "X-Auth-Token [redacted]").into_owned()
}

fn app_vault_target(account: &str) -> String {
    format!("{account}.{APP_VAULT_SERVICE}")
}

// ── env / credentials (no secret values in return paths) ────────────────────

fn env_truthy(keys: &[&str]) -> bool {
    for key in keys {
        if let Ok(v) = std::env::var(key) {
            if v.trim() == "1" {
                return true;
            }
        }
    }
    false
}

fn cloudflare_profile_mode() -> bool {
    env_truthy(&[
        "DEVBOULE_MCP_CLOUDFLARE_PROFILE_MODE",
        "ASPIS_MCP_CLOUDFLARE_PROFILE_MODE",
    ])
}

fn provider_mutation_approval_enforced() -> bool {
    // Default ENFORCED. Opt-out only when explicitly set.
    !env_truthy(&[
        "DEVBOULE_MCP_ALLOW_SELF_ATTESTED_PROVIDER_MUTATION",
        "ASPIS_MCP_ALLOW_SELF_ATTESTED_PROVIDER_MUTATION",
    ])
}

/// First non-empty env value among `names` (never logged).
fn optional_env(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(v) = std::env::var(name) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// Credential status block — **never** includes secret values.
pub fn credential_status_for_env(account: &str, env_names: &[&str]) -> Value {
    let mut source = "missing".to_string();
    for env_name in env_names {
        if let Ok(v) = std::env::var(env_name) {
            if !v.trim().is_empty() {
                source = format!("env:{env_name}");
                break;
            }
        }
    }
    json!({
        "configured": source != "missing",
        "source": source,
        "target": app_vault_target(account),
        "envNames": env_names,
    })
}

fn require_token(env_names: &[&str], label: &str) -> ToolResult<String> {
    optional_env(env_names).ok_or_else(|| {
        ToolError::new(format!(
            "Missing provider token ({label}). Save it in Devboule > Secrets (app injects env), or set env var: {}",
            env_names.join(", ")
        ))
    })
}

fn cloudflare_read_token() -> ToolResult<String> {
    // Prefer profile-specific tokens, then general.
    let mut names: Vec<&str> = Vec::new();
    names.extend_from_slice(CF_TOKEN_ENVS);
    names.extend_from_slice(CF_READONLY_TOKEN_ENVS);
    names.extend_from_slice(CF_CODER_TOKEN_ENVS);
    require_token(&names, "Cloudflare")
}

fn cloudflare_rotate_token() -> ToolResult<String> {
    let mut names: Vec<&str> = Vec::new();
    names.extend_from_slice(CF_SECRET_ROTATOR_TOKEN_ENVS);
    names.extend_from_slice(CF_CODER_TOKEN_ENVS);
    names.extend_from_slice(CF_TOKEN_ENVS);
    require_token(&names, "Cloudflare secrets rotator")
}

fn scaleway_token() -> ToolResult<String> {
    require_token(SCW_TOKEN_ENVS, "Scaleway")
}

// ── claim / live-task guards for mutations ──────────────────────────────────

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

fn matching_active_claim_mut<'a>(
    state: &'a mut Value,
    agent_id: &str,
    role: &str,
    project_id: &str,
    task_id: &str,
) -> Option<&'a mut Value> {
    let claims = state.get_mut("claims")?.as_array_mut()?;
    claims.iter_mut().find(|claim| {
        claim.get("projectId").and_then(|v| v.as_str()) == Some(project_id)
            && claim.get("taskId").and_then(|v| v.as_str()) == Some(task_id)
            && claim.get("agentId").and_then(|v| v.as_str()) == Some(agent_id)
            && claim
                .get("role")
                .and_then(|v| v.as_str())
                .map(|r| normalize_role(r).ok().as_deref() == Some(role))
                .unwrap_or(false)
            && claim_is_active(claim)
    })
}

fn require_claim_for_provider_mutation(
    state: &Value,
    agent_id: &str,
    role: &str,
    project_id: &str,
    task_id: &str,
) -> ToolResult<()> {
    let claim = active_claim_for_task(state, project_id, task_id).ok_or_else(|| {
        ToolError::new("Agent must claim the task before updating status.")
    })?;
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
    let claim_role = normalize_role(claim.get("role").and_then(|v| v.as_str()).unwrap_or(""))?;
    if claim_role != role {
        return Err(ToolError::new(
            "Claim role does not match the registered agent role.",
        ));
    }
    Ok(())
}

fn task_provider_mutation_approver(task: &Value) -> Option<String> {
    for field in ["approvedBy", "approved_by", "providerMutationApprovedBy"] {
        if let Some(v) = task.get(field).and_then(|x| x.as_str()) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn require_live_task_for_provider_mutation(
    projects_dir: &Path,
    project_id: &str,
    task_id: &str,
) -> ToolResult<()> {
    let lock = project_lock_path(projects_dir, project_id)?;
    let path = project_path(projects_dir, project_id)?;
    with_file_lock(&lock, || {
        if !path.exists() {
            return Err(ToolError::new(
                "Provider mutations require a live task in the Management project.",
            ));
        }
        let project = read_project_file(&path)?;
        let project_status = project.metadata.status().to_string();
        if project_status != "active" {
            return Err(ToolError::new(format!(
                "Provider mutations require an active Management project \
                 (status={project_status:?}; only 'active' is allowed)."
            )));
        }
        let tasks = project
            .state
            .get("tasks")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let task = tasks
            .iter()
            .find(|t| t.get("id").and_then(|id| id.as_str()) == Some(task_id))
            .ok_or_else(|| {
                ToolError::new(
                    "Provider mutations require a live task in the Management project.",
                )
            })?;
        let task_status = task.get("status").and_then(|s| s.as_str()).unwrap_or("");
        if !matches!(task_status, "wip" | "blocked") {
            return Err(ToolError::new(
                "Provider mutations require the live task to be wip or blocked.",
            ));
        }
        if provider_mutation_approval_enforced() && task_provider_mutation_approver(task).is_none()
        {
            return Err(ToolError::new(
                "Provider mutations require a non-coder approval marker (approvedBy) on the \
                 task. A verifier or human must approve the destructive action before a coder \
                 can rotate secrets or terminate compute.",
            ));
        }
        Ok(())
    })
}

/// Product rule: only coders mutate CF/SCW (F-04-020).
pub fn require_provider_mutation_role(role: &str) -> ToolResult<()> {
    if normalize_role(role)? != "coder" {
        return Err(ToolError::new(
            "Only coder agents can mutate Cloudflare or Scaleway. \
             Orchestrators and verifiers are read-only for provider mutations.",
        ));
    }
    Ok(())
}

fn provider_mutation_project_context(
    management_project_id: Option<&str>,
    aspis_project_id: Option<&str>,
    task_id: Option<&str>,
    evidence: Option<&str>,
) -> ToolResult<(String, String, String)> {
    let project_id = management_project_id
        .filter(|s| !s.trim().is_empty())
        .or(aspis_project_id.filter(|s| !s.trim().is_empty()))
        .ok_or_else(|| {
            ToolError::new(
                "Provider mutations require management_project_id and task_id so the Kanban can audit the action.",
            )
        })?;
    let project_id = normalize_project_id(project_id)?;
    let task_id = normalize_task_id(task_id.unwrap_or(""))?;
    let evidence = evidence.unwrap_or("").trim();
    if evidence.len() < 12 {
        return Err(ToolError::new(
            "Provider mutations require concrete evidence.",
        ));
    }
    let evidence: String = evidence.chars().take(2000).collect();
    Ok((project_id, task_id, evidence))
}

struct MutationContext {
    agent_id: String,
    role: String,
    project_id: String,
    task_id: String,
    evidence: String,
}

fn require_provider_mutation_context(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    tool_name: &str,
    session_token: Option<&str>,
    management_project_id: Option<&str>,
    aspis_project_id: Option<&str>,
    task_id: Option<&str>,
    evidence: Option<&str>,
) -> ToolResult<MutationContext> {
    let (agent_id, role) =
        require_agent_tool(projects_dir, agent_id, role, tool_name, session_token)?;
    require_provider_mutation_role(&role)?;
    let (project_id, task_id, evidence) = provider_mutation_project_context(
        management_project_id,
        aspis_project_id,
        task_id,
        evidence,
    )?;
    with_agents_lock(projects_dir, || {
        let state = read_agents_state(projects_dir)?;
        require_claim_for_provider_mutation(&state, &agent_id, &role, &project_id, &task_id)?;
        require_live_task_for_provider_mutation(projects_dir, &project_id, &task_id)?;
        Ok::<(), ToolError>(())
    })?;
    Ok(MutationContext {
        agent_id,
        role,
        project_id,
        task_id,
        evidence,
    })
}

fn reserve_provider_mutation(
    projects_dir: &Path,
    ctx: &MutationContext,
    tool_name: &str,
) -> ToolResult<()> {
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        require_claim_for_provider_mutation(
            &state,
            &ctx.agent_id,
            &ctx.role,
            &ctx.project_id,
            &ctx.task_id,
        )?;
        require_live_task_for_provider_mutation(projects_dir, &ctx.project_id, &ctx.task_id)?;
        if let Some(claim) = matching_active_claim_mut(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            &ctx.project_id,
            &ctx.task_id,
        ) {
            if let Some(obj) = claim.as_object_mut() {
                obj.insert("status".into(), json!("provider_action_pending"));
                obj.insert("evidence".into(), json!(&ctx.evidence));
                obj.insert("updatedAt".into(), json!(now_rfc3339()));
            }
        }
        upsert_session(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            None,
            "provider_action_pending",
            Some(&format!("{tool_name} pending.")),
            None,
            None,
            None,
            Some(&ctx.project_id),
            Some(&ctx.task_id),
        )?;
        add_event(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            "provider_action_pending",
            &format!("{tool_name} authorized."),
            Some(&ctx.project_id),
            Some(&ctx.task_id),
            None,
            Some(&ctx.evidence),
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })
}

fn release_provider_mutation_reservation(
    projects_dir: &Path,
    ctx: &MutationContext,
    tool_name: &str,
    reason: &str,
) -> ToolResult<()> {
    let reason: String = reason.chars().take(240).collect();
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        if let Some(claim) = matching_active_claim_mut(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            &ctx.project_id,
            &ctx.task_id,
        ) {
            if claim.get("status").and_then(|s| s.as_str()) == Some("provider_action_pending") {
                if let Some(obj) = claim.as_object_mut() {
                    obj.insert("status".into(), json!("wip"));
                    obj.insert("updatedAt".into(), json!(now_rfc3339()));
                }
            }
        }
        upsert_session(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            None,
            "wip",
            Some(&format!("{tool_name} failed: {reason}")),
            None,
            None,
            None,
            Some(&ctx.project_id),
            Some(&ctx.task_id),
        )?;
        add_event(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            "provider_action_failed",
            &format!("{tool_name} failed: {reason}"),
            Some(&ctx.project_id),
            Some(&ctx.task_id),
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })
}

fn record_provider_mutation(
    projects_dir: &Path,
    ctx: &MutationContext,
    event_type: &str,
    message: &str,
) -> ToolResult<()> {
    with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        require_claim_for_provider_mutation(
            &state,
            &ctx.agent_id,
            &ctx.role,
            &ctx.project_id,
            &ctx.task_id,
        )?;
        // Append note on the management project (single project lock — do not
        // call load_project_locked which would re-lock the same path).
        let lock = project_lock_path(projects_dir, &ctx.project_id)?;
        with_file_lock(&lock, || {
            let path = project_path(projects_dir, &ctx.project_id)?;
            if !path.exists() {
                return Err(ToolError::new("Task not found."));
            }
            let mut project = read_project_file(&path)?;
            let tasks = project
                .state
                .get("tasks")
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default();
            if !tasks
                .iter()
                .any(|t| t.get("id").and_then(|id| id.as_str()) == Some(ctx.task_id.as_str()))
            {
                return Err(ToolError::new("Task not found."));
            }
            let notes = project
                .state
                .as_object_mut()
                .unwrap()
                .entry("notes".to_string())
                .or_insert_with(|| json!([]));
            if let Some(arr) = notes.as_array_mut() {
                arr.push(json!({
                    "id": note_id(),
                    "text": format!("{} Evidence: {}", message, ctx.evidence),
                    "source": format!("agent:{}", ctx.agent_id),
                    "createdAt": now_rfc3339(),
                }));
            }
            project.metadata.set("updated_at", now_rfc3339());
            write_project_file(projects_dir, project)?;
            Ok(())
        })?;
        if let Some(claim) = matching_active_claim_mut(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            &ctx.project_id,
            &ctx.task_id,
        ) {
            if claim.get("status").and_then(|s| s.as_str()) == Some("provider_action_pending") {
                if let Some(obj) = claim.as_object_mut() {
                    obj.insert("status".into(), json!("wip"));
                    obj.insert("updatedAt".into(), json!(now_rfc3339()));
                }
            }
        }
        upsert_session(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            None,
            event_type,
            Some(message),
            None,
            None,
            None,
            Some(&ctx.project_id),
            Some(&ctx.task_id),
        )?;
        add_event(
            &mut state,
            &ctx.agent_id,
            &ctx.role,
            event_type,
            message,
            Some(&ctx.project_id),
            Some(&ctx.task_id),
            None,
            Some(&ctx.evidence),
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    })
}

// ── HTTP transport ──────────────────────────────────────────────────────────

fn http_client() -> ToolResult<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| ToolError::new(format!("Could not build HTTP client: {e}")))
}

fn raise_for_status(response: reqwest::blocking::Response, label: &str) -> ToolResult<reqwest::blocking::Response> {
    let status = response.status();
    if status.is_success() || status.as_u16() == 404 {
        // Caller decides 404 handling for deletes.
        if status.is_success() {
            return Ok(response);
        }
    }
    if !status.is_success() {
        let url = response.url().to_string();
        return Err(ToolError::new(sanitize_provider_error(&format!(
            "{label} rejected with HTTP {status}: {url}"
        ))));
    }
    Ok(response)
}

fn api_get(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(&str, &str)],
    query: &[(&str, String)],
) -> ToolResult<Value> {
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if !query.is_empty() {
        req = req.query(query);
    }
    let response = req
        .send()
        .map_err(|e| ToolError::new(sanitize_provider_error(&format!("Provider GET failed: {e}"))))?;
    let response = raise_for_status(response, "Provider GET")?;
    if response.status().as_u16() == 404 {
        return Ok(json!({}));
    }
    response
        .json()
        .map_err(|e| ToolError::new(format!("Provider GET JSON decode failed: {e}")))
}

fn api_put_json(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> ToolResult<Value> {
    let mut req = client.put(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let response = req
        .json(body)
        .send()
        .map_err(|e| ToolError::new(sanitize_provider_error(&format!("Provider PUT failed: {e}"))))?;
    let response = raise_for_status(response, "Provider PUT")?;
    if response.content_length() == Some(0) {
        return Ok(json!({}));
    }
    let text = response
        .text()
        .map_err(|e| ToolError::new(format!("Provider PUT body read failed: {e}")))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text)
        .map_err(|e| ToolError::new(format!("Provider PUT JSON decode failed: {e}")))
}

fn api_post_json(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> ToolResult<Value> {
    let mut req = client.post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let response = req
        .json(body)
        .send()
        .map_err(|e| {
            ToolError::new(sanitize_provider_error(&format!("Provider POST failed: {e}")))
        })?;
    let response = raise_for_status(response, "Provider POST")?;
    let text = response
        .text()
        .map_err(|e| ToolError::new(format!("Provider POST body read failed: {e}")))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text)
        .map_err(|e| ToolError::new(format!("Provider POST JSON decode failed: {e}")))
}

fn api_delete(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(&str, &str)],
    query: &[(&str, String)],
) -> ToolResult<u16> {
    let status = api_delete_status(client, url, headers, query)?;
    if provider_http_status_ok(status) {
        return Ok(status);
    }
    Err(ToolError::new(format!(
        "Provider DELETE rejected with HTTP {status}."
    )))
}

/// DELETE that returns status without treating non-2xx as error (network fails still error).
/// Used by Scaleway delete cascade so terminate/poweroff fallbacks can run (Tauri parity).
fn api_delete_status(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(&str, &str)],
    query: &[(&str, String)],
) -> ToolResult<u16> {
    let mut req = client.delete(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if !query.is_empty() {
        req = req.query(query);
    }
    let response = req.send().map_err(|e| {
        // Static-ish: sanitize so any URL-embedded secrets in reqwest Display are redacted.
        ToolError::new(sanitize_provider_error(&format!("Provider DELETE failed: {e}")))
    })?;
    Ok(response.status().as_u16())
}

/// POST that returns status without treating non-2xx as error (network fails still error).
fn api_post_status(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> ToolResult<u16> {
    let mut req = client.post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let response = req.json(body).send().map_err(|e| {
        ToolError::new(sanitize_provider_error(&format!("Provider POST failed: {e}")))
    })?;
    Ok(response.status().as_u16())
}

/// IAM api-key self-check with **static** error messages.
///
/// B2 (Tauri parity): the access key is in the URL path; reqwest's error Display
/// echoes the URL, so never interpolate `{e}` / URL into the agent-facing error.
fn scaleway_api_key_self_check(
    client: &reqwest::blocking::Client,
    token: &str,
    access_key: &str,
) -> ToolResult<Option<String>> {
    let headers = scw_headers(token);
    let href = [
        (headers[0].0, headers[0].1.as_str()),
        (headers[1].0, headers[1].1.as_str()),
    ];
    // Build request inline so failures use static messages (no URL with access key).
    let url = format!("{SCW_API}/iam/v1alpha1/api-keys/{access_key}");
    let mut req = client.get(&url);
    for (k, v) in &href {
        req = req.header(*k, *v);
    }
    let response = req
        .send()
        .map_err(|_| ToolError::new("Scaleway API key self-check request failed."))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ToolError::new("Scaleway API key self-check rejected."));
    }
    let info: Value = response
        .json()
        .map_err(|_| ToolError::new("Scaleway API key self-check response was invalid."))?;
    Ok(info
        .get("default_project_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}

fn cf_headers(token: &str) -> [(&'static str, String); 2] {
    [
        ("Authorization", format!("Bearer {token}")),
        ("Content-Type", "application/json".into()),
    ]
}

fn cf_headers_ref<'a>(headers: &'a [(&'static str, String); 2]) -> [(&'a str, &'a str); 2] {
    [(headers[0].0, headers[0].1.as_str()), (headers[1].0, headers[1].1.as_str())]
}

fn scw_headers(token: &str) -> [(&'static str, String); 2] {
    [
        ("X-Auth-Token", token.to_string()),
        ("Content-Type", "application/json".into()),
    ]
}

fn cf_result(envelope: &Value) -> ToolResult<Value> {
    if envelope.get("success") == Some(&json!(false)) {
        return Err(ToolError::new("Cloudflare API rejected the request."));
    }
    Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
}

// ── Cloudflare ──────────────────────────────────────────────────────────────

fn resolve_cloudflare_account(
    client: &reqwest::blocking::Client,
    token: &str,
    requested_account_id: Option<&str>,
) -> ToolResult<Value> {
    let headers = cf_headers(token);
    let href = cf_headers_ref(&headers);
    let envelope = api_get(client, &format!("{CF_API}/accounts"), &href, &[])?;
    let accounts = cf_result(&envelope)?;
    let accounts = accounts.as_array().cloned().unwrap_or_default();
    let requested = requested_account_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| optional_env(CF_ACCOUNT_ENVS));
    if let Some(req_id) = requested {
        for account in &accounts {
            if account.get("id").and_then(|v| v.as_str()) == Some(req_id.as_str()) {
                let name = account
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // Fail closed: pinned id alone is not enough — name must normalize to aspis-bio.
                require_pinned_provider_name(&name, CF_TARGET_ACCOUNT_NAME, "Cloudflare account")?;
                return Ok(json!({"id": req_id, "name": name}));
            }
        }
        return Err(ToolError::new(
            "Pinned Cloudflare Devboule account was not visible to this token.",
        ));
    }
    let matches: Vec<&Value> = accounts
        .iter()
        .filter(|item| {
            normalize_provider_name(item.get("name").and_then(|v| v.as_str()).unwrap_or(""))
                == CF_TARGET_ACCOUNT_NAME
        })
        .collect();
    if matches.len() != 1 {
        if accounts.len() == 1 {
            let account = &accounts[0];
            let name = account
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            require_pinned_provider_name(&name, CF_TARGET_ACCOUNT_NAME, "Cloudflare account")?;
            return Ok(json!({
                "id": account.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                "name": name,
            }));
        }
        return Err(ToolError::new(
            "Cloudflare Devboule account is ambiguous or missing. Set DEVBOULE_CLOUDFLARE_ACCOUNT_ID.",
        ));
    }
    Ok(json!({
        "id": matches[0].get("id").and_then(|v| v.as_str()).unwrap_or(""),
        "name": matches[0].get("name").and_then(|v| v.as_str()).unwrap_or(CF_TARGET_ACCOUNT_NAME),
    }))
}

fn cloudflare_list_workers_http(
    client: &reqwest::blocking::Client,
    token: &str,
    account_id: Option<&str>,
) -> ToolResult<Value> {
    let account = resolve_cloudflare_account(client, token, account_id)?;
    let acc_id = account
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let headers = cf_headers(token);
    let href = cf_headers_ref(&headers);
    let envelope = api_get(
        client,
        &format!("{CF_API}/accounts/{acc_id}/workers/scripts"),
        &href,
        &[],
    )?;
    let workers = cf_result(&envelope)?;
    let workers = workers.as_array().cloned().unwrap_or_default();
    let mut safe_workers = Vec::new();
    let mut hidden_sibling_workers = 0u64;
    for worker in workers {
        let name = worker
            .get("id")
            .or_else(|| worker.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let routes = worker
            .get("routes")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if !cloudflare_worker_in_scope(name, &routes) {
            hidden_sibling_workers += 1;
            continue;
        }
        let tags = worker.get("tags").cloned().unwrap_or_else(|| json!([]));
        let route_patterns: Vec<Value> = routes
            .iter()
            .filter_map(|r| {
                r.get("pattern")
                    .and_then(|p| p.as_str())
                    .map(|s| json!(s))
            })
            .collect();
        safe_workers.push(json!({
            "id": name,
            "name": name,
            "createdOn": worker.get("created_on"),
            "modifiedOn": worker.get("modified_on"),
            "usageModel": worker.get("usage_model"),
            "routes": route_patterns,
            "compatibilityDate": worker.get("compatibility_date"),
            "tags": tags,
        }));
    }
    Ok(json!({
        "account": account,
        "workers": safe_workers,
        "hiddenSiblingWorkers": hidden_sibling_workers,
    }))
}

fn cloudflare_rotate_secret_http(
    client: &reqwest::blocking::Client,
    token: &str,
    account_id: Option<&str>,
    worker_name: &str,
    secret_name: &str,
    secret_value: &str,
) -> ToolResult<Value> {
    let (worker_name, secret_name, secret_value) =
        validate_cloudflare_secret_rotation(worker_name, secret_name, secret_value)?;
    let inventory = cloudflare_list_workers_http(client, token, account_id)?;
    let account = inventory.get("account").cloned().unwrap_or(json!({}));
    let workers = inventory
        .get("workers")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    if !workers
        .iter()
        .any(|w| w.get("name").and_then(|n| n.as_str()) == Some(worker_name.as_str()))
    {
        return Err(ToolError::new(
            "Worker is not in the Devboule Cloudflare inventory.",
        ));
    }
    let acc_id = account.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let encoded_worker = worker_name.replace('/', "%2F");
    let url = format!("{CF_API}/accounts/{acc_id}/workers/scripts/{encoded_worker}/secrets");
    let headers = cf_headers(token);
    let href = cf_headers_ref(&headers);
    let payload = json!({
        "name": secret_name,
        "text": secret_value,
        "type": "secret_text",
    });
    let envelope = api_put_json(client, &url, &href, &payload)?;
    let _ = cf_result(&envelope)?;
    // Never return secret_value.
    Ok(json!({
        "account": account,
        "workerName": worker_name,
        "secretName": secret_name,
        "rotatedAt": now_rfc3339(),
    }))
}

// ── Scaleway ────────────────────────────────────────────────────────────────

fn resolve_scaleway_project(
    client: &reqwest::blocking::Client,
    token: &str,
    requested_project_id: Option<&str>,
) -> ToolResult<Value> {
    let headers = scw_headers(token);
    let href = [
        (headers[0].0, headers[0].1.as_str()),
        (headers[1].0, headers[1].1.as_str()),
    ];
    let requested = requested_project_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| optional_env(SCW_PROJECT_ENVS));

    let projects_payload = api_get(client, &format!("{SCW_API}/account/v3/projects"), &href, &[]);
    let projects = match projects_payload {
        Ok(payload) => payload
            .get("projects")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default(),
        Err(list_err) => {
            // Without the project list we cannot prove the project name is aspis-bio.
            // IAM self-check only refines diagnostics (static messages — never echo
            // the access-key URL). Never invent name=aspis-bio from a bare id match.
            if let Some(ref req) = requested {
                if let Some(access_key) = optional_env(SCW_OBJECT_ACCESS_KEY_ENVS) {
                    match scaleway_api_key_self_check(client, token, &access_key) {
                        Ok(Some(default_project_id)) if default_project_id == *req => {
                            return Err(ToolError::new(
                                "Scaleway project list was not readable; refusing to pin project \
without verifying its name is aspis-bio.",
                            ));
                        }
                        Ok(Some(_)) => {
                            return Err(ToolError::new(
                                "Scaleway API key default project does not match the pinned Devboule project.",
                            ));
                        }
                        Ok(None) => {
                            return Err(ToolError::new(
                                "Scaleway API key has no default project to verify against Devboule.",
                            ));
                        }
                        Err(iam_err) => return Err(iam_err),
                    }
                }
            }
            return Err(list_err);
        }
    };

    if let Some(req_id) = requested {
        for project in &projects {
            if project.get("id").and_then(|v| v.as_str()) == Some(req_id.as_str()) {
                let name = project
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                require_pinned_provider_name(&name, SCW_TARGET_PROJECT_NAME, "Scaleway project")?;
                return Ok(json!({"id": project.get("id"), "name": name}));
            }
        }
        return Err(ToolError::new(
            "Pinned Scaleway Devboule project was not visible to this token.",
        ));
    }
    let matches: Vec<&Value> = projects
        .iter()
        .filter(|item| {
            normalize_provider_name(item.get("name").and_then(|v| v.as_str()).unwrap_or(""))
                == SCW_TARGET_PROJECT_NAME
        })
        .collect();
    if matches.len() != 1 {
        return Err(ToolError::new(
            "Scaleway Devboule project is ambiguous or missing. Set DEVBOULE_SCALEWAY_PROJECT_ID.",
        ));
    }
    Ok(json!({
        "id": matches[0].get("id"),
        "name": matches[0].get("name").and_then(|v| v.as_str()).unwrap_or(SCW_TARGET_PROJECT_NAME),
    }))
}

fn scw_items_or_empty(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(&str, &str)],
    query: &[(&str, String)],
    envelope: &str,
) -> Vec<Value> {
    match api_get(client, url, headers, query) {
        Ok(payload) if payload.is_object() => payload
            .get(envelope)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter(|i| i.is_object()).cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn scaleway_list_resources_http(
    client: &reqwest::blocking::Client,
    token: &str,
    project_id: Option<&str>,
) -> ToolResult<Value> {
    let project = resolve_scaleway_project(client, token, project_id)?;
    let proj_id = project
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let headers = scw_headers(token);
    let href = [
        (headers[0].0, headers[0].1.as_str()),
        (headers[1].0, headers[1].1.as_str()),
    ];
    let mut resources: Vec<Value> = Vec::new();

    for zone in SCW_ZONES {
        let url = format!("{SCW_API}/instance/v1/zones/{zone}/servers");
        let q: Vec<(&str, String)> = vec![
            ("project", proj_id.clone()),
            ("page", "1".into()),
            ("per_page", "100".into()),
        ];
        let payload = match api_get(client, &url, &href, &q) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for server in payload
            .get("servers")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default()
        {
            resources.push(json!({
                "id": server.get("id"),
                "name": server.get("name"),
                "resourceType": "instance_server",
                "region": zone,
                "state": server.get("state"),
                "commercialType": server.get("commercial_type"),
                "projectId": proj_id,
                "availableActions": ["start", "stop", "reboot", "delete", "terminate"],
            }));
        }
    }

    for region in SCW_REGIONS {
        let ns_q = vec![
            ("project_id", proj_id.clone()),
            ("page", "1".into()),
            ("page_size", "100".into()),
        ];
        let namespaces = scw_items_or_empty(
            client,
            &format!("{SCW_API}/functions/v1beta1/regions/{region}/namespaces"),
            &href,
            &ns_q,
            "namespaces",
        );
        for namespace in namespaces {
            let ns_id = namespace
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let fn_q = vec![
                ("namespace_id", ns_id.clone()),
                ("project_id", proj_id.clone()),
                ("page", "1".into()),
                ("page_size", "100".into()),
            ];
            for item in scw_items_or_empty(
                client,
                &format!("{SCW_API}/functions/v1beta1/regions/{region}/functions"),
                &href,
                &fn_q,
                "functions",
            ) {
                resources.push(json!({
                    "id": item.get("id"),
                    "name": item.get("name"),
                    "resourceType": "serverless_function",
                    "region": region,
                    "state": item.get("status").or_else(|| item.get("state")),
                    "runtime": item.get("runtime"),
                    "projectId": proj_id,
                    "namespaceId": ns_id,
                    "availableActions": ["deploy"],
                }));
            }
        }
        let c_q = vec![
            ("project_id", proj_id.clone()),
            ("page", "1".into()),
            ("page_size", "100".into()),
        ];
        for item in scw_items_or_empty(
            client,
            &format!("{SCW_API}/containers/v1beta1/regions/{region}/containers"),
            &href,
            &c_q,
            "containers",
        ) {
            resources.push(json!({
                "id": item.get("id"),
                "name": item.get("name"),
                "resourceType": "serverless_container",
                "region": region,
                "state": item.get("status").or_else(|| item.get("state")),
                "runtime": item.get("runtime"),
                "projectId": proj_id,
                "availableActions": ["deploy"],
            }));
        }
    }

    for zone in SCW_ZONES {
        let zone_params = vec![
            ("project_id", proj_id.clone()),
            ("page", "1".into()),
            ("per_page", "100".into()),
        ];
        for item in scw_items_or_empty(
            client,
            &format!("{SCW_API}/block/v1/zones/{zone}/volumes"),
            &href,
            &zone_params,
            "volumes",
        ) {
            resources.push(json!({
                "id": item.get("id"),
                "name": item.get("name"),
                "resourceType": "block_volume",
                "region": zone,
                "state": item.get("status").or_else(|| item.get("state")),
                "projectId": proj_id,
                "availableActions": [],
            }));
        }
        for item in scw_items_or_empty(
            client,
            &format!("{SCW_API}/block/v1/zones/{zone}/snapshots"),
            &href,
            &zone_params,
            "snapshots",
        ) {
            resources.push(json!({
                "id": item.get("id"),
                "name": item.get("name"),
                "resourceType": "block_snapshot",
                "region": zone,
                "state": item.get("status").or_else(|| item.get("state")),
                "projectId": proj_id,
                "availableActions": [],
            }));
        }
    }

    for region in SCW_REGIONS {
        let region_params = vec![
            ("project_id", proj_id.clone()),
            ("page", "1".into()),
            ("page_size", "100".into()),
        ];
        for item in scw_items_or_empty(
            client,
            &format!("{SCW_API}/file/v1alpha1/regions/{region}/filesystems"),
            &href,
            &region_params,
            "filesystems",
        ) {
            resources.push(json!({
                "id": item.get("id"),
                "name": item.get("name"),
                "resourceType": "file_system",
                "region": region,
                "state": item.get("status").or_else(|| item.get("state")),
                "projectId": proj_id,
                "availableActions": [],
            }));
        }
        for item in scw_items_or_empty(
            client,
            &format!("{SCW_API}/serverless-sqldb/v1alpha1/regions/{region}/databases"),
            &href,
            &region_params,
            "databases",
        ) {
            // DSN/endpoint deliberately NOT emitted — may carry credentials.
            resources.push(json!({
                "id": item.get("id"),
                "name": item.get("name"),
                "resourceType": "serverless_sql_database",
                "region": region,
                "state": item.get("status").or_else(|| item.get("state")),
                "projectId": proj_id,
                "availableActions": [],
            }));
        }
    }

    let resources: Vec<Value> = resources
        .into_iter()
        .filter(|item| {
            item.get("id")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
                || item.get("id").and_then(|v| v.as_str()).is_some()
        })
        .filter(|item| item.get("id").is_some() && !item.get("id").unwrap().is_null())
        .collect();

    Ok(json!({
        "project": project,
        "resources": resources,
    }))
}

fn delete_scaleway_instance_with_volumes(
    client: &reqwest::blocking::Client,
    token: &str,
    zone: &str,
    server_id: &str,
) -> ToolResult<()> {
    let headers = scw_headers(token);
    let href = [
        (headers[0].0, headers[0].1.as_str()),
        (headers[1].0, headers[1].1.as_str()),
    ];
    // C2: FAILED volume lookup is NOT "no volumes". Abort so attached volumes
    // are not orphaned (and keep billing) when inventory is unreachable.
    let volume_ids = match api_get(
        client,
        &format!("{SCW_API}/instance/v1/zones/{zone}/servers/{server_id}"),
        &href,
        &[],
    ) {
        Ok(payload) => scaleway_volume_ids_from_server_payload(&payload),
        Err(e) => {
            return Err(refuse_delete_on_volume_lookup_failure(server_id, &e.message));
        }
    };

    let delete_url = format!("{SCW_API}/instance/v1/zones/{zone}/servers/{server_id}");
    let params = vec![
        ("with_volumes", "all".into()),
        ("with_ip", "true".into()),
        ("force_shutdown", "true".into()),
    ];
    // Capture status — do NOT use `?` on non-2xx or the terminate/poweroff cascade is lost.
    let delete_status = api_delete_status(client, &delete_url, &href, &params)?;
    if provider_http_status_ok(delete_status) {
        delete_scaleway_instance_volumes(client, &href, zone, &volume_ids)?;
        return Ok(());
    }

    let action_url = format!("{SCW_API}/instance/v1/zones/{zone}/servers/{server_id}/action");
    let term_status = api_post_status(
        client,
        &action_url,
        &href,
        &json!({"action": "terminate"}),
    )?;
    if provider_http_status_ok(term_status) {
        delete_scaleway_instance_volumes(client, &href, zone, &volume_ids)?;
        return Ok(());
    }

    // Best-effort poweroff, then final delete (must succeed).
    let _ = api_post_status(
        client,
        &action_url,
        &href,
        &json!({"action": "poweroff"}),
    );
    let params2 = vec![
        ("with_volumes", "all".into()),
        ("with_ip", "true".into()),
    ];
    // Final delete: non-2xx is a hard error (api_delete enforces).
    api_delete(client, &delete_url, &href, &params2)?;
    // Volume cleanup failures must surface — do not claim full success if orphans remain.
    delete_scaleway_instance_volumes(client, &href, zone, &volume_ids)?;
    Ok(())
}

fn delete_scaleway_instance_volumes(
    client: &reqwest::blocking::Client,
    headers: &[(&str, &str)],
    zone: &str,
    volume_ids: &[String],
) -> ToolResult<()> {
    for volume_id in volume_ids {
        let safe = clean_text(volume_id, "Volume id", 160)?;
        let url = format!("{SCW_API}/instance/v1/zones/{zone}/volumes/{safe}");
        // Non-success (except 404) propagates — caller must not report success with orphans.
        api_delete(client, &url, headers, &[])?;
    }
    Ok(())
}

fn scaleway_resource_action_http(
    client: &reqwest::blocking::Client,
    token: &str,
    resource_id: &str,
    action: &str,
    confirm_resource_name: Option<&str>,
    project_id: Option<&str>,
) -> ToolResult<Value> {
    let resource_id = clean_text(resource_id, "Resource id", 160)?;
    let inventory = scaleway_list_resources_http(client, token, project_id)?;
    let resources = inventory
        .get("resources")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let resource = resources
        .iter()
        .find(|item| item.get("id").and_then(|v| v.as_str()) == Some(resource_id.as_str()))
        .ok_or_else(|| ToolError::new("Scaleway resource is not in the Devboule inventory."))?;

    let resource_name = resource
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let action = validate_scaleway_action(&resource_name, action, confirm_resource_name)?;

    let available = resource
        .get("availableActions")
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    // Check agent-facing action (and mapped alias) against inventory.
    let api_action = scaleway_api_action(&action);
    let allowed = available.iter().any(|a| {
        a.as_str()
            .map(|s| s.eq_ignore_ascii_case(&action) || s.eq_ignore_ascii_case(api_action))
            .unwrap_or(false)
    });
    if !allowed {
        return Err(ToolError::new(
            "Scaleway action is not available for this resource type.",
        ));
    }

    let resource_type = resource
        .get("resourceType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let region = resource
        .get("region")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let headers = scw_headers(token);
    let href = [
        (headers[0].0, headers[0].1.as_str()),
        (headers[1].0, headers[1].1.as_str()),
    ];

    match resource_type.as_str() {
        "instance_server" => {
            if action == "delete" {
                delete_scaleway_instance_with_volumes(client, token, &region, &resource_id)?;
            } else {
                let url = format!(
                    "{SCW_API}/instance/v1/zones/{region}/servers/{resource_id}/action"
                );
                let _ = api_post_json(
                    client,
                    &url,
                    &href,
                    &json!({"action": api_action}),
                )?;
            }
        }
        "serverless_function" => {
            let url = format!(
                "{SCW_API}/functions/v1beta1/regions/{region}/functions/{resource_id}/deploy"
            );
            let _ = api_post_json(client, &url, &href, &json!({}))?;
        }
        "serverless_container" => {
            let url = format!(
                "{SCW_API}/containers/v1beta1/regions/{region}/containers/{resource_id}/deploy"
            );
            let _ = api_post_json(client, &url, &href, &json!({}))?;
        }
        _ => {
            return Err(ToolError::new("Unsupported Scaleway resource type."));
        }
    }

    Ok(json!({
        "project": inventory.get("project"),
        "resourceId": resource_id,
        "resourceName": resource_name,
        "resourceType": resource_type,
        "action": action,
        "triggeredAt": now_rfc3339(),
    }))
}

// ── public tool entry points ────────────────────────────────────────────────

/// Read-only credential readiness. Never returns secret values.
pub fn provider_credentials_status(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "provider_credentials_status",
        session_token,
    )?;

    // Tokens: env only (app injects). Profile mode still reports env sources.
    let cf_token_envs: Vec<&str> = {
        let mut v = Vec::new();
        v.extend_from_slice(CF_TOKEN_ENVS);
        v.extend_from_slice(CF_READONLY_TOKEN_ENVS);
        v.extend_from_slice(CF_CODER_TOKEN_ENVS);
        v.extend_from_slice(CF_SECRET_ROTATOR_TOKEN_ENVS);
        v
    };
    // In profile mode, general CF token status is env-only (same as Python).
    let _ = cloudflare_profile_mode();

    let body = json!({
        "providers": {
            "cloudflare": {
                "targetName": CF_TARGET_ACCOUNT_NAME,
                "token": credential_status_for_env("provider:cloudflare", &cf_token_envs),
                "accountId": credential_status_for_env("scope:cloudflare_account_id", CF_ACCOUNT_ENVS),
                "agentProfiles": {
                    "verifierReadonly": credential_status_for_env(
                        "provider:cloudflare_agent_profile:verifier-readonly",
                        CF_READONLY_TOKEN_ENVS,
                    ),
                    "coderWorkerWrite": credential_status_for_env(
                        "provider:cloudflare_agent_profile:coder-worker-write",
                        CF_CODER_TOKEN_ENVS,
                    ),
                    "secretsRotator": credential_status_for_env(
                        "provider:cloudflare_agent_profile:secrets-rotator",
                        CF_SECRET_ROTATOR_TOKEN_ENVS,
                    ),
                },
            },
            "scaleway": {
                "targetProjectName": SCW_TARGET_PROJECT_NAME,
                "token": credential_status_for_env("provider:scaleway", SCW_TOKEN_ENVS),
                "projectId": credential_status_for_env("scope:scaleway_project_id", SCW_PROJECT_ENVS),
                "objectAccessKey": credential_status_for_env(
                    "aux:scaleway_object_access_key",
                    SCW_OBJECT_ACCESS_KEY_ENVS,
                ),
                "objectSecretKey": credential_status_for_env(
                    "aux:scaleway_object_secret_key",
                    SCW_OBJECT_SECRET_KEY_ENVS,
                ),
            },
            // GitHub: vault-only in the app; MCP process does not read keyring in P5.
            // Report configured=false unless a future env is introduced. Never emit token.
            "github": {
                "configured": false,
                "source": "missing",
                "target": app_vault_target("provider:github"),
            },
        },
        "oracleLlm": {
            "settingsConfigured": false,
            "settingsTarget": app_vault_target("oracle:llm_settings"),
            "note": "Oracle LLM settings live in the app vault; this MCP reports env-injected cloud tokens only (P5).",
        },
    });

    // Audit read (best-effort).
    let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            "provider-credentials",
            Some("Read provider credential readiness without secrets."),
            None,
            None,
            None,
            None,
            None,
        )?;
        add_event(
            &mut state,
            &agent_id,
            &role,
            "provider_credentials_status",
            "Read provider credential readiness without secrets.",
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    });

    Ok(body)
}

pub fn cloudflare_list_workers(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    account_id: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "cloudflare_list_workers",
        session_token,
    )?;
    let token = cloudflare_read_token()?;
    let client = http_client()?;
    let result = cloudflare_list_workers_http(&client, &token, account_id)?;
    let n = result
        .get("workers")
        .and_then(|w| w.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            "cloudflare-read",
            Some("Read Cloudflare Workers inventory."),
            None,
            None,
            None,
            None,
            None,
        )?;
        add_event(
            &mut state,
            &agent_id,
            &role,
            "cloudflare_read",
            &format!("Read {n} Cloudflare Workers."),
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    });
    Ok(result)
}

pub fn cloudflare_rotate_worker_secret(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    worker_name: &str,
    secret_name: &str,
    secret_value: &str,
    account_id: Option<&str>,
    management_project_id: Option<&str>,
    aspis_project_id: Option<&str>,
    task_id: Option<&str>,
    evidence: Option<&str>,
) -> ToolResult<Value> {
    let ctx = require_provider_mutation_context(
        projects_dir,
        agent_id,
        role,
        "cloudflare_rotate_worker_secret",
        session_token,
        management_project_id,
        aspis_project_id,
        task_id,
        evidence,
    )?;
    // Validate inputs early (before token / HTTP) so secret never hits network
    // when scope/name is wrong. Does not log secret_value.
    let _ = validate_cloudflare_secret_rotation(worker_name, secret_name, secret_value)?;
    let token = cloudflare_rotate_token()?;
    reserve_provider_mutation(projects_dir, &ctx, "cloudflare_rotate_worker_secret")?;
    let client = http_client()?;
    let result = match cloudflare_rotate_secret_http(
        &client,
        &token,
        account_id,
        worker_name,
        secret_name,
        secret_value,
    ) {
        Ok(r) => r,
        Err(e) => {
            let _ = release_provider_mutation_reservation(
                projects_dir,
                &ctx,
                "cloudflare_rotate_worker_secret",
                &e.message,
            );
            return Err(e);
        }
    };
    let message = format!(
        "Rotated Worker secret {} on {}.",
        result
            .get("secretName")
            .and_then(|v| v.as_str())
            .unwrap_or("?"),
        result
            .get("workerName")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    record_provider_mutation(projects_dir, &ctx, "cloudflare_secret", &message)?;
    let mut out = result;
    if let Some(obj) = out.as_object_mut() {
        obj.insert("managementProjectId".into(), json!(ctx.project_id));
        obj.insert("taskId".into(), json!(ctx.task_id));
    }
    Ok(out)
}

pub fn scaleway_list_resources(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    project_id: Option<&str>,
) -> ToolResult<Value> {
    let (agent_id, role) = require_agent_tool(
        projects_dir,
        agent_id,
        role,
        "scaleway_list_resources",
        session_token,
    )?;
    let token = scaleway_token()?;
    let client = http_client()?;
    let result = scaleway_list_resources_http(&client, &token, project_id)?;
    let n = result
        .get("resources")
        .and_then(|r| r.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let _: Result<(), ToolError> = with_agents_lock(projects_dir, || {
        let mut state = read_agents_state(projects_dir)?;
        upsert_session(
            &mut state,
            &agent_id,
            &role,
            None,
            "scaleway-read",
            Some("Read Scaleway Devboule inventory."),
            None,
            None,
            None,
            None,
            None,
        )?;
        add_event(
            &mut state,
            &agent_id,
            &role,
            "scaleway_read",
            &format!("Read {n} Scaleway resources."),
            None,
            None,
            None,
            None,
        )?;
        write_agents_state(projects_dir, state)?;
        Ok(())
    });
    Ok(result)
}

pub fn scaleway_resource_action(
    projects_dir: &Path,
    agent_id: &str,
    role: &str,
    session_token: Option<&str>,
    resource_id: &str,
    action: &str,
    confirm_resource_name: Option<&str>,
    scaleway_project_id: Option<&str>,
    management_project_id: Option<&str>,
    aspis_project_id: Option<&str>,
    task_id: Option<&str>,
    evidence: Option<&str>,
) -> ToolResult<Value> {
    let ctx = require_provider_mutation_context(
        projects_dir,
        agent_id,
        role,
        "scaleway_resource_action",
        session_token,
        management_project_id,
        aspis_project_id,
        task_id,
        evidence,
    )?;
    // Early validate action shape when we have a confirm name (pin/confirm mismatch
    // before any network). Full inventory check still happens in HTTP path.
    if action.trim().eq_ignore_ascii_case("terminate") {
        return Err(ToolError::new(
            "Use delete with exact resource-name confirmation for destructive Scaleway actions.",
        ));
    }
    if action.trim().eq_ignore_ascii_case("delete") {
        // Confirm presence early; exact name match is re-checked against inventory.
        let confirmed = confirm_resource_name.map(str::trim).filter(|v| !v.is_empty());
        if confirmed.is_none() {
            return Err(ToolError::new(
                "Deleting Scaleway resources requires exact resource-name confirmation.",
            ));
        }
    }
    let token = scaleway_token()?;
    reserve_provider_mutation(projects_dir, &ctx, "scaleway_resource_action")?;
    let client = http_client()?;
    let result = match scaleway_resource_action_http(
        &client,
        &token,
        resource_id,
        action,
        confirm_resource_name,
        scaleway_project_id,
    ) {
        Ok(r) => r,
        Err(e) => {
            let _ = release_provider_mutation_reservation(
                projects_dir,
                &ctx,
                "scaleway_resource_action",
                &e.message,
            );
            return Err(e);
        }
    };
    let message = format!(
        "{} {}.",
        result.get("action").and_then(|v| v.as_str()).unwrap_or("?"),
        result
            .get("resourceName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| result.get("resourceId").and_then(|v| v.as_str()))
            .unwrap_or("?")
    );
    record_provider_mutation(projects_dir, &ctx, "scaleway_action", &message)?;
    let mut out = result;
    if let Some(obj) = out.as_object_mut() {
        obj.insert("managementProjectId".into(), json!(ctx.project_id));
        obj.insert("taskId".into(), json!(ctx.task_id));
    }
    Ok(out)
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{hash_session_token, seed_launch_pending, write_agents_state};
    use crate::tools::agent_lifecycle::agent_register;
    use serde_json::json;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn clear_unmanaged() {
        std::env::remove_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS");
        std::env::remove_var("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS");
    }

    fn set_unmanaged(on: bool) {
        if on {
            std::env::set_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "1");
        } else {
            clear_unmanaged();
            std::env::set_var("DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", "");
        }
    }

    fn set_self_attested(on: bool) {
        if on {
            std::env::set_var("DEVBOULE_MCP_ALLOW_SELF_ATTESTED_PROVIDER_MUTATION", "1");
        } else {
            std::env::remove_var("DEVBOULE_MCP_ALLOW_SELF_ATTESTED_PROVIDER_MUTATION");
            std::env::remove_var("ASPIS_MCP_ALLOW_SELF_ATTESTED_PROVIDER_MUTATION");
        }
    }

    fn temp_projects() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        fs::create_dir_all(&projects).unwrap();
        (tmp, projects)
    }

    fn register_managed(projects: &Path, agent_id: &str, role: &str) -> String {
        let token = format!("launch-{agent_id}");
        seed_launch_pending(projects, agent_id, role, &token).unwrap();
        let ack = agent_register(
            projects,
            agent_id,
            role,
            Some("test-model"),
            None,
            Some("registered"),
            Some(&token),
        )
        .unwrap();
        ack["sessionToken"].as_str().unwrap().to_string()
    }

    fn write_active_project(projects: &Path, project_id: &str, approved: bool) {
        let approved_field = if approved {
            r#", "approvedBy": "verifier-1""#
        } else {
            ""
        };
        let md = format!(
            r#"---
id: {project_id}
title: Test
status: active
updated_at: 2026-01-01T00:00:00Z
---

```aspis-project
{{
  "version": 1,
  "tasks": [
    {{"id": "T1", "title": "Work", "status": "wip", "updatedAt": "2026-01-01T00:00:00Z"{approved_field}}}
  ],
  "notes": []
}}
```
"#
        );
        let path = projects.join(format!("{project_id}.md"));
        fs::write(&path, &md).unwrap();
        // Ensure parseable via project_file.
        let _ = read_project_file(&path).expect("project parse");
    }

    fn add_claim(projects: &Path, agent_id: &str, role: &str, project_id: &str, task_id: &str) {
        with_agents_lock(projects, || {
            let mut state = read_agents_state(projects).unwrap();
            let claims = state
                .as_object_mut()
                .unwrap()
                .entry("claims".to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap();
            let lease = (Utc::now() + Duration::minutes(30)).to_rfc3339();
            claims.push(json!({
                "projectId": project_id,
                "taskId": task_id,
                "agentId": agent_id,
                "role": role,
                "status": "wip",
                "claimedAt": now_rfc3339(),
                "updatedAt": now_rfc3339(),
                "leaseUntil": lease,
            }));
            write_agents_state(projects, state).unwrap();
            Ok::<(), ToolError>(())
        })
        .unwrap();
    }

    // ── pure validation ─────────────────────────────────────────────────────

    #[test]
    fn normalize_provider_name_collapses_separators() {
        assert_eq!(normalize_provider_name("Aspis Bio"), "aspis-bio");
        assert_eq!(normalize_provider_name("aspis_bio"), "aspis-bio");
        assert_eq!(normalize_provider_name("  ASPIS--BIO  "), "aspis-bio");
    }

    #[test]
    fn cf_account_name_pin_requires_aspis_bio() {
        assert!(require_pinned_provider_name("Aspis Bio", CF_TARGET_ACCOUNT_NAME, "Cloudflare account").is_ok());
        assert!(require_pinned_provider_name("aspis_bio", CF_TARGET_ACCOUNT_NAME, "Cloudflare account").is_ok());
        let err = require_pinned_provider_name(
            "personal-account",
            CF_TARGET_ACCOUNT_NAME,
            "Cloudflare account",
        )
        .unwrap_err();
        assert!(err.message.contains("not aspis-bio"), "{}", err.message);
        assert!(err.message.contains("Cloudflare"), "{}", err.message);
    }

    #[test]
    fn scw_project_name_pin_requires_aspis_bio() {
        assert!(require_pinned_provider_name("aspis-bio", SCW_TARGET_PROJECT_NAME, "Scaleway project").is_ok());
        let err = require_pinned_provider_name(
            "other-project",
            SCW_TARGET_PROJECT_NAME,
            "Scaleway project",
        )
        .unwrap_err();
        assert!(err.message.contains("not aspis-bio"), "{}", err.message);
        assert!(err.message.contains("Scaleway"), "{}", err.message);
    }

    #[test]
    fn volume_lookup_failure_fails_closed() {
        // Unit-level: empty volume list on lookup failure is forbidden; we refuse delete.
        let err = refuse_delete_on_volume_lookup_failure(
            "srv-abc",
            "Provider GET rejected with HTTP 503",
        );
        assert!(err.message.contains("Refusing to delete"), "{}", err.message);
        assert!(err.message.contains("srv-abc"), "{}", err.message);
        assert!(err.message.contains("volumes"), "{}", err.message);
        assert!(err.message.contains("orphaned"), "{}", err.message);
        // Cause is preserved for retry guidance, but the path never continues with [].
        assert!(err.message.contains("503"), "{}", err.message);
    }

    #[test]
    fn scaleway_volume_ids_from_payload_parses_map() {
        let payload = json!({
            "server": {
                "volumes": {
                    "0": {"id": "vol-a"},
                    "1": {"id": "  vol-b  "},
                    "2": {"id": ""},
                    "3": {"name": "no-id"},
                }
            }
        });
        let ids = scaleway_volume_ids_from_server_payload(&payload);
        assert_eq!(ids, vec!["vol-a".to_string(), "vol-b".to_string()]);
        assert!(scaleway_volume_ids_from_server_payload(&json!({})).is_empty());
        assert!(scaleway_volume_ids_from_server_payload(&json!({"server": {}})).is_empty());
    }

    #[test]
    fn provider_http_status_ok_for_cascade() {
        assert!(provider_http_status_ok(200));
        assert!(provider_http_status_ok(204));
        assert!(provider_http_status_ok(404));
        assert!(!provider_http_status_ok(400));
        assert!(!provider_http_status_ok(409));
        assert!(!provider_http_status_ok(500));
    }

    #[test]
    fn worker_scope_name_and_host_boundary() {
        assert!(cloudflare_worker_name_in_scope("aspis-bio-api"));
        assert!(cloudflare_worker_name_in_scope("aspis-bio-custom"));
        assert!(cloudflare_worker_name_in_scope("orasis-worker"));
        assert!(!cloudflare_worker_name_in_scope("personal-worker"));
        assert!(!cloudflare_worker_name_in_scope("not-aspis-bio-api"));

        assert!(route_pattern_in_aspis_bio_host("api.aspis-bio.com/*"));
        assert!(route_pattern_in_aspis_bio_host("https://aspis-bio.com/api/*"));
        assert!(!route_pattern_in_aspis_bio_host("aspis-bio.com.evil.tld/*"));
        assert!(!route_pattern_in_aspis_bio_host("oracle.aspis.bio/*"));

        let routes = vec![json!({"pattern": "api.aspis-bio.com/*"})];
        assert!(cloudflare_worker_in_scope("random-name", &routes));
        let evil = vec![json!({"pattern": "aspis-bio.com.evil.tld/*"})];
        assert!(!cloudflare_worker_in_scope("random-name", &evil));
    }

    #[test]
    fn secret_rotation_validation() {
        let ok = validate_cloudflare_secret_rotation(
            "aspis-bio-api",
            "API_KEY",
            "long-enough-value",
        );
        assert!(ok.is_ok());

        let err = validate_cloudflare_secret_rotation(
            "personal-worker",
            "API_KEY",
            "long-enough-value",
        )
        .unwrap_err();
        assert!(err.message.contains("scope"), "{}", err.message);

        let err = validate_cloudflare_secret_rotation(
            "aspis-bio-api",
            "1BAD",
            "long-enough-value",
        )
        .unwrap_err();
        assert!(err.message.contains("binding"), "{}", err.message);

        let err =
            validate_cloudflare_secret_rotation("aspis-bio-api", "API_KEY", "short").unwrap_err();
        assert!(err.message.contains("too short"), "{}", err.message);
    }

    #[test]
    fn scaleway_pin_confirm_mismatch_fails() {
        let err = validate_scaleway_action("trainer-a", "delete", Some("wrong")).unwrap_err();
        assert!(
            err.message.contains("does not match") || err.message.contains("confirmation"),
            "{}",
            err.message
        );
        assert!(validate_scaleway_action("trainer-a", "delete", Some("trainer-a")).is_ok());
        let err = validate_scaleway_action("trainer-a", "terminate", Some("trainer-a")).unwrap_err();
        assert!(err.message.contains("delete"), "{}", err.message);
        assert!(validate_scaleway_action("trainer-a", "start", None).is_ok());
        assert!(validate_scaleway_action("trainer-a", "force_wipe", None).is_err());
    }

    #[test]
    fn sanitize_redacts_tokens() {
        let raw = "401 for api-keys/SCWG23BVY4W9C9VEQFFB with Bearer secret-token and X-Auth-Token scw-secret";
        let clean = sanitize_provider_error(raw);
        assert!(clean.contains("SCW[redacted]"));
        assert!(!clean.contains("SCWG23BVY4W9C9VEQFFB"));
        assert!(clean.contains("Bearer [redacted]"));
        assert!(!clean.contains("secret-token"));
        assert!(clean.contains("X-Auth-Token [redacted]"));
        assert!(!clean.contains("scw-secret"));
    }

    #[test]
    fn credentials_status_redacts_secrets() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register_managed(&projects, "status-agent", "verifier");

        std::env::set_var("SCALEWAY_API_TOKEN", "scaleway-secret-value-XYZ");
        std::env::set_var("ASPIS_CLOUDFLARE_API_TOKEN", "cf-secret-value-ABC");
        std::env::set_var("ASPIS_CLOUDFLARE_VERIFIER_TOKEN", "cf-readonly-secret");

        let result =
            provider_credentials_status(&projects, "status-agent", "verifier", Some(&tok))
                .unwrap();

        let dumped = result.to_string();
        assert!(!dumped.contains("scaleway-secret-value-XYZ"));
        assert!(!dumped.contains("cf-secret-value-ABC"));
        assert!(!dumped.contains("cf-readonly-secret"));
        assert!(result["providers"]["scaleway"]["token"]["configured"]
            .as_bool()
            .unwrap());
        assert!(result["providers"]["cloudflare"]["token"]["configured"]
            .as_bool()
            .unwrap());
        // No raw token field under github.
        assert!(result["providers"]["github"].get("token").is_none());
        assert_eq!(result["providers"]["github"]["source"], "missing");

        std::env::remove_var("SCALEWAY_API_TOKEN");
        std::env::remove_var("ASPIS_CLOUDFLARE_API_TOKEN");
        std::env::remove_var("ASPIS_CLOUDFLARE_VERIFIER_TOKEN");
    }

    #[test]
    fn orchestrator_denied_mutate() {
        let _g = env_lock();
        set_unmanaged(false);
        set_self_attested(true);
        let (_tmp, projects) = temp_projects();
        write_active_project(&projects, "scrna-seq", true);
        let tok = register_managed(&projects, "orch-1", "orchestrator");
        add_claim(&projects, "orch-1", "orchestrator", "scrna-seq", "T1");

        // Role allowlist denies first.
        let err = cloudflare_rotate_worker_secret(
            &projects,
            "orch-1",
            "orchestrator",
            Some(&tok),
            "aspis-bio-api",
            "API_KEY",
            "long-enough-value",
            None,
            Some("scrna-seq"),
            None,
            Some("T1"),
            Some("Rotate secret for claimed task."),
        )
        .unwrap_err();
        assert!(
            err.message.contains("cannot use") || err.message.contains("Only coder"),
            "{}",
            err.message
        );

        // Direct role gate.
        assert!(require_provider_mutation_role("orchestrator").is_err());
        assert!(require_provider_mutation_role("verifier").is_err());
        assert!(require_provider_mutation_role("coder").is_ok());

        set_self_attested(false);
    }

    #[test]
    fn coder_mutate_without_claim_fails() {
        let _g = env_lock();
        set_unmanaged(false);
        set_self_attested(true);
        let (_tmp, projects) = temp_projects();
        write_active_project(&projects, "scrna-seq", true);
        let tok = register_managed(&projects, "coder-1", "coder");

        let err = cloudflare_rotate_worker_secret(
            &projects,
            "coder-1",
            "coder",
            Some(&tok),
            "aspis-bio-api",
            "API_KEY",
            "long-enough-value",
            None,
            Some("scrna-seq"),
            None,
            Some("T1"),
            Some("Rotate secret for claimed task."),
        )
        .unwrap_err();
        assert!(
            err.message.contains("claim"),
            "{}",
            err.message
        );

        set_self_attested(false);
    }

    #[test]
    fn coder_mutate_without_management_context_fails() {
        let _g = env_lock();
        set_unmanaged(false);
        set_self_attested(true);
        let (_tmp, projects) = temp_projects();
        let tok = register_managed(&projects, "coder-2", "coder");

        let err = scaleway_resource_action(
            &projects,
            "coder-2",
            "coder",
            Some(&tok),
            "srv-1",
            "stop",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(
            err.message.contains("management_project_id"),
            "{}",
            err.message
        );

        set_self_attested(false);
    }

    #[test]
    fn session_token_required_for_cloud_tools() {
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let _tok = register_managed(&projects, "coder-3", "coder");

        let err = provider_credentials_status(&projects, "coder-3", "coder", None).unwrap_err();
        assert!(
            err.message.to_ascii_lowercase().contains("session")
                || err.message.contains("token"),
            "{}",
            err.message
        );

        let err =
            cloudflare_list_workers(&projects, "coder-3", "coder", None, None).unwrap_err();
        assert!(
            err.message.to_ascii_lowercase().contains("session")
                || err.message.contains("token"),
            "{}",
            err.message
        );
    }

    #[test]
    fn coder_mutate_blocked_without_approval_marker() {
        let _g = env_lock();
        set_unmanaged(false);
        set_self_attested(false); // enforce approval
        let (_tmp, projects) = temp_projects();
        write_active_project(&projects, "scrna-seq", false); // no approvedBy
        let tok = register_managed(&projects, "coder-4", "coder");
        add_claim(&projects, "coder-4", "coder", "scrna-seq", "T1");

        let err = cloudflare_rotate_worker_secret(
            &projects,
            "coder-4",
            "coder",
            Some(&tok),
            "aspis-bio-api",
            "API_KEY",
            "long-enough-value",
            None,
            Some("scrna-seq"),
            None,
            Some("T1"),
            Some("Self-attested rotation without approval."),
        )
        .unwrap_err();
        assert!(
            err.message.contains("approval marker") || err.message.contains("approvedBy"),
            "{}",
            err.message
        );
    }

    #[test]
    fn coder_mutate_fails_missing_token_after_guards() {
        let _g = env_lock();
        set_unmanaged(false);
        set_self_attested(true);
        // Ensure no CF tokens in env.
        for k in [
            "DEVBOULE_CLOUDFLARE_API_TOKEN",
            "ASPIS_CLOUDFLARE_API_TOKEN",
            "CLOUDFLARE_API_TOKEN",
            "DEVBOULE_CLOUDFLARE_SECRETS_ROTATOR_TOKEN",
            "ASPIS_CLOUDFLARE_SECRETS_ROTATOR_TOKEN",
            "DEVBOULE_CLOUDFLARE_CODER_WORKER_WRITE_TOKEN",
            "ASPIS_CLOUDFLARE_CODER_WORKER_WRITE_TOKEN",
        ] {
            std::env::remove_var(k);
        }
        let (_tmp, projects) = temp_projects();
        write_active_project(&projects, "scrna-seq", true);
        let tok = register_managed(&projects, "coder-5", "coder");
        add_claim(&projects, "coder-5", "coder", "scrna-seq", "T1");

        let err = cloudflare_rotate_worker_secret(
            &projects,
            "coder-5",
            "coder",
            Some(&tok),
            "aspis-bio-api",
            "API_KEY",
            "long-enough-value",
            None,
            Some("scrna-seq"),
            None,
            Some("T1"),
            Some("Rotate API key for claimed task."),
        )
        .unwrap_err();
        assert!(
            err.message.contains("Missing provider token"),
            "{}",
            err.message
        );

        set_self_attested(false);
    }

    #[test]
    fn delete_confirm_mismatch_on_action_path() {
        let _g = env_lock();
        set_unmanaged(false);
        set_self_attested(true);
        let (_tmp, projects) = temp_projects();
        write_active_project(&projects, "scrna-seq", true);
        let tok = register_managed(&projects, "coder-6", "coder");
        add_claim(&projects, "coder-6", "coder", "scrna-seq", "T1");

        // Early confirm gate — no SCW token needed for empty confirm.
        let err = scaleway_resource_action(
            &projects,
            "coder-6",
            "coder",
            Some(&tok),
            "srv-1",
            "delete",
            None,
            None,
            Some("scrna-seq"),
            None,
            Some("T1"),
            Some("Delete server after claimed work."),
        )
        .unwrap_err();
        assert!(
            err.message.contains("confirmation") || err.message.contains("confirm"),
            "{}",
            err.message
        );

        set_self_attested(false);
    }

    #[test]
    fn evidence_too_short_rejected() {
        let err = provider_mutation_project_context(
            Some("scrna-seq"),
            None,
            Some("T1"),
            Some("too-short"),
        )
        .unwrap_err();
        assert!(err.message.contains("evidence"), "{}", err.message);
    }

    #[test]
    fn hash_session_token_not_leaked_in_status() {
        // Sanity: status output never includes a session token hash string we plant.
        let _g = env_lock();
        set_unmanaged(false);
        let (_tmp, projects) = temp_projects();
        let tok = register_managed(&projects, "coder-7", "coder");
        let planted = hash_session_token(&tok);
        let result =
            provider_credentials_status(&projects, "coder-7", "coder", Some(&tok)).unwrap();
        assert!(!result.to_string().contains(&planted));
        assert!(!result.to_string().contains(&tok));
    }
}
