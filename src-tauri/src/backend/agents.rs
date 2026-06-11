use super::fs_replace::replace_file_with_backup;
use super::model::{AgentClaim, AgentEvent, AgentLiveState, AgentRoleRule, AgentSession};
use super::state::BackendState;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tauri::{Manager, State};

const PROJECTS_DIR: &str = "projects";
const AGENTS_STATE_FILE: &str = ".aspis-agents.json";
// Rust-owned ledger mapping agentId -> launch CLI. The MCP server may rewrite
// the session file without the `client` field, so the app keeps its own flat
// `{ "<agentId>": "codex" }` map next to the agent state and re-stamps each
// session's client on read.
const AGENT_CLIENTS_FILE: &str = ".aspis-agent-clients.json";
const MAX_EVENTS: usize = 300;

#[tauri::command]
pub fn get_agent_live_state(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<AgentLiveState, String> {
    state.ensure_unlocked()?;
    let projects_dir = projects_dir(&app)?;
    fs::create_dir_all(&projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    let state_path = projects_dir.join(AGENTS_STATE_FILE);
    // Prune ledger entries whose agent window is gone before we read it back, so
    // the live state never re-stamps a client from a dead/recycled entry and the
    // file stays bounded. Takes/releases its own lock, so it runs before ours.
    // `app` is passed so the prune can consult the live in-memory PTY session map
    // (app-hosted agents have no OS window/pid to probe).
    prune_dead_ledger_entries(&app, &projects_dir);
    let _guard = agent_state_file_lock(&projects_dir)?;
    let mut live_state = if state_path.exists() {
        let content = fs::read_to_string(&state_path)
            .map_err(|e| format!("Could not read agent state file: {e}"))?;
        serde_json::from_str::<AgentLiveState>(&content)
            .map_err(|e| format!("Agent state file is invalid: {e}"))?
    } else {
        default_agent_live_state()
    };
    live_state.rules = default_role_rules();
    live_state.state_path = state_path.to_string_lossy().into_owned();
    live_state.mcp_command = mcp_command_hint(&app, &projects_dir);
    live_state.mcp_client_config = mcp_client_config_hint(&app, &projects_dir);
    let ledger = read_agent_ledger(&projects_dir);
    stamp_sessions_from_ledger(&mut live_state.sessions, &ledger);
    Ok(live_state)
}

/// Read-time stamp: scrub the token-hash fields the UI must never see, then
/// overwrite each session's launch CLI (`client`) and terminal `host` from the
/// Rust-owned ledger. The ledger — not the MCP-owned session file — is the source
/// of truth for these two while the agent is live, so the values survive even when
/// the Python MCP server rewrote the session without them.
///
/// `host` lifecycle (the subtle part):
///   - LIVE agent: it has a ledger entry, so host is stamped from there
///     ("app"/"external"). Authoritative; overwrites whatever the file carried.
///   - CLOSED app agent: the ledger entry is pruned the moment the PTY dies, so
///     there is no entry. But `mark_agent_session_closed` persisted `host="app"`
///     onto the session at close time, so we must NOT clobber it to None here —
///     we PRESERVE the session's existing host. That durable "app" lets the UI
///     show a "Terminal exited — relaunch" hint instead of a dead Open CLI button
///     on a closed app-hosted row.
///   - Never launched by the app: no ledger entry AND no persisted host -> stays
///     None, so the UI keeps Open CLI available (legacy/external behavior).
///
/// Pure over its inputs so it is unit-testable without an AppHandle.
fn stamp_sessions_from_ledger(
    sessions: &mut [AgentSession],
    ledger: &HashMap<String, AgentLedgerEntry>,
) {
    for session in sessions.iter_mut() {
        session.launch_token_hash = None;
        session.launch_token_issued_at = None;
        session.session_token_hash = None;
        session.session_token_issued_at = None;
        if let Some(entry) = ledger.get(&session.agent_id) {
            session.client = Some(entry.client.clone());
            session.host = entry.host.clone();
        }
        // No ledger entry: leave `session.host` as the file carries it. For a
        // closed app agent that is the persisted "app" (see above); for a
        // never-app-launched session it is None.
    }
}

/// One Rust-owned ledger entry per agent. Stores the launch CLI (`client`) plus
/// the spawned terminal's process id and unique window title so the app can
/// later focus the window or stop the process. Older ledgers stored just a bare
/// client string per agent (`{"coder-7f":"codex"}`); `LedgerEntry` deserializes
/// from both shapes via the untagged enum below so the migration is transparent.
///
/// `creation_time` is the Windows process creation timestamp (the raw FILETIME
/// as a u64) captured at launch via `GetProcessTimes`. It is the anti-pid-reuse
/// fingerprint: a recycled pid will (essentially always) have a DIFFERENT
/// creation time, so the verified-pid fallback in `stop_agent`/`focus` only acts
/// when BOTH the live pid's image name AND its creation time match what we
/// recorded. Legacy entries and the non-Windows paths leave this `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentLedgerEntry {
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_time: Option<u64>,
    /// Absolute path of the launch-token-bearing prompt temp file, so Rust can
    /// delete it on stop_agent if the child shell died before its own Remove-Item
    /// ran. Legacy/non-launch entries leave this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<String>,
    /// Terminal host for this agent: "app" (PTY hosted inside Aspis Management) or
    /// "external" (detached OS console). `stop_agent` routes by this value: "app"
    /// goes to `agent_pty_kill`, everything else to the kill-by-title path. Added
    /// with `#[serde(default)]` so legacy ledgers (bare string OR rich struct
    /// without this field) still parse — old entries read back as `None`, which
    /// `stop_agent` treats as the external (legacy) path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Backward-compatible on-disk form: either the rich struct or a legacy bare
/// client string. Untagged so serde tries the struct first, then the string.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum LedgerValue {
    Entry(AgentLedgerEntry),
    LegacyClient(String),
}

impl From<LedgerValue> for AgentLedgerEntry {
    fn from(value: LedgerValue) -> Self {
        match value {
            LedgerValue::Entry(entry) => entry,
            LedgerValue::LegacyClient(client) => AgentLedgerEntry {
                client,
                pid: None,
                window_title: None,
                creation_time: None,
                prompt_file: None,
                host: None,
            },
        }
    }
}

/// Read the agentId -> entry ledger next to the agent state. Missing,
/// unreadable, or malformed files are treated as an empty ledger so a stray
/// ledger can never break the live-state read path. Legacy bare-string entries
/// are migrated to `AgentLedgerEntry` on read.
fn read_agent_ledger(projects_dir: &Path) -> HashMap<String, AgentLedgerEntry> {
    let path = projects_dir.join(AGENT_CLIENTS_FILE);
    match fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<HashMap<String, LedgerValue>>(&content) {
            Ok(raw) => raw
                .into_iter()
                .map(|(agent_id, value)| (agent_id, value.into()))
                .collect(),
            Err(_) => HashMap::new(),
        },
        Err(_) => HashMap::new(),
    }
}

fn write_agent_ledger(
    projects_dir: &Path,
    ledger: &HashMap<String, AgentLedgerEntry>,
) -> Result<(), String> {
    let path = projects_dir.join(AGENT_CLIENTS_FILE);
    let content = serde_json::to_string_pretty(ledger)
        .map_err(|e| format!("Could not serialize agent client ledger: {e}"))?;
    let temp_path = path.with_extension(format!(
        "json.{}-{}.tmp",
        std::process::id(),
        Utc::now().timestamp_millis()
    ));
    fs::write(&temp_path, content)
        .map_err(|e| format!("Could not write agent client ledger: {e}"))?;
    let backup_path = path.with_extension(format!(
        "json.{}-{}.bak",
        std::process::id(),
        Utc::now().timestamp_millis()
    ));
    replace_file_with_backup(&temp_path, &path, &backup_path, "agent client ledger")
}

/// Upsert one agentId -> entry in the ledger. Locked the same way as the agent
/// state file so concurrent launches do not corrupt the map.
#[allow(clippy::too_many_arguments)]
fn record_agent_entry(
    projects_dir: &Path,
    agent_id: &str,
    client: &str,
    pid: Option<u32>,
    window_title: Option<&str>,
    creation_time: Option<u64>,
    prompt_file: Option<&str>,
    host: Option<&str>,
) -> Result<(), String> {
    let _guard = agent_state_file_lock(projects_dir)?;
    let mut ledger = read_agent_ledger(projects_dir);
    ledger.insert(
        agent_id.to_string(),
        AgentLedgerEntry {
            client: client.to_string(),
            pid,
            window_title: window_title.map(String::from),
            creation_time,
            prompt_file: prompt_file.map(String::from),
            host: host.map(String::from),
        },
    );
    write_agent_ledger(projects_dir, &ledger)
}

/// Public entrypoint used at launch time (projects.rs) to persist the launch CLI
/// for an agent into the Rust-owned ledger, together with the spawned terminal's
/// pid and unique window title.
#[allow(clippy::too_many_arguments)]
pub fn record_agent_launch(
    app: &tauri::AppHandle,
    agent_id: &str,
    client: &str,
    pid: Option<u32>,
    window_title: Option<&str>,
    creation_time: Option<u64>,
    prompt_file: Option<&str>,
    host: Option<&str>,
) -> Result<(), String> {
    let projects_dir = projects_dir(app)?;
    fs::create_dir_all(&projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    record_agent_entry(
        &projects_dir,
        agent_id,
        client,
        pid,
        window_title,
        creation_time,
        prompt_file,
        host,
    )
}

/// Mini-coder executor support (MC-P2): a locked READ snapshot of the agent live
/// state, used by `mini_coder_executor` to read `miniCoderDirectives` + look up the
/// parent session WITHOUT holding the lock across the spawn/IO that follows. The
/// snapshot is a clone; the executor decides what to do from it, then re-takes the
/// lock via `mutate_agent_live_state` to apply transitions (re-checking under the
/// lock so a stale snapshot can never double-claim).
pub fn read_agent_live_state_snapshot(app: &tauri::AppHandle) -> Result<AgentLiveState, String> {
    let projects_dir = projects_dir(app)?;
    fs::create_dir_all(&projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    let state_path = projects_dir.join(AGENTS_STATE_FILE);
    let _guard = agent_state_file_lock(&projects_dir)?;
    if state_path.exists() {
        let content = fs::read_to_string(&state_path)
            .map_err(|e| format!("Could not read agent state file: {e}"))?;
        serde_json::from_str::<AgentLiveState>(&content)
            .map_err(|e| format!("Agent state file is invalid: {e}"))
    } else {
        Ok(default_agent_live_state())
    }
}

/// Mini-coder executor support (MC-P2): take the agent-state lock, read the live
/// state, run `mutate` against it, and write it back atomically — the
/// read-modify-write the executor uses to `apply_claim`/`apply_launched`/
/// `apply_result`/`apply_timeout` a directive and persist it. The closure returns a
/// value passed back to the caller (e.g. whether a claim won the race). The lock is
/// held ONLY for this in-memory mutation + write; the executor never spawns a PTY or
/// reads a result file inside it (lock discipline mirrors agent_pty/censor).
///
/// `rules`/`state_path`/`mcp_*` are the read-time-only fields `get_agent_live_state`
/// fills in; here we clear `state_path`/`mcp_*` before writing (as every other
/// write path does) but DO NOT reset `rules` — the writer leaves the persisted file
/// as-is for those, matching `mark_agent_session_closed`.
pub fn mutate_agent_live_state<T>(
    app: &tauri::AppHandle,
    mutate: impl FnOnce(&mut AgentLiveState) -> T,
) -> Result<T, String> {
    let projects_dir = projects_dir(app)?;
    fs::create_dir_all(&projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    let state_path = projects_dir.join(AGENTS_STATE_FILE);
    let _guard = agent_state_file_lock(&projects_dir)?;
    let mut live_state = if state_path.exists() {
        let content = fs::read_to_string(&state_path)
            .map_err(|e| format!("Could not read agent state file: {e}"))?;
        serde_json::from_str::<AgentLiveState>(&content)
            .map_err(|e| format!("Agent state file is invalid: {e}"))?
    } else {
        default_agent_live_state()
    };
    let outcome = mutate(&mut live_state);
    live_state.updated_at = Utc::now().to_rfc3339();
    live_state.state_path.clear();
    live_state.mcp_command.clear();
    live_state.mcp_client_config.clear();
    write_agent_live_state(&state_path, &live_state)?;
    Ok(outcome)
}

/// GH-P4 FIX F2: a HARDENED `mutate_agent_live_state` for a CRITICAL finalize that
/// MUST land — the push-approve command's step-3 bookkeeping, which records the real
/// outcome of a push that ALREADY PHYSICALLY RAN and clears the requesting agent's
/// `needs_user` bell. The plain `mutate_agent_live_state` gives up after the lock's
/// own ~5s spin; under contention that single budget can be exhausted, leaving the
/// bell stuck forever and no result recorded though the push happened.
///
/// This wrapper RE-RUNS the whole locked read-modify-write up to `attempts` times
/// (the closure is `Fn`, re-applied against a freshly re-read state each pass — so it
/// must stay idempotent, which the finalize is: it re-checks the live status and only
/// transitions a not-yet-recorded request). Between attempts it backs off. On total
/// failure it returns the last error so the caller can surface a clear message and
/// make a best-effort separate bell-clear. Each attempt internally still respects the
/// lock's own spin budget, so `attempts` multiplies the total time we are willing to
/// wait for the lock for this one critical write.
pub fn mutate_agent_live_state_retrying<T>(
    app: &tauri::AppHandle,
    attempts: usize,
    mutate: impl Fn(&mut AgentLiveState) -> T,
) -> Result<T, String> {
    let attempts = attempts.max(1);
    let mut last_err = String::from("mutate_agent_live_state_retrying: no attempts run");
    for attempt in 0..attempts {
        match mutate_agent_live_state(app, &mutate) {
            Ok(outcome) => return Ok(outcome),
            Err(e) => {
                last_err = e;
                // Back off before retrying (skip the sleep after the final attempt).
                if attempt + 1 < attempts {
                    thread::sleep(Duration::from_millis(100 * (attempt as u64 + 1)));
                }
            }
        }
    }
    Err(last_err)
}

/// Mini-coder executor support (MC-P2): record a ledger entry for a freshly-launched
/// mini, mirroring `record_agent_launch` but stamping host="app", the backend kind
/// as `client`, and the parent coder's id. The mini is an app-hosted PTY (no OS
/// window/pid), so pid/title/creationTime stay None. The `parent_agent_id` is kept
/// for symmetry/audit even though the nesting source of truth is the SESSION's
/// `parentAgentId` (set by the executor in the same launch transition); the ledger
/// has no parentAgentId column, so we encode it only via the session.
pub fn record_mini_launch(
    app: &tauri::AppHandle,
    agent_id: &str,
    backend_kind: &str,
) -> Result<(), String> {
    record_agent_launch(
        app,
        agent_id,
        backend_kind,
        None,
        None,
        None,
        None,
        Some(HOST_APP),
    )
}

/// Look up one agent's ledger entry (client + pid + window title) by id.
pub fn read_agent_ledger_entry(
    app: &tauri::AppHandle,
    agent_id: &str,
) -> Result<Option<AgentLedgerEntry>, String> {
    let projects_dir = projects_dir(app)?;
    Ok(read_agent_ledger(&projects_dir).get(agent_id).cloned())
}

/// Drop one agent's ledger entry (e.g. after stopping it). Best-effort: a
/// missing ledger or missing entry is treated as success.
pub fn remove_agent_ledger_entry(app: &tauri::AppHandle, agent_id: &str) -> Result<(), String> {
    let projects_dir = projects_dir(app)?;
    if !projects_dir.join(AGENT_CLIENTS_FILE).exists() {
        return Ok(());
    }
    let _guard = agent_state_file_lock(&projects_dir)?;
    let mut ledger = read_agent_ledger(&projects_dir);
    if ledger.remove(agent_id).is_none() {
        return Ok(());
    }
    write_agent_ledger(&projects_dir, &ledger)
}

fn projects_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(value) = std::env::var("ASPIS_PROJECTS_DIR") {
        let path = PathBuf::from(value.trim());
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("config.json").exists() || cwd.join(PROJECTS_DIR).exists() {
            return Ok(cwd.join(PROJECTS_DIR));
        }
        if let Some(parent) = cwd.parent() {
            if parent.join("config.json").exists() || parent.join(PROJECTS_DIR).exists() {
                return Ok(parent.join(PROJECTS_DIR));
            }
        }
    }
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data folder: {e}"))?;
    Ok(data_dir.join(PROJECTS_DIR))
}

fn default_agent_live_state() -> AgentLiveState {
    AgentLiveState {
        // Keep in lockstep with AGENTS_STATE_VERSION in oracle/server/aspis_mcp.py
        // (=2). A fresh file the Rust side writes must not look like a stale v1 to
        // the Python MCP, which only ever upgrades the version, never downgrades.
        version: 2,
        updated_at: Utc::now().to_rfc3339(),
        sessions: Vec::new(),
        claims: Vec::new(),
        events: Vec::new(),
        rules: default_role_rules(),
        state_path: String::new(),
        mcp_command: String::new(),
        mcp_client_config: String::new(),
        mini_coder_directives: Vec::new(),
        visual_check_directives: Vec::new(),
        git_push_requests: Vec::new(),
        plan_approval_requests: Vec::new(),
    }
}

pub fn record_launch_pending(
    app: &tauri::AppHandle,
    project_id: &str,
    project_title: &str,
    agent_id: &str,
    role: &str,
    task_id: Option<&str>,
    client: Option<&str>,
    launch_token_hash: &str,
) -> Result<(), String> {
    let projects_dir = projects_dir(app)?;
    fs::create_dir_all(&projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    let state_path = projects_dir.join(AGENTS_STATE_FILE);
    let _guard = agent_state_file_lock(&projects_dir)?;
    let mut live_state = if state_path.exists() {
        let content = fs::read_to_string(&state_path)
            .map_err(|e| format!("Could not read agent state file: {e}"))?;
        serde_json::from_str::<AgentLiveState>(&content)
            .map_err(|e| format!("Agent state file is invalid: {e}"))?
    } else {
        default_agent_live_state()
    };
    let timestamp = Utc::now().to_rfc3339();
    let token_hash = launch_token_hash.trim();
    if token_hash.len() < 32 {
        return Err("Launch token hash is invalid.".into());
    }
    let session = live_state
        .sessions
        .iter_mut()
        .find(|session| session.agent_id == agent_id);
    match session {
        Some(session) => {
            if session.status != "launch_pending" {
                return Err(format!(
                    "Agent id {agent_id} is already registered or active; choose a new agent id."
                ));
            }
            if session.role != role {
                return Err(format!(
                    "Agent id {agent_id} already has a pending {} launch.",
                    session.role
                ));
            }
            session.role = role.into();
            session.status = "launch_pending".into();
            session.client = client.map(String::from);
            session.message = Some(format!("Terminal launched for {project_title}."));
            session.current_project_id = Some(project_id.into());
            session.current_task_id = task_id.map(String::from);
            session.last_seen_at = Some(timestamp.clone());
            session.launch_token_hash = Some(token_hash.into());
            session.launch_token_issued_at = Some(timestamp.clone());
        }
        None => live_state.sessions.push(AgentSession {
            agent_id: agent_id.into(),
            role: role.into(),
            model: None,
            status: "launch_pending".into(),
            client: client.map(String::from),
            message: Some(format!("Terminal launched for {project_title}.")),
            current_project_id: Some(project_id.into()),
            current_task_id: task_id.map(String::from),
            current_file_path: None,
            first_seen_at: Some(timestamp.clone()),
            last_seen_at: Some(timestamp.clone()),
            launch_token_hash: Some(token_hash.into()),
            launch_token_issued_at: Some(timestamp.clone()),
            session_token_hash: None,
            session_token_issued_at: None,
            subagents: Vec::new(),
            needs_user: None,
            // host is a read-time stamp from the ledger (get_agent_live_state),
            // never persisted by the write path.
            host: None,
            // This generic launch path never spawns a mini; the P2 mini launcher
            // sets parent_agent_id through its own path.
            parent_agent_id: None,
            // Phase 1 reply-box: a fresh launch has no pending question / reply.
            pending_question: None,
            user_reply: None,
        }),
    }
    live_state.events.push(AgentEvent {
        id: format!("E{}-launch", Utc::now().timestamp_millis()),
        timestamp: timestamp.clone(),
        agent_id: agent_id.into(),
        role: role.into(),
        event_type: "launch_pending".into(),
        project_id: Some(project_id.into()),
        task_id: task_id.map(String::from),
        status: Some("launch_pending".into()),
        message: format!("Terminal launched for {project_title}; waiting for agent_register."),
        evidence: None,
    });
    if live_state.events.len() > MAX_EVENTS {
        let keep_from = live_state.events.len() - MAX_EVENTS;
        live_state.events = live_state.events.split_off(keep_from);
    }
    live_state.updated_at = timestamp;
    live_state.rules = default_role_rules();
    live_state.state_path.clear();
    live_state.mcp_command.clear();
    live_state.mcp_client_config.clear();
    write_agent_live_state(&state_path, &live_state)
}

pub fn record_manual_task_status(
    app: &tauri::AppHandle,
    project_id: &str,
    task_id: &str,
    status: &str,
) -> Result<(), String> {
    let projects_dir = projects_dir(app)?;
    fs::create_dir_all(&projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    let state_path = projects_dir.join(AGENTS_STATE_FILE);
    let _guard = agent_state_file_lock(&projects_dir)?;
    let mut live_state = if state_path.exists() {
        let content = fs::read_to_string(&state_path)
            .map_err(|e| format!("Could not read agent state file: {e}"))?;
        match serde_json::from_str::<AgentLiveState>(&content) {
            Ok(state) => state,
            Err(error) => {
                let mut state = default_agent_live_state();
                state.events.push(AgentEvent {
                    id: format!("E{}-manual-repair", Utc::now().timestamp_millis()),
                    timestamp: Utc::now().to_rfc3339(),
                    agent_id: "app-user".into(),
                    // Phase B merge: app-authored coordination events use the
                    // merged "coder" role (orchestrator is no longer a role).
                    role: "coder".into(),
                    event_type: "manual_repair".into(),
                    project_id: Some(project_id.into()),
                    task_id: Some(task_id.into()),
                    status: Some(status.into()),
                    message: "Manual Kanban move repaired unreadable agent telemetry.".into(),
                    evidence: Some(format!("Previous agent state JSON was invalid: {error}")),
                });
                state
            }
        }
    } else {
        default_agent_live_state()
    };
    let timestamp = Utc::now().to_rfc3339();
    if status == "todo" {
        live_state
            .claims
            .retain(|claim| !(claim.project_id == project_id && claim.task_id == task_id));
    } else {
        for claim in live_state.claims.iter_mut() {
            if claim.project_id == project_id && claim.task_id == task_id {
                claim.status = status.into();
                claim.updated_at = timestamp.clone();
                if matches!(status, "review" | "blocked") && claim.evidence.is_none() {
                    claim.evidence = Some("Task moved manually in Projects UI.".into());
                }
            }
        }
    }
    live_state.events.push(AgentEvent {
        id: format!("E{}-manual", Utc::now().timestamp_millis()),
        timestamp: timestamp.clone(),
        agent_id: "app-user".into(),
        // Phase B merge: app-authored coordination events use the merged "coder"
        // role (orchestrator is no longer a role).
        role: "coder".into(),
        event_type: "manual_move".into(),
        project_id: Some(project_id.into()),
        task_id: Some(task_id.into()),
        status: Some(status.into()),
        message: format!("Manual Kanban move set {task_id} to {status}."),
        evidence: Some("Projects UI manual move reconciled agent claims.".into()),
    });
    if live_state.events.len() > MAX_EVENTS {
        let keep_from = live_state.events.len() - MAX_EVENTS;
        live_state.events = live_state.events.split_off(keep_from);
    }
    live_state.updated_at = timestamp;
    live_state.rules = default_role_rules();
    live_state.state_path.clear();
    live_state.mcp_command.clear();
    live_state.mcp_client_config.clear();
    write_agent_live_state(&state_path, &live_state)
}

pub fn open_task_claim_summary(
    app: &tauri::AppHandle,
    project_id: &str,
    task_id: &str,
) -> Result<Option<String>, String> {
    let projects_dir = projects_dir(app)?;
    let state_path = projects_dir.join(AGENTS_STATE_FILE);
    if !state_path.exists() {
        return Ok(None);
    }
    let _guard = agent_state_file_lock(&projects_dir)?;
    let content = fs::read_to_string(&state_path)
        .map_err(|e| format!("Could not read agent state file: {e}"))?;
    let live_state = serde_json::from_str::<AgentLiveState>(&content)
        .map_err(|e| format!("Agent state file is invalid: {e}"))?;
    Ok(live_state
        .claims
        .iter()
        .find(|claim| {
            claim.project_id == project_id
                && claim.task_id == task_id
                && claim_blocks_manual_move(claim)
        })
        .map(|claim| format!("{} / {} / {}", claim.agent_id, claim.role, claim.status)))
}

fn claim_blocks_manual_move(claim: &AgentClaim) -> bool {
    if claim.status == "done" {
        return false;
    }
    if let Some(lease_until) = claim.lease_until.as_deref().and_then(parse_agent_timestamp) {
        return lease_until > Utc::now();
    }
    parse_agent_timestamp(&claim.updated_at)
        .is_some_and(|updated_at| Utc::now() - updated_at <= ChronoDuration::minutes(15))
}

fn parse_agent_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

struct AgentStateFileLock {
    _file: File,
}

fn agent_state_file_lock(projects_dir: &Path) -> Result<AgentStateFileLock, String> {
    fs::create_dir_all(projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    let lock_path = projects_dir.join(format!("{AGENTS_STATE_FILE}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "Could not open agent state lock {}: {e}",
                lock_path.display()
            )
        })?;
    for _ in 0..100 {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(AgentStateFileLock { _file: file }),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    Err(format!(
        "Could not acquire agent state lock: {}",
        lock_path.display()
    ))
}

fn write_agent_live_state(path: &Path, state: &AgentLiveState) -> Result<(), String> {
    let temp_path = path.with_extension(format!(
        "json.{}-{}.tmp",
        std::process::id(),
        Utc::now().timestamp_millis()
    ));
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Could not serialize agent state: {e}"))?;
    fs::write(&temp_path, content).map_err(|e| format!("Could not write agent state: {e}"))?;
    let backup_path = path.with_extension(format!(
        "json.{}-{}.bak",
        std::process::id(),
        Utc::now().timestamp_millis()
    ));
    replace_file_with_backup(&temp_path, path, &backup_path, "agent state file")
}

// ANTI-DRIFT: the `contract` strings below MUST stay verbatim-identical to the
// `contract` lists in oracle/server/aspis_mcp.py ROLE_RULES (currently the same
// three Italian mandate lines for every role). If you change one, change both —
// agents read those via MCP, so they cannot drift.
//
// PHASE B MERGE: spawn-time roles collapse to {coder, verifier}. The coder PLANS
// and CODES (absorbing the former orchestrator's planning/coordination mandate +
// its project_create_followup allowance); "orchestrator" is no longer a rule —
// it survives only as an inbound alias (→ coder) and a DERIVED UI badge. The
// Python ROLE_RULES list mirrors this exact two-role shape.
//
// INTENTIONAL BILINGUAL SPLIT (not drift): only `contract` is mirrored. The
// `summary` and `forbidden` strings here are English ON PURPOSE because they feed
// the fleet UI (house rule: English UI copy), whereas the Python copies are
// Italian because agents read those. Same data, two audiences — do NOT "fix" the
// language mismatch on summary/forbidden; it is deliberate.
fn default_role_rules() -> Vec<AgentRoleRule> {
    // The three contract lines every role shares (model declaration, subagent
    // reporting, needs_user signalling). Copied verbatim from the Python MCP.
    let shared_contract = || -> Vec<String> {
        vec![
            "Dichiara il modello (`model`) ad agent_register.",
            "Quando spawni o chiudi subagenti manda agent_heartbeat con `subagents=[{label, model, count, role?}]` aggiornato.",
            "Quando aspetti l'umano (domanda, permesso allow/deny, blocco) manda agent_heartbeat con status=\"needs_user\" e un message chiaro.",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    };
    vec![
        AgentRoleRule {
            // Phase B merge: the coder now PLANS and CODES. It absorbs the former
            // orchestrator's planning/coordination mandate (assign work, open
            // blockers, reopen tasks, create follow-ups via project_create_followup)
            // on top of its own implementation duties. Final done stays verifier-only.
            role: "coder".into(),
            summary: "Plans (/plan), works on code and uses Oracle context; opens blockers, reopens tasks, and moves work to review or blocked, but never done.".into(),
            allowed_tools: vec![
                "agent_register",
                "agent_heartbeat",
                "agent_state",
                "project_list",
                "project_get",
                "project_next_task",
                "project_claim_task",
                "project_update_status",
                "project_append_note",
                "project_create_followup",
                "provider_credentials_status",
                "cloudflare_list_workers",
                "cloudflare_rotate_worker_secret",
                "scaleway_list_resources",
                "scaleway_resource_action",
                "oracle_ask",
                "oracle_context",
                "censor_findings",
                "censor_dispose",
                "visual_check",
                "spawn_mini_coder",
                "request_git_push",
                "plan_submit",
                "plan_status",
                "ask_user",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            forbidden: vec![
                "No done status.",
                "No token printing or token logging.",
                "No provider action outside verified Aspis Bio scopes.",
                // MC-P7: mini-coder ROUTING mandate (bilingual by design — English
                // here, Italian in the Python ROLE_RULES). Delegate only cheap,
                // mechanical sub-tasks to spawn_mini_coder; front-load context; do
                // the thinking yourself; REVIEW the cheaper model's output as a draft.
                "Delegate only cheap, mechanical sub-tasks to spawn_mini_coder (boilerplate, bulk read->summary, simple edits, docstrings, tests); front-load the needed context; do the thinking yourself; REVIEW the mini's output as a draft before using it.",
                // MC-P5: mirror of the Python coder.forbidden mini-coder escalation
                // line (bilingual by design — English here, Italian in the Python
                // ROLE_RULES). If spawn_mini_coder returns aborted_by_human the coder
                // STOPS, never silently retries, and escalates via needs_user.
                "If spawn_mini_coder returns status='aborted_by_human', STOP that line of work, do NOT silently retry the mini, and escalate to the human via needs_user (agent_heartbeat status=\"needs_user\").",
                // MC-P6: mirror of the Python coder.forbidden escalation line (bilingual
                // by design — English here, Italian in the Python ROLE_RULES). When a mini
                // chain returns status='escalated' (Censor still dirty after its automatic
                // retries), the coder REDOES that file itself — the training rail already
                // captured the failed attempts — and does NOT re-spawn the mini for it.
                "If spawn_mini_coder returns status='escalated', REDO that file yourself (the mini's automatic retries failed Censor and the training rail captured them); do NOT re-spawn the mini for the same file.",
                "When you produce or review a self-contained HTML artifact and need visual feedback, call visual_check(html_path, focus?) and treat the returned critique as advisory evidence.",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            contract: shared_contract(),
            // PHASE E mandate (mirrored in Python ROLE_RULES coder.censor).
            censor: vec![
                "At each step boundary call censor_findings(project_id, file=<files you touched>) for the files you changed.",
                "Fix the real local findings; dispose false positives with censor_dispose(disposition=\"fp\").",
                "Batch at the step boundary: this is a per-step check before moving on, not a live interrupt.",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            // GH-P5 cooperative push mandate (mirrored in Python ROLE_RULES
            // coder.push — bilingual by design, English here, Italian there).
            // Agents commit freely but NEVER raw `git push`: the agent launch env
            // carries no git credentials, so a raw push fails fast; publishing
            // goes through the request_git_push MCP tool + human approval.
            push: vec![
                "Commit freely (git add -u / git commit) to save your work.",
                "NEVER run a raw `git push` — your environment has no git credentials and it will fail. To publish, call the `request_git_push` MCP tool; a human approves it.",
                "If the push request is denied or times out, STOP and escalate to the human via needs_user (agent_heartbeat status=\"needs_user\"). Do NOT retry, do NOT attempt a raw push, do NOT work around the gate.",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            // Phase 1 plan mandate (mirrored in Python ROLE_RULES coder.plan —
            // bilingual by design, English here, Italian there). The coder PLANS
            // before multi-file work: submit a plan, wait for the human's approval,
            // revise on reject per the note, and use ask_user for blocking questions
            // instead of guessing.
            plan: vec![
                "Before any multi-file or non-trivial change, submit a plan with the `plan_submit` MCP tool (a short title + the markdown plan) and WAIT for the human's approval.",
                "If the plan is rejected, READ the note and REVISE the plan per it, then resubmit — do NOT start coding against a rejected plan.",
                "If the plan request times out, STOP and escalate via needs_user (agent_heartbeat status=\"needs_user\"); do NOT proceed unapproved.",
                "When you are BLOCKED on a decision only the human can make, ask via the `ask_user` MCP tool and wait for the reply — do NOT guess or work around the question.",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        },
        AgentRoleRule {
            role: "verifier".into(),
            summary: "Checks review tasks, evidence, tests and risk. Can close or block tasks.".into(),
            allowed_tools: vec![
                "agent_register",
                "agent_heartbeat",
                "agent_state",
                "project_list",
                "project_get",
                "project_next_task",
                "project_claim_task",
                "project_update_status",
                "project_append_note",
                "provider_credentials_status",
                "cloudflare_list_workers",
                "scaleway_list_resources",
                "oracle_ask",
                "oracle_context",
                "censor_findings",
                "censor_dispose",
                "visual_check",
                "ask_user",
                "plan_status",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            forbidden: vec![
                "No coding.",
                "No Cloudflare or Scaleway mutation; read-only provider access.",
                "No done status unless the task is in review and evidence/confidence are concrete.",
                "When reviewing a self-contained HTML artifact, call visual_check(html_path, focus?) if visual layout could affect the verdict; treat the critique as advisory evidence.",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            contract: shared_contract(),
            // PHASE E mandate (mirrored in Python ROLE_RULES verifier.censor).
            censor: vec![
                "Call censor_findings(project_id) for the residual ledger; ignore findings already resolved.",
                "Focus on cross-file, architectural and multi-file security issues the small model cannot see.",
                "Adjudicate: confirm the real findings and dispose false positives with censor_dispose (fp/wontfix/fixed).",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            // GH-P5: the verifier has NO push capability (request_git_push is
            // coder-only, gated in P4) and therefore gets NO push mandate.
            push: Vec::new(),
            // Phase 1: planning is coder-only; the verifier gets NO plan mandate.
            plan: Vec::new(),
        },
    ]
}

fn mcp_command_hint(app: &tauri::AppHandle, projects_dir: &Path) -> String {
    let root = management_root_for_mcp(app, projects_dir);
    mcp_command_hint_for_paths(&root, projects_dir)
}

fn mcp_command_hint_for_paths(root: &Path, projects_dir: &Path) -> String {
    format!(
        "cd \"{}\"; $env:PYTHONPATH=\"{}\"; $env:PYTHONIOENCODING=\"utf-8\"; $env:ASPIS_MCP_CLOUDFLARE_PROFILE_MODE=\"1\"; python -m oracle.server.aspis_mcp --root \"{}\" --projects-dir \"{}\"",
        root.to_string_lossy(),
        root.to_string_lossy(),
        root.to_string_lossy(),
        projects_dir.to_string_lossy()
    )
}

fn mcp_client_config_hint(app: &tauri::AppHandle, projects_dir: &Path) -> String {
    let root = management_root_for_mcp(app, projects_dir);
    mcp_client_config_hint_for_paths(&root, projects_dir)
}

fn mcp_client_config_hint_for_paths(root: &Path, projects_dir: &Path) -> String {
    serde_json::to_string_pretty(&json!({
        "mcpServers": {
            "aspis-management": {
                "command": "python",
                "args": [
                    "-m",
                    "oracle.server.aspis_mcp",
                    "--root",
                    root.to_string_lossy(),
                    "--projects-dir",
                    projects_dir.to_string_lossy()
                ],
                "cwd": root.to_string_lossy(),
                "env": {
                    "PYTHONPATH": root.to_string_lossy(),
                    "PYTHONIOENCODING": "utf-8",
                    "HF_HUB_OFFLINE": "1",
                    "TRANSFORMERS_OFFLINE": "1",
                    "ASPIS_MCP_CLOUDFLARE_PROFILE_MODE": "1"
                }
            }
        }
    }))
    .unwrap_or_default()
}

pub fn management_root_for_mcp(app: &tauri::AppHandle, projects_dir: &Path) -> PathBuf {
    if let Ok(value) = std::env::var("ASPIS_MANAGEMENT_ROOT") {
        if let Some(path) = normalize_management_root_candidate(&PathBuf::from(value.trim())) {
            return path;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(path) = normalize_management_root_candidate(&cwd) {
            return path;
        }
        if let Some(parent) = cwd.parent() {
            if let Some(path) = normalize_management_root_candidate(parent) {
                return path;
            }
        }
    }
    if let Ok(resource_dir) = app.path().resource_dir() {
        if let Some(path) = normalize_management_root_candidate(&resource_dir) {
            return path;
        }
    }
    projects_dir.parent().unwrap_or(projects_dir).to_path_buf()
}

fn normalize_management_root_candidate(candidate: &Path) -> Option<PathBuf> {
    let mut path = candidate.to_path_buf();
    if path.file_name().and_then(|value| value.to_str()) == Some("src-tauri") {
        if let Some(parent) = path.parent() {
            path = parent.to_path_buf();
        }
    }
    if is_valid_management_root(&path) {
        return Some(path);
    }
    None
}

fn is_valid_management_root(path: &Path) -> bool {
    path.join("config.json").is_file()
        && path
            .join("oracle")
            .join("server")
            .join("aspis_mcp.py")
            .is_file()
}

/// Unique, stable window-title marker for an agent's dedicated console window.
/// `spawn_agent_terminal` sets this as the PowerShell window title; the focus
/// and stop commands find the window by EXACT-matching this string. Centralized
/// here so the producer (launch) and consumer (focus/stop) never drift apart.
///
/// EXACT match (not substring) is what makes stop/focus pid-reuse-safe and
/// reboot-safe: if the agent's window is gone, nothing matches the exact title,
/// so we touch nothing. codex/claude may overwrite the console title while
/// running; that case is handled by the verified-pid fallback (image name +
/// creation time), never by a bare stored pid.
pub fn agent_window_title(agent_id: &str) -> String {
    format!("Aspis Agent {agent_id}")
}

/// Canonical terminal-host strings.
pub const HOST_APP: &str = "app";
pub const HOST_EXTERNAL: &str = "external";

/// Normalize a raw `host` value (from launch input or the ledger) to exactly
/// "app" or "external". ONLY a case-insensitive "app" maps to app-hosted; every
/// other value — `None`, empty, "external", "APP " with stray chars, or any
/// garbage — falls back to "external". This is the zero-regression rule: an
/// unrecognised host never accidentally routes to the in-app PTY path.
pub fn normalize_agent_host(host: Option<&str>) -> &'static str {
    match host {
        Some(value) if value.trim().eq_ignore_ascii_case(HOST_APP) => HOST_APP,
        _ => HOST_EXTERNAL,
    }
}

/// True if this agent is app-hosted (its terminal runs under our in-app PTY), so
/// `stop_agent`/focus route to the PTY path rather than the kill-by-title path.
pub fn ledger_host_is_app(entry: Option<&AgentLedgerEntry>) -> bool {
    entry
        .map(|entry| normalize_agent_host(entry.host.as_deref()) == HOST_APP)
        .unwrap_or(false)
}

/// True only if `window_title` equals `wanted` EXACTLY (same length, same code
/// units). This is the single source of truth for the exact-title rule used by
/// the EnumWindows callbacks; factored out so the behavior is unit-testable on
/// every platform (the callbacks themselves are `unsafe extern` and Windows-only).
#[cfg(any(windows, test))]
fn title_matches_exact(window_title: &[u16], wanted: &[u16]) -> bool {
    !wanted.is_empty() && window_title == wanted
}

/// Windows: read the process creation time (raw FILETIME as a u64) for `pid`.
/// Returns `None` if the process cannot be opened (gone / access denied) or the
/// times cannot be read. Captured at launch and stored in the ledger so a later
/// verified-pid fallback can prove the live pid is still OUR process and not a
/// recycled one.
#[cfg(windows)]
pub(crate) fn process_creation_time(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return None;
    }

    // SAFETY: OpenProcess with QUERY_LIMITED_INFORMATION only obtains a handle we
    // immediately query and close; no memory is shared with the target. A failed
    // open returns Err and we bail. On success we ALWAYS CloseHandle below.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let result = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(handle);
        result.ok()?;
        let value = ((creation.dwHighDateTime as u64) << 32) | (creation.dwLowDateTime as u64);
        if value == 0 {
            None
        } else {
            Some(value)
        }
    }
}

/// Non-Windows stub so call sites compile unconditionally. We only capture and
/// verify creation time on Windows.
#[cfg(not(windows))]
pub(crate) fn process_creation_time(_pid: u32) -> Option<u64> {
    None
}

/// Windows: lowercased image (executable) name for `pid`, e.g. `conhost.exe`.
/// Used by the verified-pid fallback to confirm the live pid is one of OUR
/// launched processes before ever force-killing it.
#[cfg(windows)]
fn process_image_name(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return None;
    }

    // SAFETY: handle is query-only and always closed; the buffer is owned and the
    // length in/out param bounds the write to its capacity.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; MAX_PATH as usize];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let full = String::from_utf16_lossy(&buffer[..size as usize]);
        let name = full
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&full)
            .to_ascii_lowercase();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }
}

/// Windows: true only if the live `pid` is REALLY one of our launched agent
/// processes — i.e. its image is `conhost.exe` or `powershell.exe` AND its
/// process creation time equals the one we captured at launch. A recycled pid
/// (different process now owning the same number) fails at least one check, so
/// this can never green-light killing/focusing the wrong process. If we never
/// captured a creation time (legacy entry), we REFUSE (return false) — better to
/// not act than to risk a wrong-process kill.
#[cfg(windows)]
fn pid_is_verified_agent(pid: u32, expected_creation_time: Option<u64>) -> bool {
    let Some(expected) = expected_creation_time else {
        return false;
    };
    let Some(actual) = process_creation_time(pid) else {
        return false;
    };
    if actual != expected {
        return false;
    }
    match process_image_name(pid) {
        Some(name) => name == "conhost.exe" || name == "powershell.exe",
        None => false,
    }
}

/// Windows: true if at least one top-level window has EXACTLY this title. Used to
/// (a) decide whether a ledger entry is still live (pruning) and (b) gate the
/// verified-pid fallback (only used when no exact-title window exists).
#[cfg(windows)]
fn window_with_exact_title_exists(title: &str) -> bool {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;

    let title_utf16: Vec<u16> = title.encode_utf16().collect();
    let mut probe = ExactTitleProbe {
        wanted: title_utf16,
        found: false,
    };
    // SAFETY: `probe` outlives the synchronous EnumWindows call; the callback only
    // reads window text into a stack buffer and flips `probe.found`.
    unsafe {
        let lparam = LPARAM(&mut probe as *mut ExactTitleProbe as isize);
        let _ = EnumWindows(Some(exact_title_probe_proc), lparam);
    }
    probe.found
}

#[cfg(windows)]
struct ExactTitleProbe {
    wanted: Vec<u16>,
    found: bool,
}

/// EnumWindows callback for `window_with_exact_title_exists`. Stops (returns
/// FALSE) on the first exact-title match.
#[cfg(windows)]
unsafe extern "system" fn exact_title_probe_proc(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::Foundation::{FALSE, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;

    // SAFETY: the LPARAM is the `&mut ExactTitleProbe` handed to EnumWindows.
    let probe = &mut *(lparam.0 as *mut ExactTitleProbe);
    let mut buffer = [0u16; 512];
    // SAFETY: owned buffer; GetWindowTextW writes at most buffer.len() code units.
    let len = GetWindowTextW(hwnd, &mut buffer) as usize;
    if title_matches_exact(&buffer[..len], &probe.wanted) {
        probe.found = true;
        return FALSE;
    }
    TRUE
}

/// Drop ledger entries whose agent window no longer exists (EXACT title gone),
/// so a stale/recycled pid can never be acted on and the file does not grow
/// unbounded. Cheap: a single EnumWindows pass per surviving-or-dead decision is
/// avoided by enumerating once is not trivial here, so we probe per entry — the
/// ledger holds at most a handful of agents. Only rewrites the file if something
/// was actually pruned. Non-Windows: no-op (no EnumWindows), returns the ledger
/// unchanged.
/// Pure prune decision for an app-hosted ledger entry, extracted so the policy is
/// unit-testable without an AppHandle:
///   - `state_available == false`  -> KEEP (fail-safe: the PTY session map could not
///     be reached, so we must not erase a control record we cannot verify).
///   - `state_available == true`   -> keep IFF the in-memory session still exists.
/// (App-hosted entries have no OS window/pid, so the window-title liveness probe used
/// for external entries does not apply to them.)
fn app_hosted_entry_should_keep(state_available: bool, session_exists: bool) -> bool {
    if !state_available {
        return true;
    }
    session_exists
}

/// Best-effort cleanup of a dropped ledger entry's restricted prompt file. The
/// prompt file carries the launch token; when an entry is pruned (dead PTY or
/// vanished external window) the file would otherwise linger on disk until its 2h
/// expiry, so we delete it here exactly as stop_agent does on the explicit-stop
/// path. Extracted so the side effect is unit-testable without an AppHandle. A
/// `None` prompt_file (built-in clients delete it in-script) is a no-op.
// Used by the Windows prune loop and by the cross-platform unit test; on a non-test
// non-Windows build there is no prune caller, so silence the dead-code lint there.
#[cfg_attr(all(not(windows), not(test)), allow(dead_code))]
fn drop_ledger_entry_prompt_file(entry: &AgentLedgerEntry) {
    if let Some(prompt_file) = entry.prompt_file.as_deref() {
        super::projects::remove_restricted_temp_file(std::path::Path::new(prompt_file));
    }
}

fn prune_dead_ledger_entries(app: &tauri::AppHandle, projects_dir: &Path) {
    #[cfg(windows)]
    {
        let _guard = match agent_state_file_lock(projects_dir) {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let ledger = read_agent_ledger(projects_dir);
        if ledger.is_empty() {
            return;
        }
        let mut kept: HashMap<String, AgentLedgerEntry> = HashMap::new();
        let mut pruned = false;
        for (agent_id, entry) in ledger.into_iter() {
            // APP-HOSTED entries have NO OS window/pid to probe — the window/title
            // liveness check below would ALWAYS say "dead" and wrongly prune them,
            // which then misroutes stop_agent to kill-by-title and leaks the PTY
            // child. So app-hosted liveness is decided by the in-memory PTY session
            // map instead:
            //   - map CONTAINS the id  -> live, KEEP.
            //   - map does NOT contain -> the child died with the app (sessions are
            //     memory-only and do not survive a restart) -> PRUNE.
            //   - map state UNAVAILABLE (try_state None) -> FAIL-SAFE KEEP: never
            //     erase a control record we cannot verify.
            if normalize_agent_host(entry.host.as_deref()) == HOST_APP {
                let keep = match app.try_state::<super::agent_pty::AgentPtySessions>() {
                    Some(sessions) => app_hosted_entry_should_keep(
                        true,
                        super::agent_pty::pty_session_exists(&sessions, &agent_id),
                    ),
                    // State unreachable: fail-safe keep (do not erase what we can't verify).
                    None => app_hosted_entry_should_keep(false, false),
                };
                if keep {
                    kept.insert(agent_id, entry);
                } else {
                    // Dropping the entry: delete its launch-token-bearing prompt file
                    // best-effort, mirroring stop_agent (token leak otherwise).
                    drop_ledger_entry_prompt_file(&entry);
                    pruned = true;
                }
                continue;
            }
            let title = entry
                .window_title
                .clone()
                .unwrap_or_else(|| agent_window_title(&agent_id));
            // Keep the entry if its exact-title window still exists, OR if its pid
            // is still a verified live agent process (title-override case). Drop it
            // only when BOTH say the agent is gone.
            let alive = window_with_exact_title_exists(&title)
                || entry
                    .pid
                    .is_some_and(|pid| pid_is_verified_agent(pid, entry.creation_time));
            if alive {
                kept.insert(agent_id, entry);
            } else {
                // Dropping a dead external-window entry: delete its restricted prompt
                // file too (token leak otherwise), same discipline as stop_agent.
                drop_ledger_entry_prompt_file(&entry);
                pruned = true;
            }
        }
        if pruned {
            let _ = write_agent_ledger(projects_dir, &kept);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (app, projects_dir);
    }
}

/// Focus (restore + bring to foreground) the dedicated console window for an
/// agent, found by its EXACT unique title. Falls back to the ledger pid ONLY if
/// that pid passes the same image-name + creation-time verification as stop_agent
/// (the title-override case); a bare/recycled pid is never focused.
#[tauri::command]
pub fn focus_agent_terminal(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    agent_id: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let entry = read_agent_ledger_entry(&app, &agent_id)?;
    // App-hosted agents have NO OS console window to foreground — their terminal
    // lives inside the app (the frontend opens the in-app viewer directly). Focus
    // is therefore a no-op (Ok), not an error: the contract is Result<(), String>
    // and an Err would surface as a spurious "window not found" failure.
    if ledger_host_is_app(entry.as_ref()) {
        return Ok(());
    }
    let title = entry
        .as_ref()
        .and_then(|entry| entry.window_title.clone())
        .unwrap_or_else(|| agent_window_title(&agent_id));
    let pid = entry.as_ref().and_then(|entry| entry.pid);
    let creation_time = entry.as_ref().and_then(|entry| entry.creation_time);

    #[cfg(windows)]
    {
        focus_window_by_title_or_pid(&title, pid, creation_time)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (pid, creation_time);
        focus_agent_terminal_macos(&title)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (title, pid, creation_time);
        Err("Opening the agent terminal window is supported on Windows and macOS only.".into())
    }
}

// UNVERIFIED on macOS — needs testing on a real Mac.
//
// Best-effort focus on macOS: there is no EnumWindows. We ask Terminal.app (via
// `osascript`) to activate and raise the first window whose name contains the
// `Aspis Agent {id}` marker that the launch script set via the OSC-0 title
// escape. If no such window exists (closed, or the CLI overwrote the title),
// AppleScript raises an error and we surface a not-found message.
#[cfg(target_os = "macos")]
fn focus_agent_terminal_macos(title: &str) -> Result<(), String> {
    // Escape the marker for embedding inside an AppleScript string literal.
    let needle = title.replace('\\', "\\\\").replace('"', "\\\"");
    let activate = "tell application \"Terminal\" to activate".to_string();
    // `set index ... to 1` brings the matching window to the front. Wrapped in a
    // tell block so the `whose` clause resolves against Terminal's windows. EXACT
    // name match (`is`, not `contains`) mirrors the Windows exact-title rule so a
    // recycled/renamed window cannot be focused by accident.
    let raise = format!(
        "tell application \"Terminal\" to set index of (first window whose name is \"{needle}\") to 1"
    );

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&activate)
        .arg("-e")
        .arg(&raise)
        .output()
        .map_err(|e| format!("Could not run osascript to focus the agent terminal: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        // A non-zero exit usually means no window matched the marker.
        Err("Agent terminal window not found — it may have been closed.".into())
    }
}

#[cfg(windows)]
struct FocusTarget {
    title_marker: Vec<u16>,
    // Verified-pid fallback: only set when the stored pid passed image-name +
    // creation-time verification. If `None`, the pid branch never matches.
    pid: Option<u32>,
    hwnd: Option<windows::Win32::Foundation::HWND>,
}

/// Enumerate top-level windows; match the first whose title EQUALS the agent's
/// unique title. If no exact-title window exists, fall back to the owning-pid
/// match ONLY when that pid is a verified live agent process (image name +
/// creation time) — handling the case where codex/claude overwrote the console
/// title. A recycled/stale pid is rejected by verification, so the wrong window
/// is never focused. On a match the window is restored if minimized and
/// foregrounded.
#[cfg(windows)]
fn focus_window_by_title_or_pid(
    title: &str,
    pid: Option<u32>,
    creation_time: Option<u64>,
) -> Result<(), String> {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    // UTF-16 title (no trailing NUL) we EXACT-match against each window title.
    let title_marker: Vec<u16> = title.encode_utf16().collect();
    // Only allow the pid fallback if the stored pid really is our agent process.
    let verified_pid = pid.filter(|pid| pid_is_verified_agent(*pid, creation_time));
    let mut target = FocusTarget {
        title_marker,
        pid: verified_pid,
        hwnd: None,
    };

    // SAFETY: `enum_window_proc` only reads window text into a stack buffer and
    // compares it; it stores at most one HWND into `target` via the LPARAM we
    // pass below. `target` outlives the synchronous EnumWindows call, so the raw
    // pointer is valid for the whole enumeration.
    unsafe {
        let lparam = LPARAM(&mut target as *mut FocusTarget as isize);
        // EnumWindows returns Err when the callback stops early (returns FALSE);
        // that is our success signal, so the result itself is not load-bearing.
        let _ = EnumWindows(Some(enum_window_proc), lparam);
    }

    let hwnd = target
        .hwnd
        .ok_or_else(|| "Agent terminal window not found — it may have been closed.".to_string())?;

    // SAFETY: `hwnd` was captured from a live EnumWindows enumeration moments ago.
    // Restoring/foregrounding a stale handle is a no-op that returns an error we
    // ignore; none of these calls can violate memory safety.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// EnumWindows callback. Returns FALSE to stop enumeration once a match is found,
/// TRUE to keep going. Matches the window title by EXACT equality to the agent's
/// unique title; only if that misses does it match the already-verified pid.
#[cfg(windows)]
unsafe extern "system" fn enum_window_proc(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::Foundation::{FALSE, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};

    // SAFETY: the LPARAM is the `&mut FocusTarget` we handed to EnumWindows above;
    // it is valid for the duration of the enumeration and not aliased elsewhere.
    let target = &mut *(lparam.0 as *mut FocusTarget);

    let mut buffer = [0u16; 512];
    // SAFETY: `buffer` is a valid, owned slice; GetWindowTextW writes at most
    // `buffer.len()` code units and returns the count it wrote.
    let len = GetWindowTextW(hwnd, &mut buffer) as usize;
    if title_matches_exact(&buffer[..len], &target.title_marker) {
        target.hwnd = Some(hwnd);
        return FALSE;
    }

    // Fallback: only reached when `target.pid` was pre-verified as our agent
    // process (image name + creation time) by focus_window_by_title_or_pid.
    if let Some(wanted_pid) = target.pid {
        let mut window_pid: u32 = 0;
        // SAFETY: `window_pid` is a valid stack slot; the function writes the
        // owning process id of `hwnd` into it.
        GetWindowThreadProcessId(hwnd, Some(&mut window_pid as *mut u32));
        if window_pid == wanted_pid && window_pid != 0 {
            target.hwnd = Some(hwnd);
            return FALSE;
        }
    }

    TRUE
}

/// Outcome of an agent-stop kill attempt, so stop_agent can report a clear
/// message (especially the "refused: recycled pid" case).
enum KillOutcome {
    /// Killed by exact window title (or, on macOS, closed the titled window). The
    /// reboot-/pid-reuse-safe primary path.
    KilledByTitle,
    /// No exact-title window existed, but the stored pid verified as our agent
    /// process (title-override case) and was force-killed by pid.
    KilledByVerifiedPid,
    /// Nothing matched — no titled window and no usable pid. The agent is already
    /// gone (or was never ours). Treated as success: the goal is "not running".
    NothingToKill,
    /// A pid was stored but it did NOT verify as our process (recycled pid). We
    /// REFUSED to force-kill it to avoid killing an unrelated process tree.
    RefusedUnverifiedPid,
}

/// Windows: stop an agent WITHOUT ever trusting a bare stored pid.
///
/// PRIMARY: `taskkill /F /T /FI "WINDOWTITLE eq Aspis Agent <id>"`. This matches
/// by the exact unique window title, so it is reboot-safe and pid-reuse-safe: if
/// no window has that title, the filter matches nothing and kills nothing.
///
/// FALLBACK (only if the title filter matched nothing AND a pid is stored): we
/// verify the live pid is REALLY our process — image name is conhost.exe or
/// powershell.exe AND its creation time equals the one captured at launch — and
/// only then `taskkill /PID <pid> /T /F`. A recycled pid fails verification and
/// is REFUSED, never force-killed.
#[cfg(windows)]
fn kill_agent_process(title: &str, pid: Option<u32>, creation_time: Option<u64>) -> KillOutcome {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW so the helper taskkill does not flash its own console.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // PRIMARY: kill by exact window title. taskkill's WINDOWTITLE filter is an
    // exact (case-insensitive) match, so a stale title never matches another app.
    if window_with_exact_title_exists(title) {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/FI", &format!("WINDOWTITLE eq {title}")])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        return KillOutcome::KilledByTitle;
    }

    // FALLBACK: no titled window (closed, or the CLI overwrote the title). Only
    // act on the pid if it verifies as one of OUR launched processes.
    if let Some(pid) = pid {
        if pid_is_verified_agent(pid, creation_time) {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
            return KillOutcome::KilledByVerifiedPid;
        }
        // A pid was stored but it is NOT our process anymore (recycled, or never
        // captured a creation time): refuse rather than risk a wrong-process kill.
        return KillOutcome::RefusedUnverifiedPid;
    }

    KillOutcome::NothingToKill
}

// UNVERIFIED on macOS — needs testing on a real Mac.
//
/// macOS: close the agent's Terminal window by its EXACT title via AppleScript,
/// NOT by killing the stored osascript pid (which is dead/recycled and never the
/// agent shell). `close (every window whose name is "Aspis Agent <id>")` is a
/// no-op when no such window exists, so it is pid-reuse-safe. The `pid`/
/// `creation_time` are accepted for signature parity but unused here.
#[cfg(target_os = "macos")]
fn kill_agent_process(title: &str, _pid: Option<u32>, _creation_time: Option<u64>) -> KillOutcome {
    let needle = title.replace('\\', "\\\\").replace('"', "\\\"");
    let close =
        format!("tell application \"Terminal\" to close (every window whose name is \"{needle}\")");
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&close)
        .status();
    // AppleScript close is idempotent; we cannot cheaply tell "closed something"
    // from "nothing matched", so report the title path uniformly.
    KillOutcome::KilledByTitle
}

// UNVERIFIED on non-macOS unix — Terminal/AppleScript do not exist there.
//
/// Other unix (Linux): we have no terminal-window model to close by title and the
/// stored pid is unreliable, so we do nothing rather than risk a wrong-process
/// kill. Marking the session closed (the caller's next step) still updates the UI.
#[cfg(all(unix, not(target_os = "macos")))]
fn kill_agent_process(_title: &str, _pid: Option<u32>, _creation_time: Option<u64>) -> KillOutcome {
    KillOutcome::NothingToKill
}

/// Stop an agent: kill its launched process tree (best-effort), mark its session
/// closed in `.aspis-agents.json`, append a `stopped` event, drop its ledger
/// entry, and return the refreshed live state so the UI updates immediately.
#[tauri::command]
pub fn stop_agent(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    agent_id: String,
) -> Result<AgentLiveState, String> {
    state.ensure_unlocked()?;
    let entry = read_agent_ledger_entry(&app, &agent_id)?;

    // ROUTE BY HOST. App-hosted agents (host == "app") run under our in-app PTY,
    // so there is no OS console window/pid to taskkill — we tear down the PTY
    // session (kill + reap the child) instead. Every other host value (including
    // legacy entries with no host, which read back as None) takes the original
    // external kill-by-title path below.
    if ledger_host_is_app(entry.as_ref()) {
        super::agent_pty::kill_agent_pty(&app, &agent_id);
        // Best-effort prompt-file cleanup (same discipline as the external path).
        // FIX 2: remove the per-launch restricted DIRECTORY too, not just the file.
        if let Some(prompt_file) = entry.as_ref().and_then(|entry| entry.prompt_file.clone()) {
            super::projects::remove_restricted_temp_file(std::path::Path::new(&prompt_file));
        }
        // FIX 5 (idempotency): mark-closed must NOT short-circuit the ledger
        // removal. If the live-state write fails, propagating `?` here would strand
        // the ledger entry forever (a later stop would re-route to a dead PTY). So
        // capture the close result, ALWAYS remove the ledger entry, then surface the
        // close error (if any) only after the removal happened.
        // FIX 4: this branch is app-hosted (ledger_host_is_app == true), so persist
        // host="app" on the closed session.
        let close_result = mark_agent_session_closed(&app, &agent_id, Some(HOST_APP));
        let _ = remove_agent_ledger_entry(&app, &agent_id);
        close_result?;
        return get_agent_live_state(app, state);
    }

    // Identity primitive: the EXACT unique window title. Kill by title first
    // (reboot-safe, pid-reuse-safe); only fall back to the stored pid when it is
    // verified as OUR process. A recycled/stale pid can NEVER cause a wrong-process
    // kill — the title filter matches nothing and the pid fails verification, so
    // the worst case is "refuse and report", never "kill the wrong tree".
    let title = entry
        .as_ref()
        .and_then(|entry| entry.window_title.clone())
        .unwrap_or_else(|| agent_window_title(&agent_id));
    let pid = entry.as_ref().and_then(|entry| entry.pid);
    let creation_time = entry.as_ref().and_then(|entry| entry.creation_time);
    let outcome = kill_agent_process(&title, pid, creation_time);

    // Rust-side cleanup of the launch-token-bearing prompt temp file: if the child
    // shell died before its own Remove-Item ran, the token would otherwise linger
    // on disk. Best-effort; the child-side delete still runs in the happy path.
    // FIX 2: remove the per-launch restricted DIRECTORY too, not just the file.
    if let Some(prompt_file) = entry.as_ref().and_then(|entry| entry.prompt_file.clone()) {
        super::projects::remove_restricted_temp_file(std::path::Path::new(&prompt_file));
    }

    // FIX 5 (idempotency): mark-closed is best-effort here too — ALWAYS drop the
    // ledger entry (so a later focus/stop never targets a dead pid), then surface a
    // close error only after the removal happened.
    // FIX 4: this is the EXTERNAL stop path (ledger_host_is_app == false), so pass
    // None — do NOT rewrite the session's host to "app".
    let close_result = mark_agent_session_closed(&app, &agent_id, None);
    let _ = remove_agent_ledger_entry(&app, &agent_id);
    close_result?;

    let live_state = get_agent_live_state(app, state)?;

    // Surface the rare "refused to kill a recycled pid" case so the operator knows
    // the agent's original process is gone and a stranger now holds that pid.
    if matches!(outcome, KillOutcome::RefusedUnverifiedPid) {
        return Err(format!(
            "Agent {agent_id} has no live window with its title and its recorded pid now belongs to a different process (pid reuse). Refused to force-kill it; the agent session was marked closed."
        ));
    }
    let _ = outcome;

    Ok(live_state)
}

/// Best-effort public wrapper for `mark_agent_session_closed`, used by the
/// app-hosted PTY subsystem (`backend::agent_pty`) when a session ends (reader
/// EOF or explicit kill). Swallows the result: the UI already saw the terminal
/// `exited` sentinel, and a missing/unwritable state file must not poison the
/// PTY teardown path.
pub fn mark_agent_session_closed_public(app: &tauri::AppHandle, agent_id: &str) {
    // The PTY teardown paths (agent_pty.rs) are by definition app-hosted, so they
    // persist host="app" — this DURABLE stamp is the only thing that lets the UI
    // tell a dead app-hosted row (show a "Terminal exited — relaunch" hint) apart
    // from an external console (Open CLI).
    let _ = mark_agent_session_closed(app, agent_id, Some(HOST_APP));
}

/// Mark one agent's session as `closed` in the live-state file and append a
/// `stopped` event. Locked like every other agent-state mutation. A missing
/// session is not an error: stopping an already-gone agent is idempotent.
///
/// `host`: when `Some("app")`, persist host="app" on the closed session (the PTY
/// teardown path, where the session IS app-hosted). When `None`, leave the stored
/// host untouched — closing an EXTERNAL agent must NOT rewrite its host to "app"
/// (doing so hid "Open CLI" forever and showed the wrong "Terminal exited" hint).
fn mark_agent_session_closed(
    app: &tauri::AppHandle,
    agent_id: &str,
    host: Option<&str>,
) -> Result<(), String> {
    let projects_dir = projects_dir(app)?;
    fs::create_dir_all(&projects_dir)
        .map_err(|e| format!("Could not create projects folder: {e}"))?;
    let state_path = projects_dir.join(AGENTS_STATE_FILE);
    let _guard = agent_state_file_lock(&projects_dir)?;
    let mut live_state = if state_path.exists() {
        let content = fs::read_to_string(&state_path)
            .map_err(|e| format!("Could not read agent state file: {e}"))?;
        serde_json::from_str::<AgentLiveState>(&content)
            .map_err(|e| format!("Agent state file is invalid: {e}"))?
    } else {
        default_agent_live_state()
    };
    let timestamp = Utc::now().to_rfc3339();
    apply_agent_session_close(&mut live_state, agent_id, host, &timestamp);
    write_agent_live_state(&state_path, &live_state)
}

/// Pure (no-I/O) in-memory mutation behind `mark_agent_session_closed`. Extracted
/// so the host-stamping and missing-session-idempotency rules (FIX 4) are unit
/// testable without a `tauri::AppHandle`. Mutates `live_state` in place.
fn apply_agent_session_close(
    live_state: &mut AgentLiveState,
    agent_id: &str,
    host: Option<&str>,
    timestamp: &str,
) {
    // Only append the `stopped` event (and mutate state) when the session actually
    // exists. Stopping an already-gone agent is idempotent: a missing session must
    // append NO event — a synthetic event with a hardcoded role="coder" was
    // misleading (it lied about the role and invented activity for a dead agent).
    if let Some(session) = live_state
        .sessions
        .iter_mut()
        .find(|session| session.agent_id == agent_id)
    {
        session.status = "closed".into();
        session.message = Some("Stopped from Aspis Management".into());
        session.last_seen_at = Some(timestamp.to_string());
        // Persist host="app" ONLY when the caller says this close is app-hosted
        // (the PTY teardown paths pass Some("app")). For an EXTERNAL stop the caller
        // passes None and we leave the stored host untouched — rewriting it to "app"
        // would hide "Open CLI" forever and show the wrong "Terminal exited" hint.
        // The ledger entry is pruned the instant the PTY dies, so for app-hosted
        // closes this DURABLE stamp is the only thing stamp_sessions_from_ledger has
        // to preserve when there is no ledger entry.
        if let Some(host) = host {
            session.host = Some(host.to_string());
        }
        // Use the session's REAL role for the event, not a hardcoded default.
        let role = session.role.clone();
        let project_id = session.current_project_id.clone();
        let task_id = session.current_task_id.clone();
        live_state.events.push(AgentEvent {
            id: format!("E{}-stopped", Utc::now().timestamp_millis()),
            timestamp: timestamp.to_string(),
            agent_id: agent_id.into(),
            role,
            event_type: "stopped".into(),
            project_id,
            task_id,
            status: Some("closed".into()),
            message: "Stopped from Aspis Management".into(),
            evidence: None,
        });
    }
    if live_state.events.len() > MAX_EVENTS {
        let keep_from = live_state.events.len() - MAX_EVENTS;
        live_state.events = live_state.events.split_off(keep_from);
    }
    live_state.updated_at = timestamp.to_string();
    live_state.rules = default_role_rules();
    live_state.state_path.clear();
    live_state.mcp_command.clear();
    live_state.mcp_client_config.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{AgentNeedsUser, AgentSubagent};

    #[test]
    fn default_rules_keep_cloud_actions_out_of_mcp() {
        let rules = default_role_rules();
        // Phase B merge: spawn roles collapse to {coder, verifier}; the
        // standalone orchestrator rule is GONE (its planning/coordination mandate
        // folds into coder, which now PLANS and CODES).
        assert_eq!(rules.len(), 2);
        let role_set: std::collections::BTreeSet<&str> =
            rules.iter().map(|rule| rule.role.as_str()).collect();
        assert_eq!(role_set, ["coder", "verifier"].into_iter().collect());
        assert!(!rules.iter().any(|rule| rule.role == "orchestrator"));
        // The coder absorbed the orchestrator's planning mandate.
        let coder = rules
            .iter()
            .find(|rule| rule.role == "coder")
            .expect("coder rule present");
        assert!(coder.summary.to_ascii_lowercase().contains("plan"));
        assert!(coder
            .allowed_tools
            .iter()
            .any(|tool| tool == "project_create_followup"));
        assert!(rules
            .iter()
            .flat_map(|rule| rule.forbidden.iter())
            .any(|item| item.to_ascii_lowercase().contains("cloud")));
    }

    #[test]
    fn management_root_requires_mcp_entrypoint() {
        let root =
            std::env::temp_dir().join(format!("aspis-management-root-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("oracle").join("server")).unwrap();
        fs::write(root.join("config.json"), "{}").unwrap();
        assert!(normalize_management_root_candidate(&root).is_none());
        fs::write(
            root.join("oracle").join("server").join("aspis_mcp.py"),
            "# test",
        )
        .unwrap();
        assert_eq!(
            normalize_management_root_candidate(&root),
            Some(root.clone())
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn manual_mcp_hints_enable_cloudflare_profile_mode() {
        let root = std::env::temp_dir().join(format!(
            "aspis-management-mcp-hint-test-{}",
            std::process::id()
        ));
        let projects = root.join("projects");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("oracle").join("server")).unwrap();
        fs::create_dir_all(&projects).unwrap();
        fs::write(root.join("config.json"), "{}").unwrap();
        fs::write(
            root.join("oracle").join("server").join("aspis_mcp.py"),
            "# test",
        )
        .unwrap();

        let command = mcp_command_hint_for_paths(&root, &projects);
        let config = mcp_client_config_hint_for_paths(&root, &projects);

        assert!(command.contains("ASPIS_MCP_CLOUDFLARE_PROFILE_MODE"));
        assert!(config.contains("ASPIS_MCP_CLOUDFLARE_PROFILE_MODE"));
        let _ = fs::remove_dir_all(&root);
    }

    // Part B: the launch ledger must record an arbitrary CUSTOM client id verbatim
    // (the client field is a plain String), so a custom-client agent is routed and
    // displayed like any built-in.
    #[test]
    fn agent_ledger_records_an_arbitrary_custom_client_id() {
        let projects = std::env::temp_dir().join(format!(
            "aspis-custom-client-ledger-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&projects);
        fs::create_dir_all(&projects).unwrap();

        record_agent_entry(
            &projects,
            "coder-99",
            "deepseek",
            None,
            None,
            None,
            None,
            Some(HOST_APP),
        )
        .expect("ledger write");

        let ledger = read_agent_ledger(&projects);
        let entry = ledger.get("coder-99").expect("entry recorded");
        assert_eq!(entry.client, "deepseek");
        assert_eq!(entry.host.as_deref(), Some(HOST_APP));

        let _ = fs::remove_dir_all(&projects);
    }

    #[test]
    fn open_claim_blocks_manual_kanban_move_until_expired() {
        let now = Utc::now();
        let mut claim = AgentClaim {
            project_id: "project".into(),
            project_title: Some("Project".into()),
            task_id: "T1".into(),
            task_title: Some("Task".into()),
            agent_id: "coder-1".into(),
            role: "coder".into(),
            status: "review".into(),
            claimed_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            lease_until: Some((now + ChronoDuration::minutes(5)).to_rfc3339()),
            evidence: None,
        };
        assert!(claim_blocks_manual_move(&claim));

        claim.status = "done".into();
        assert!(!claim_blocks_manual_move(&claim));

        claim.status = "wip".into();
        claim.lease_until = Some((now - ChronoDuration::minutes(1)).to_rfc3339());
        assert!(!claim_blocks_manual_move(&claim));
    }

    #[test]
    fn launch_pending_session_carries_client() {
        let now = Utc::now().to_rfc3339();
        let session = AgentSession {
            agent_id: "coder-1".into(),
            role: "coder".into(),
            model: None,
            status: "launch_pending".into(),
            client: Some("codex".into()),
            message: Some("Terminal launched.".into()),
            current_project_id: Some("proj-edge".into()),
            current_task_id: Some("T-1".into()),
            current_file_path: None,
            first_seen_at: Some(now.clone()),
            last_seen_at: Some(now.clone()),
            launch_token_hash: None,
            launch_token_issued_at: None,
            session_token_hash: None,
            session_token_issued_at: None,
            subagents: Vec::new(),
            needs_user: None,
            host: None,
            parent_agent_id: None,
            pending_question: None,
            user_reply: None,
        };
        // Serializes WITH the client field set...
        let raw = serde_json::to_string(&session).unwrap();
        assert!(raw.contains("\"client\":\"codex\""));

        // ...but older JSON without the field still deserializes (client = None),
        // keeping the change additive for the Python MCP round-trip.
        let legacy = r#"{
            "agentId": "coder-2",
            "role": "coder",
            "model": null,
            "status": "wip",
            "message": null,
            "currentProjectId": null,
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null
        }"#;
        let parsed: AgentSession = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.client, None);
    }

    #[test]
    fn agent_ledger_round_trips_client_pid_and_window_title() {
        let projects = std::env::temp_dir().join(format!(
            "aspis-agent-client-ledger-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&projects);
        fs::create_dir_all(&projects).unwrap();

        // Empty/missing ledger reads as an empty map.
        assert!(read_agent_ledger(&projects).is_empty());

        record_agent_entry(&projects, "coder-7f", "codex", None, None, None, None, None).unwrap();
        record_agent_entry(
            &projects,
            "verifier-2a",
            "claude",
            Some(4242),
            Some("Aspis Agent verifier-2a"),
            Some(0x01DA_0000_0000_0001),
            Some("C:\\Temp\\aspis-agent-prompt-abc.txt"),
            Some("external"),
        )
        .unwrap();
        // Upsert overwrites the same agent id rather than duplicating it, and
        // carries the spawned pid + window title + creation time + prompt file.
        record_agent_entry(
            &projects,
            "coder-7f",
            "powershell",
            Some(1234),
            Some("Aspis Agent coder-7f"),
            Some(0x01DA_0000_0000_0002),
            None,
            Some("app"),
        )
        .unwrap();

        let ledger = read_agent_ledger(&projects);
        let coder = ledger.get("coder-7f").unwrap();
        assert_eq!(coder.client, "powershell");
        assert_eq!(coder.pid, Some(1234));
        assert_eq!(coder.window_title.as_deref(), Some("Aspis Agent coder-7f"));
        assert_eq!(coder.creation_time, Some(0x01DA_0000_0000_0002));
        // The upsert stamped host "app": stop_agent will route this one to the PTY.
        assert_eq!(coder.host.as_deref(), Some("app"));
        let verifier = ledger.get("verifier-2a").unwrap();
        assert_eq!(verifier.client, "claude");
        assert_eq!(verifier.pid, Some(4242));
        assert_eq!(verifier.creation_time, Some(0x01DA_0000_0000_0001));
        assert_eq!(verifier.host.as_deref(), Some("external"));
        assert_eq!(
            verifier.prompt_file.as_deref(),
            Some("C:\\Temp\\aspis-agent-prompt-abc.txt")
        );
        assert_eq!(ledger.len(), 2);

        let _ = fs::remove_dir_all(&projects);
    }

    #[test]
    fn agent_ledger_migrates_legacy_bare_client_strings() {
        // Legacy on-disk shape: agentId -> bare client string. Must still read as
        // an AgentLedgerEntry with client set and pid/windowTitle absent.
        let projects = std::env::temp_dir().join(format!(
            "aspis-agent-ledger-legacy-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&projects);
        fs::create_dir_all(&projects).unwrap();
        fs::write(
            projects.join(AGENT_CLIENTS_FILE),
            r#"{ "coder-1": "codex", "verifier-1": { "client": "claude", "pid": 9, "windowTitle": "Aspis Agent verifier-1" } }"#,
        )
        .unwrap();

        let ledger = read_agent_ledger(&projects);
        let legacy = ledger.get("coder-1").unwrap();
        assert_eq!(legacy.client, "codex");
        assert_eq!(legacy.pid, None);
        assert_eq!(legacy.window_title, None);
        // Legacy bare-string entries have no creation time or prompt file: the
        // verified-pid fallback therefore refuses to act on their (absent) pid.
        assert_eq!(legacy.creation_time, None);
        assert_eq!(legacy.prompt_file, None);
        // Mixed file: the new-shape entry deserializes fully alongside the legacy one.
        let modern = ledger.get("verifier-1").unwrap();
        assert_eq!(modern.client, "claude");
        assert_eq!(modern.pid, Some(9));
        // An older rich entry that predates creationTime/promptFile still migrates:
        // the missing fields default to None rather than failing the whole read.
        assert_eq!(modern.creation_time, None);
        assert_eq!(modern.prompt_file, None);
        // Legacy entries predate `host`: both read back as None, which stop_agent
        // treats as the external (legacy) path — no accidental PTY routing.
        assert_eq!(legacy.host, None);
        assert_eq!(modern.host, None);

        let _ = fs::remove_dir_all(&projects);
    }

    #[test]
    fn stamp_sessions_from_ledger_stamps_from_ledger_and_preserves_host_without_entry() {
        // Build sessions: one app-hosted (live ledger), one external (live ledger),
        // one closed app agent with a persisted host="app" but NO ledger entry
        // (entry pruned at PTY death), and one never-app session with host=None and
        // no entry. The stamp must take host from the ledger when present and
        // PRESERVE the session's own host when there is no entry (so the durable
        // closed-app "app" survives for the UI's exited hint).
        let mk = |agent_id: &str, host_seed: Option<&str>| AgentSession {
            agent_id: agent_id.into(),
            role: "coder".into(),
            model: None,
            status: "wip".into(),
            message: None,
            // Pre-seed a stale client + host. With a ledger entry the stamp must
            // overwrite both; without an entry the host is PRESERVED (closed-app
            // durable value) and the client is left as the file carried it.
            client: Some("stale".into()),
            current_project_id: None,
            current_task_id: None,
            current_file_path: None,
            first_seen_at: None,
            last_seen_at: None,
            // Token hashes must be scrubbed regardless of ledger presence.
            launch_token_hash: Some("LEAK".into()),
            launch_token_issued_at: Some("2026-01-01T00:00:00Z".into()),
            session_token_hash: Some("LEAK".into()),
            session_token_issued_at: Some("2026-01-01T00:00:00Z".into()),
            subagents: Vec::new(),
            needs_user: None,
            host: host_seed.map(String::from),
            parent_agent_id: None,
            pending_question: None,
            user_reply: None,
        };
        let mut sessions = vec![
            mk("app-agent", Some("stale-host")),
            mk("ext-agent", None),
            // Closed app agent: persisted host="app", ledger entry already pruned.
            mk("closed-app", Some("app")),
            // Never launched by the app: no host, no ledger entry.
            mk("never-app", None),
        ];

        let mut ledger: HashMap<String, AgentLedgerEntry> = HashMap::new();
        ledger.insert(
            "app-agent".into(),
            AgentLedgerEntry {
                client: "powershell".into(),
                pid: None,
                window_title: None,
                creation_time: None,
                prompt_file: None,
                host: Some("app".into()),
            },
        );
        ledger.insert(
            "ext-agent".into(),
            AgentLedgerEntry {
                client: "codex".into(),
                pid: None,
                window_title: None,
                creation_time: None,
                prompt_file: None,
                host: Some("external".into()),
            },
        );

        stamp_sessions_from_ledger(&mut sessions, &ledger);

        // app-agent: host + client from the ledger; tokens scrubbed.
        assert_eq!(sessions[0].host.as_deref(), Some("app"));
        assert_eq!(sessions[0].client.as_deref(), Some("powershell"));
        assert_eq!(sessions[0].launch_token_hash, None);
        assert_eq!(sessions[0].session_token_hash, None);
        assert_eq!(sessions[0].launch_token_issued_at, None);
        assert_eq!(sessions[0].session_token_issued_at, None);
        // ext-agent: host "external" from the ledger.
        assert_eq!(sessions[1].host.as_deref(), Some("external"));
        assert_eq!(sessions[1].client.as_deref(), Some("codex"));
        // closed-app: no ledger entry -> persisted host="app" PRESERVED (drives the
        // UI exited hint); client untouched; tokens still scrubbed.
        assert_eq!(sessions[2].host.as_deref(), Some("app"));
        assert_eq!(sessions[2].client.as_deref(), Some("stale"));
        assert_eq!(sessions[2].launch_token_hash, None);
        // never-app: no entry, no persisted host -> stays None (Open CLI allowed).
        assert_eq!(sessions[3].host, None);
        assert_eq!(sessions[3].client.as_deref(), Some("stale"));
        assert_eq!(sessions[3].launch_token_hash, None);
    }

    #[test]
    fn agent_ledger_entry_serializes_new_fields_and_round_trips() {
        // creationTime/promptFile/host serialize when present and are omitted (not
        // null) when absent, keeping the on-disk form compact and the migration
        // additive.
        let full = AgentLedgerEntry {
            client: "codex".into(),
            pid: Some(4321),
            window_title: Some("Aspis Agent coder-9".into()),
            creation_time: Some(0x01DA_1234_5678_9ABC),
            prompt_file: Some("C:\\Temp\\aspis-agent-prompt-xyz.txt".into()),
            host: Some("app".into()),
        };
        let json = serde_json::to_string(&full).unwrap();
        assert!(json.contains("\"creationTime\":"));
        assert!(json.contains("\"promptFile\":"));
        assert!(json.contains("\"host\":\"app\""));
        let back: AgentLedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, full);

        let bare = AgentLedgerEntry {
            client: "claude".into(),
            pid: None,
            window_title: None,
            creation_time: None,
            prompt_file: None,
            host: None,
        };
        let bare_json = serde_json::to_string(&bare).unwrap();
        assert!(!bare_json.contains("creationTime"));
        assert!(!bare_json.contains("promptFile"));
        assert!(!bare_json.contains("pid"));
        assert!(!bare_json.contains("host"));

        // Legacy rich entry JSON that predates `host` still parses (host = None).
        let legacy_json = r#"{"client":"codex","pid":7,"windowTitle":"Aspis Agent coder-1"}"#;
        let legacy: AgentLedgerEntry = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.host, None);
        assert_eq!(legacy.client, "codex");
    }

    #[test]
    fn normalize_host_only_app_maps_to_app_everything_else_external() {
        assert_eq!(normalize_agent_host(Some("app")), "app");
        // Case-insensitive + surrounding whitespace tolerated for "app".
        assert_eq!(normalize_agent_host(Some("APP")), "app");
        assert_eq!(normalize_agent_host(Some("  App  ")), "app");
        // Everything else falls back to external (zero-regression rule).
        assert_eq!(normalize_agent_host(Some("external")), "external");
        assert_eq!(normalize_agent_host(Some("garbage")), "external");
        assert_eq!(normalize_agent_host(Some("")), "external");
        assert_eq!(normalize_agent_host(None), "external");
    }

    #[test]
    fn ledger_host_is_app_routes_only_app_entries() {
        let app_entry = AgentLedgerEntry {
            client: "codex".into(),
            pid: None,
            window_title: None,
            creation_time: None,
            prompt_file: None,
            host: Some("app".into()),
        };
        let external_entry = AgentLedgerEntry {
            host: Some("external".into()),
            ..app_entry.clone()
        };
        let legacy_entry = AgentLedgerEntry {
            host: None,
            ..app_entry.clone()
        };
        assert!(ledger_host_is_app(Some(&app_entry)));
        assert!(!ledger_host_is_app(Some(&external_entry)));
        assert!(!ledger_host_is_app(Some(&legacy_entry)));
        // No ledger entry at all (agent never launched by us) -> not app-hosted.
        assert!(!ledger_host_is_app(None));
    }

    #[test]
    fn app_hosted_prune_keeps_live_map_entry_prunes_missing_keeps_when_unavailable() {
        // host=app + live in-memory session -> KEEP.
        assert!(app_hosted_entry_should_keep(true, true));
        // host=app + no session (child died with the app / restart) -> PRUNE.
        assert!(!app_hosted_entry_should_keep(true, false));
        // PTY state unreachable -> FAIL-SAFE KEEP (never erase what we can't verify).
        assert!(app_hosted_entry_should_keep(false, false));
        assert!(app_hosted_entry_should_keep(false, true));
    }

    // WARNING fix: pruning a dead ledger entry must delete its restricted prompt
    // file (the launch token would otherwise linger on disk until 2h expiry). The
    // drop helper is the side effect both prune drop sites share; assert it removes
    // the file (and its per-launch dir) and is a safe no-op when prompt_file is None.
    #[test]
    fn dropping_a_ledger_entry_removes_its_prompt_file() {
        // Build a per-launch-style restricted dir with a token file inside it.
        let dir = std::env::temp_dir().join(format!(
            "aspis-agent-prompt-prune-test-{}-{}.d",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create restricted dir");
        let prompt_path = dir.join("prompt.txt");
        std::fs::write(&prompt_path, "launch-token-secret").expect("write prompt file");
        assert!(prompt_path.exists());

        let entry = AgentLedgerEntry {
            client: "deepseek".into(),
            pid: Some(1234),
            window_title: Some("Aspis Agent coder-1".into()),
            creation_time: None,
            prompt_file: Some(prompt_path.to_string_lossy().into_owned()),
            host: None,
        };
        drop_ledger_entry_prompt_file(&entry);
        assert!(
            !prompt_path.exists(),
            "prompt file must be removed on prune"
        );
        assert!(
            !dir.exists(),
            "per-launch restricted dir must be removed too"
        );

        // No prompt_file (built-in client): no panic, nothing to remove.
        let none_entry = AgentLedgerEntry {
            client: "claude".into(),
            pid: None,
            window_title: None,
            creation_time: None,
            prompt_file: None,
            host: None,
        };
        drop_ledger_entry_prompt_file(&none_entry);
    }

    #[test]
    fn agent_window_title_is_stable_and_contains_id() {
        assert_eq!(agent_window_title("coder-7f"), "Aspis Agent coder-7f");
        assert!(agent_window_title("verifier-2a").contains("verifier-2a"));
    }

    #[test]
    fn title_match_is_exact_not_substring() {
        let wanted: Vec<u16> = agent_window_title("coder-7f").encode_utf16().collect();

        // Exact title matches.
        let exact: Vec<u16> = "Aspis Agent coder-7f".encode_utf16().collect();
        assert!(title_matches_exact(&exact, &wanted));

        // A superstring (e.g. a window that merely CONTAINS the marker, or a CLI
        // that appended status text) must NOT match: this is what makes stop/focus
        // pid-reuse-safe and prevents grabbing an unrelated window.
        let superstring: Vec<u16> = "Aspis Agent coder-7f - codex running"
            .encode_utf16()
            .collect();
        assert!(!title_matches_exact(&superstring, &wanted));

        // A different agent id must NOT match.
        let other: Vec<u16> = "Aspis Agent coder-7e".encode_utf16().collect();
        assert!(!title_matches_exact(&other, &wanted));

        // An empty wanted slice never matches (guards the no-title case).
        assert!(!title_matches_exact(&exact, &[]));
    }

    #[test]
    fn agent_session_round_trips_subagents_and_needs_user() {
        let now = Utc::now().to_rfc3339();
        let session = AgentSession {
            agent_id: "orchestrator-1".into(),
            role: "orchestrator".into(),
            model: Some("opus".into()),
            status: "needs_user".into(),
            message: Some("Waiting for allow/deny.".into()),
            client: Some("claude".into()),
            current_project_id: Some("proj-1".into()),
            current_task_id: None,
            current_file_path: None,
            first_seen_at: Some(now.clone()),
            last_seen_at: Some(now.clone()),
            launch_token_hash: None,
            launch_token_issued_at: None,
            session_token_hash: None,
            session_token_issued_at: None,
            subagents: vec![
                AgentSubagent {
                    label: "coders".into(),
                    model: "sonnet".into(),
                    count: 2,
                    role: Some("coder".into()),
                },
                AgentSubagent {
                    label: "helper".into(),
                    model: "haiku".into(),
                    count: 1,
                    role: None,
                },
            ],
            needs_user: Some(AgentNeedsUser {
                reason: "permission".into(),
                message: "Allow file write?".into(),
                since: now.clone(),
            }),
            host: None,
            parent_agent_id: None,
            pending_question: None,
            user_reply: None,
        };
        let raw = serde_json::to_string(&session).unwrap();
        // camelCase rename for needsUser; subagents present.
        assert!(raw.contains("\"needsUser\":"));
        assert!(raw.contains("\"subagents\":"));
        let back: AgentSession = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.subagents.len(), 2);
        assert_eq!(back.subagents[0].role.as_deref(), Some("coder"));
        assert_eq!(back.subagents[1].role, None);
        assert_eq!(back.subagents[0].count, 2);
        let needs = back.needs_user.expect("needsUser preserved");
        assert_eq!(needs.reason, "permission");
        assert_eq!(needs.message, "Allow file write?");
    }

    #[test]
    fn agent_session_without_new_fields_deserializes_with_defaults() {
        // Old JSON predating subagents/needsUser must still deserialize, with the
        // new fields defaulting to empty/None so the Python MCP round-trip and
        // legacy files keep working.
        let legacy = r#"{
            "agentId": "coder-3",
            "role": "coder",
            "model": null,
            "status": "wip",
            "message": null,
            "currentProjectId": null,
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null
        }"#;
        let parsed: AgentSession = serde_json::from_str(legacy).unwrap();
        assert!(parsed.subagents.is_empty());
        assert_eq!(parsed.needs_user, None);
        // Empty subagents / None needsUser must NOT be serialized back into the
        // file (skip_serializing_if), so Rust never injects fields Python owns.
        let raw = serde_json::to_string(&parsed).unwrap();
        assert!(!raw.contains("subagents"));
        assert!(!raw.contains("needsUser"));
    }

    #[test]
    fn legacy_orchestrator_session_loads_and_preserves_role_string() {
        // Phase B back-compat: an old `.aspis-agents.json` session stored with
        // role:"orchestrator" (no longer a spawnable role) must still deserialize
        // without error, and the role string must be preserved verbatim so the TS
        // display layer can derive the "orchestrator" badge + Polis noble figure
        // for it. The ledger role is a tolerant String, not an enum, so no
        // migration is needed.
        let legacy = r#"{
            "agentId": "orchestrator-legacy",
            "role": "orchestrator",
            "model": "opus",
            "status": "online",
            "message": "Coordinating",
            "currentProjectId": "proj-1",
            "currentTaskId": null,
            "firstSeenAt": null,
            "lastSeenAt": null
        }"#;
        let parsed: AgentSession = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.role, "orchestrator");
        // Round-trips unchanged (Rust never rewrites the legacy role to coder).
        let raw = serde_json::to_string(&parsed).unwrap();
        assert!(raw.contains("\"role\":\"orchestrator\""));
    }

    #[test]
    fn agent_live_state_round_trip_preserves_python_written_fields() {
        // A full AgentLiveState written by the Python MCP (camelCase, version 2,
        // subagents with role:null, both needsUser:null and needsUser:object)
        // must survive a Rust read -> write cycle without losing any field.
        let python_json = r#"{
            "version": 2,
            "updatedAt": "2026-06-04T10:00:00+00:00",
            "sessions": [
                {
                    "agentId": "orchestrator-1",
                    "role": "orchestrator",
                    "model": "opus",
                    "status": "online",
                    "message": "Coordinating",
                    "currentProjectId": "proj-1",
                    "currentTaskId": null,
                    "firstSeenAt": "2026-06-04T09:00:00+00:00",
                    "lastSeenAt": "2026-06-04T10:00:00+00:00",
                    "subagents": [
                        {"label": "coders", "model": "sonnet", "count": 3, "role": "coder"},
                        {"label": "scratch", "model": "haiku", "count": 1, "role": null}
                    ],
                    "needsUser": null
                },
                {
                    "agentId": "coder-9",
                    "role": "coder",
                    "model": "sonnet",
                    "status": "needs_user",
                    "message": "Blocked",
                    "currentProjectId": "proj-1",
                    "currentTaskId": "T-1",
                    "firstSeenAt": "2026-06-04T09:30:00+00:00",
                    "lastSeenAt": "2026-06-04T10:00:00+00:00",
                    "needsUser": {
                        "reason": "question",
                        "message": "Which schema?",
                        "since": "2026-06-04T09:59:00+00:00"
                    }
                }
            ],
            "claims": [],
            "events": [],
            "rules": [],
            "statePath": "",
            "mcpCommand": "",
            "mcpClientConfig": ""
        }"#;
        let state: AgentLiveState = serde_json::from_str(python_json).unwrap();
        assert_eq!(state.version, 2);
        // Round-trip: serialize back and reparse, asserting field-level survival.
        let rewritten = serde_json::to_string(&state).unwrap();
        let reparsed: AgentLiveState = serde_json::from_str(&rewritten).unwrap();

        let orch = &reparsed.sessions[0];
        assert_eq!(orch.subagents.len(), 2);
        assert_eq!(orch.subagents[0].count, 3);
        assert_eq!(orch.subagents[0].role.as_deref(), Some("coder"));
        assert_eq!(orch.subagents[1].role, None);
        assert_eq!(orch.needs_user, None);

        let coder = &reparsed.sessions[1];
        // A session with no subagents key stays empty and is not re-emitted.
        assert!(coder.subagents.is_empty());
        let needs = coder.needs_user.as_ref().expect("needsUser kept");
        assert_eq!(needs.reason, "question");
        assert_eq!(needs.message, "Which schema?");
        assert_eq!(needs.since, "2026-06-04T09:59:00+00:00");
    }

    #[test]
    fn default_role_rules_carry_contract_for_all_roles() {
        let rules = default_role_rules();
        // Phase B merge: two roles remain (coder, verifier).
        assert_eq!(rules.len(), 2);
        for rule in &rules {
            assert!(
                !rule.contract.is_empty(),
                "role {} must declare a contract",
                rule.role
            );
            // The model-declaration mandate must be present verbatim for each role.
            assert!(rule
                .contract
                .iter()
                .any(|line| line == "Dichiara il modello (`model`) ad agent_register."));
        }
    }

    #[test]
    fn shared_contract_matches_python_literals_verbatim() {
        // ANTI-DRIFT: every role's `contract` must be byte-identical to the
        // ROLE_RULES[].contract lines in oracle/server/aspis_mcp.py. If this
        // breaks, the Python and Rust copies have drifted — fix BOTH together.
        let expected = [
            "Dichiara il modello (`model`) ad agent_register.",
            "Quando spawni o chiudi subagenti manda agent_heartbeat con `subagents=[{label, model, count, role?}]` aggiornato.",
            "Quando aspetti l'umano (domanda, permesso allow/deny, blocco) manda agent_heartbeat con status=\"needs_user\" e un message chiaro.",
        ];
        for rule in default_role_rules() {
            assert_eq!(
                rule.contract, expected,
                "role {} contract drifted from the Python mirror",
                rule.role
            );
        }
    }

    #[test]
    fn default_role_rules_carry_censor_mandates() {
        // PHASE E: both roles must declare a Censor-consumption mandate, and each
        // must reference the two MCP tools so an agent reading its rules knows to
        // call them. The coder mandate is per-step + file-scoped; the verifier's is
        // the whole residual ledger + adjudication.
        let rules = default_role_rules();
        let coder = rules.iter().find(|r| r.role == "coder").unwrap();
        let verifier = rules.iter().find(|r| r.role == "verifier").unwrap();
        assert!(
            !coder.censor.is_empty(),
            "coder must declare a Censor mandate"
        );
        assert!(
            !verifier.censor.is_empty(),
            "verifier must declare a Censor mandate"
        );
        let coder_blob = coder.censor.join(" ");
        assert!(
            coder_blob.contains("censor_findings"),
            "coder cites censor_findings"
        );
        assert!(
            coder_blob.contains("censor_dispose"),
            "coder cites censor_dispose"
        );
        assert!(
            coder_blob.to_lowercase().contains("step boundary"),
            "coder mandate is per-step"
        );
        let verifier_blob = verifier.censor.join(" ");
        assert!(
            verifier_blob.contains("censor_findings"),
            "verifier cites censor_findings"
        );
        assert!(
            verifier_blob.contains("censor_dispose"),
            "verifier cites censor_dispose"
        );
        assert!(
            verifier_blob.to_lowercase().contains("residual"),
            "verifier mandate is residual adjudication"
        );
    }

    #[test]
    fn coder_rule_carries_mini_coder_aborted_escalation() {
        // MC-P5: the coder role's forbidden list must carry the aborted_by_human
        // escalation mandate (STOP + no silent retry + escalate via needs_user),
        // mirroring the Python ROLE_RULES coder.forbidden line.
        let rules = default_role_rules();
        let coder = rules.iter().find(|r| r.role == "coder").unwrap();
        let blob = coder.forbidden.join(" ");
        assert!(
            blob.contains("aborted_by_human"),
            "coder cites aborted_by_human"
        );
        assert!(blob.contains("STOP"), "coder STOPS on abort");
        assert!(
            blob.contains("silently retry"),
            "coder must not silently retry"
        );
        assert!(
            blob.contains("needs_user"),
            "coder escalates via needs_user"
        );
        // The verifier (no spawn_mini_coder) must NOT carry it.
        let verifier = rules.iter().find(|r| r.role == "verifier").unwrap();
        assert!(
            !verifier.forbidden.join(" ").contains("aborted_by_human"),
            "verifier must not carry the mini-coder escalation rule"
        );
    }

    #[test]
    fn coder_rule_carries_plan_mandate() {
        // Phase 1: the coder role must declare a plan mandate citing plan_submit,
        // wait-for-approval, revise-on-reject, and ask_user. The verifier (planning is
        // coder-only) must carry NO plan mandate.
        let rules = default_role_rules();
        let coder = rules.iter().find(|r| r.role == "coder").unwrap();
        assert!(!coder.plan.is_empty(), "coder must declare a plan mandate");
        let blob = coder.plan.join(" ");
        assert!(blob.contains("plan_submit"), "coder cites plan_submit");
        assert!(
            blob.to_lowercase().contains("approval") || blob.to_lowercase().contains("approv"),
            "coder waits for approval"
        );
        assert!(
            blob.to_lowercase().contains("revise"),
            "coder revises on reject"
        );
        assert!(blob.contains("ask_user"), "coder cites ask_user");
        let verifier = rules.iter().find(|r| r.role == "verifier").unwrap();
        assert!(
            verifier.plan.is_empty(),
            "verifier must carry no plan mandate"
        );
    }

    #[test]
    fn coder_rule_carries_mini_coder_routing_mandate() {
        // MC-P7: the coder role's forbidden list must carry the mini routing mandate
        // (delegate only cheap/mechanical work + review the mini's output as a draft),
        // mirroring the Python ROLE_RULES coder.forbidden line. The verifier (no
        // spawn_mini_coder) must NOT carry it.
        let rules = default_role_rules();
        let coder = rules.iter().find(|r| r.role == "coder").unwrap();
        let blob = coder.forbidden.join(" ");
        assert!(
            blob.contains("Delegate only cheap, mechanical sub-tasks to spawn_mini_coder"),
            "coder routing mandate scopes delegation to cheap/mechanical sub-tasks"
        );
        assert!(
            blob.contains("REVIEW the mini's output as a draft"),
            "coder must review the mini's output as a draft"
        );
        let verifier = rules.iter().find(|r| r.role == "verifier").unwrap();
        assert!(
            !verifier.forbidden.join(" ").contains("spawn_mini_coder"),
            "verifier must not carry any spawn_mini_coder routing rule"
        );
    }

    #[test]
    fn request_git_push_is_coder_only() {
        // GH-P4/P5: request_git_push is a coder-only capability. It MUST be in the
        // coder's allowed_tools and MUST NOT be in the verifier's. Mirrored in the
        // Python side (test_request_git_push_is_coder_only).
        let rules = default_role_rules();
        let coder = rules.iter().find(|r| r.role == "coder").unwrap();
        let verifier = rules.iter().find(|r| r.role == "verifier").unwrap();
        assert!(
            coder.allowed_tools.iter().any(|t| t == "request_git_push"),
            "coder must have request_git_push"
        );
        assert!(
            !verifier.allowed_tools.iter().any(|t| t == "request_git_push"),
            "verifier must NOT have request_git_push"
        );
    }

    #[test]
    fn coder_rule_carries_cooperative_push_mandate() {
        // GH-P5: the coder role's `push` mandate must carry the cooperative push
        // contract (commit freely, NEVER raw git push, publish via request_git_push,
        // STOP + needs_user on deny/timeout). Mirrored — bilingual — in the Python
        // ROLE_RULES coder.push (test_coder_role_rules_carry_cooperative_push_mandate).
        let rules = default_role_rules();
        let coder = rules.iter().find(|r| r.role == "coder").unwrap();
        assert!(!coder.push.is_empty(), "coder must declare a push mandate");
        let blob = coder.push.join(" ");
        // Commit-freely line.
        assert!(blob.contains("Commit freely"), "coder commits freely: {blob}");
        // Never-raw-push line that names the request_git_push tool.
        assert!(
            blob.contains("NEVER run a raw `git push`"),
            "coder must be told never to raw push: {blob}"
        );
        assert!(
            blob.contains("request_git_push"),
            "coder push mandate cites the request_git_push tool: {blob}"
        );
        // Deny/timeout → STOP + escalate via needs_user, no retry/workaround.
        assert!(blob.contains("denied or times out"), "deny/timeout branch: {blob}");
        assert!(blob.contains("STOP"), "coder STOPS on deny/timeout: {blob}");
        assert!(
            blob.contains("needs_user"),
            "coder escalates via needs_user: {blob}"
        );
        assert!(
            blob.contains("Do NOT retry"),
            "coder must not retry/work around the gate: {blob}"
        );
        // The verifier (no request_git_push) must NOT carry ANY push mandate.
        let verifier = rules.iter().find(|r| r.role == "verifier").unwrap();
        assert!(
            verifier.push.is_empty(),
            "verifier must have NO push mandate"
        );
        assert!(
            !verifier.push.join(" ").contains("request_git_push"),
            "verifier must not reference request_git_push in a push mandate"
        );
    }

    #[test]
    fn allowed_tools_match_python_role_rules_verbatim() {
        // ANTI-DRIFT (WARNING 5): each role's `allowed_tools` MUST equal the
        // ROLE_RULES[].allowedTools list in oracle/server/aspis_mcp.py. The Python
        // side has a twin test (test_allowed_tools_match_rust_default_role_rules)
        // that parses THIS file and compares the other direction, so the two
        // implementations cannot drift their permission surface. If this breaks,
        // fix BOTH languages together — a mismatch is a real privilege divergence.
        let coder_expected = [
            "agent_register",
            "agent_heartbeat",
            "agent_state",
            "project_list",
            "project_get",
            "project_next_task",
            "project_claim_task",
            "project_update_status",
            "project_append_note",
            "project_create_followup",
            "provider_credentials_status",
            "cloudflare_list_workers",
            "cloudflare_rotate_worker_secret",
            "scaleway_list_resources",
            "scaleway_resource_action",
            "oracle_ask",
            "oracle_context",
            "censor_findings",
            "censor_dispose",
            "visual_check",
            "spawn_mini_coder",
            "request_git_push",
            "plan_submit",
            "plan_status",
            "ask_user",
        ];
        let verifier_expected = [
            "agent_register",
            "agent_heartbeat",
            "agent_state",
            "project_list",
            "project_get",
            "project_next_task",
            "project_claim_task",
            "project_update_status",
            "project_append_note",
            "provider_credentials_status",
            "cloudflare_list_workers",
            "scaleway_list_resources",
            "oracle_ask",
            "oracle_context",
            "censor_findings",
            "censor_dispose",
            "visual_check",
            "ask_user",
            "plan_status",
        ];
        let rules = default_role_rules();
        let coder = rules.iter().find(|r| r.role == "coder").unwrap();
        let verifier = rules.iter().find(|r| r.role == "verifier").unwrap();
        assert_eq!(coder.allowed_tools, coder_expected);
        assert_eq!(verifier.allowed_tools, verifier_expected);
    }

    #[test]
    fn default_agent_live_state_uses_current_version() {
        // Must match AGENTS_STATE_VERSION in oracle/server/aspis_mcp.py (=2) so a
        // fresh file the Rust side writes is not seen as a stale v1 by Python.
        assert_eq!(default_agent_live_state().version, 2);
    }

    // FIX 4 helpers/tests: exercise the pure in-memory close logic without a real
    // tauri::AppHandle.
    fn live_state_with_session(agent_id: &str, role: &str, host: Option<&str>) -> AgentLiveState {
        let mut session: AgentSession = serde_json::from_value(serde_json::json!({
            "agentId": agent_id,
            "role": role,
            "status": "active",
        }))
        .unwrap();
        session.host = host.map(|h| h.to_string());
        let mut state = default_agent_live_state();
        state.sessions.push(session);
        state
    }

    #[test]
    fn external_close_does_not_rewrite_host_to_app() {
        // An external agent (host="external") closed via the EXTERNAL stop path
        // (host param = None) keeps its host — it must NOT become "app".
        let mut state = live_state_with_session("ext-1", "reviewer", Some("external"));
        apply_agent_session_close(&mut state, "ext-1", None, "2026-06-04T00:00:00Z");
        let session = &state.sessions[0];
        assert_eq!(session.status, "closed");
        assert_eq!(session.host.as_deref(), Some("external"));
        // The event uses the REAL role, not a hardcoded "coder".
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.events[0].role, "reviewer");
        assert_eq!(state.events[0].event_type, "stopped");
    }

    #[test]
    fn external_close_with_no_stored_host_leaves_host_none() {
        // A legacy/external session with no host stays None under the external path.
        let mut state = live_state_with_session("ext-2", "coder", None);
        apply_agent_session_close(&mut state, "ext-2", None, "2026-06-04T00:00:00Z");
        assert_eq!(state.sessions[0].host, None);
    }

    #[test]
    fn pty_close_persists_host_app() {
        // The PTY teardown path passes Some("app") and the durable stamp is written.
        let mut state = live_state_with_session("pty-1", "coder", None);
        apply_agent_session_close(&mut state, "pty-1", Some(HOST_APP), "2026-06-04T00:00:00Z");
        assert_eq!(state.sessions[0].status, "closed");
        assert_eq!(state.sessions[0].host.as_deref(), Some("app"));
    }

    #[test]
    fn missing_session_close_appends_no_event() {
        // Stopping an already-gone agent is idempotent: no synthetic event, no
        // hardcoded role. Only updated_at/rules housekeeping changes.
        let mut state = live_state_with_session("present", "coder", None);
        let events_before = state.events.len();
        apply_agent_session_close(&mut state, "absent", Some(HOST_APP), "2026-06-04T00:00:00Z");
        assert_eq!(
            state.events.len(),
            events_before,
            "no event for a missing session"
        );
        // The unrelated present session is untouched.
        assert_eq!(state.sessions[0].status, "active");
    }
}
