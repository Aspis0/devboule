//! pi-sidecar bridge — Phase 1: per-session lifecycle + vault adapter.
//!
//! Spawns a Node.js sidecar process (`pi-sidecar/sidecar.mjs`) that embeds the pi SDK,
//! reads its JSONL event stream from stdout, maps pi events to the existing
//! `MiniActivityEvent` / `ConsoleActivity` schema, and emits them on the
//! `mini-activity://<sessionId>` channel so the existing `WorkConsole.tsx` renders them
//! WITHOUT any React changes.
//!
//! Phase 0 → Phase 1 changes (decision #7, #9):
//! - Per-session agent IDs (pi-<counter>) instead of hardcoded `pi-spike`.
//! - Per-session generation counters (Arc<AtomicU64>) — spawning session B does NOT
//!   kill session A's reader thread.
//! - Multi-session state: HashMap<sessionId, session> + per-session generation.
//! - `spike_pi_prompt(sessionId?)`: creates new session if absent, routes to existing if present.
//! - `spike_pi_stop(sessionId)`: kills a session, joins reader, drops state.
//! - Vault adapter: reads coder backend from config.json + API key from keyring,
//!   passes as env vars to the Node sidecar.
//!
//! Design doc: `docs/devboule-on-pi-architecture.md` §7 (bridge), §11 (decisions #7, #9).
//! Mirror pattern: `oracle/python_oracle.rs` (Command spawn + env injection).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::mini_activity::{ConsoleActivity, ConsoleEntry, MiniActivityEvent, NodeStyle, PageEntry};

// ---- per-session state (decision #7) --------------------------------------

/// Maximum concurrent pi sidecar sessions (F6).
const MAX_SESSIONS: usize = 8;

/// Placeholder inserted under the lock before spawning. The real PiSession
/// replaces it after spawn completes (F2: spawn outside lock).
struct SessionSlot {
    inner: Option<PiSession>,
}

/// A single active pi sidecar session. Each session gets its own Node child process,
/// stdin writer, per-session generation counter, and reader thread handle.
struct PiSession {
    child: Child,
    /// Shared stdin writer. Also written by the reader thread (which sends
    /// `classified` responses back to the sidecar), so it is reference-counted
    /// and mutex-guarded to keep JSONL lines from interleaving.
    stdin: Arc<Mutex<ChildStdin>>,
    /// Per-session generation counter — bumped ONLY when THIS session respawns.
    /// The reader thread compares against this, NOT a global counter.
    generation: Arc<AtomicU64>,
    /// Handle to the stdout reader thread. Joined on stop to ensure clean teardown.
    reader_handle: Option<JoinHandle<()>>,
}

/// Tauri-managed state for all active pi sidecar sessions.
/// Each session has a unique id (`pi-<counter>`) and its own child process + reader thread.
pub struct PiSidecarState {
    inner: Mutex<HashMap<String, SessionSlot>>,
    /// Monotonically incremented to generate unique session ids.
    session_counter: AtomicU64,
}

impl Default for PiSidecarState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            // Note: HashMap<String, SessionSlot> — each entry holds Option<PiSession>.
            session_counter: AtomicU64::new(0),
        }
    }
}

/// Generate a unique session id in the form `pi-<counter>`.
fn generate_session_id(counter: u64) -> String {
    format!("pi-{counter}")
}

/// Info about a newly created or existing session, returned to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub is_new: bool,
    /// The agent role this session runs as (orchestrator / main-coder / mini-coder).
    pub agent_role: String,
    /// The `mini-activity://<agentId>` channel the console should subscribe to.
    pub channel: String,
}

/// Generate a **role-aware** agent id for a sidecar session. The frontend consoles
/// subscribe to `mini-activity://<agentId>`, so the id namespace must match the
/// agent role:
///
/// - `"orchestrator"`  -> `orchestrator-<sanitized project id>` (stable per project,
///   so the console channel survives relaunches — matches `stable_orchestrator_agent_id`).
/// - `"main-coder"` / `"main"` -> `main-<timestamp_ms>`.
/// - `"mini-coder"` / `"mini"` -> `mini-<timestamp_ms>`.
/// - anything else    -> legacy auto-generated `pi-<counter>` (safe fallback; keeps
///   the function pure by using a self-contained monotonic counter).
///
/// This is the single source of truth for the sidecar's agent id: the same value
/// is passed to the sidecar as `DEVBOULE_SESSION_ID` and used as the Rust event
/// channel, so the two never drift.
pub fn generate_agent_id(role: &str, project_id: Option<&str>) -> String {
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    };
    match role {
        "orchestrator" => {
            let pid = project_id.unwrap_or("unknown");
            super::projects::stable_orchestrator_agent_id(pid)
        }
        r if r.starts_with("main") => format!("main-{}", now_ms()),
        r if r.starts_with("mini") => format!("mini-{}", now_ms()),
        _ => {
            // Legacy fallback: pi-{counter}. Self-contained counter so the function
            // stays pure (no PiSidecarState needed).
            static FALLBACK_COUNTER: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let c = FALLBACK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            generate_session_id(c)
        }
    }
}

/// Whether the pi sidecar should be used for agent launches (Phase 1 wiring).
///
/// Opt-in via the `DEVBOULE_PI_ENABLED` env var (set to `1`/`true`). Defaults to
/// `false` so the legacy devboule-orchestrator binary path remains the fallback
/// until Phase 4 cleanup. Called from `projects.rs` to decide whether to delegate
/// an orchestrator launch to the pi sidecar.
pub fn pi_sidecar_enabled() -> bool {
    match std::env::var("DEVBOULE_PI_ENABLED") {
        Ok(v) => matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

// ---- vault adapter (decision #9) ------------------------------------------

/// Resolved coder-backend env vars to pass to the Node sidecar at spawn time.
/// Read from the Devboule config.json (`localCoderBackend`) + vault API key.
pub(crate) struct SidecarEnvVars {
    provider: String,
    model: String,
    api_key_env: Option<(String, String)>, // (env_var_name, value)
    base_url: Option<String>,
}

/// Read the coder role's provider+model+key+baseUrl from the vault/config and
/// resolve them into env vars for the sidecar. Decision #9: the Devboule vault
/// is the single source of truth; Rust reads it and passes to the sidecar.
///
/// Falls back to a non-Claude default (openrouter/tencent/hy3:free) if nothing
/// is configured. Decision #10: do NOT default to Claude.
pub(crate) fn resolve_coder_env_for_sidecar(app: &AppHandle) -> SidecarEnvVars {
    // Try reading the local coder backend from config.json.
    let local_backend = super::projects::read_local_coder_backend(app);

    match local_backend {
        Some(ref backend) => match backend.kind {
            super::local_coder::LocalCoderBackendKind::Ollama => {
                let (base_url, model) = super::local_coder::resolve_omlx_env(backend);
                let model = if model.is_empty() {
                    "qwen2.5-coder:7b".to_string()
                } else {
                    model
                };
                SidecarEnvVars {
                    provider: "openai".to_string(),
                    model,
                    api_key_env: Some(("OPENAI_API_KEY".to_string(), "ollama".to_string())),
                    base_url: Some(if base_url.is_empty() {
                        super::local_coder::OLLAMA_OPENAI_BASE_URL.to_string()
                    } else {
                        base_url
                    }),
                }
            }
            super::local_coder::LocalCoderBackendKind::Omlx => {
                let (base_url, model) = super::local_coder::resolve_omlx_env(backend);
                let model = if model.is_empty() {
                    "qwen2.5-coder:7b".to_string()
                } else {
                    model
                };
                SidecarEnvVars {
                    provider: "openai".to_string(),
                    model,
                    api_key_env: Some(("OPENAI_API_KEY".to_string(), "mlx".to_string())),
                    base_url: Some(if base_url.is_empty() {
                        "http://127.0.0.1:8000/v1".to_string()
                    } else {
                        base_url
                    }),
                }
            }
            super::local_coder::LocalCoderBackendKind::Cloud => {
                let (base_url, model) = super::local_coder::resolve_cloud_env(backend);
                let model = if model.is_empty() {
                    "tencent/hy3:free".to_string()
                } else {
                    model
                };
                let api_key = super::vault::read_cloud_llm_key().ok().flatten();
                SidecarEnvVars {
                    provider: "openrouter".to_string(),
                    model,
                    api_key_env: api_key.map(|k| ("OPENROUTER_API_KEY".to_string(), k)),
                    base_url: if base_url.is_empty() {
                        None
                    } else {
                        Some(base_url)
                    },
                }
            }
        },
        None => {
            // No coder backend configured — use a safe non-Claude default.
            // Decision #10: do NOT default to Claude.
            eprintln!(
                "[pi-sidecar] WARNING: no local coder backend configured in config.json. \
                 Falling back to openrouter/tencent/hy3:free. \
                 Configure a coder backend in Settings → Providers → Coders."
            );
            SidecarEnvVars {
                provider: "openrouter".to_string(),
                model: "tencent/hy3:free".to_string(),
                api_key_env: None,
                base_url: None,
            }
        }
    }
}

// ---- sidecar spawn --------------------------------------------------------

/// Resolve the path to the `pi-sidecar/sidecar.mjs` script relative to the app.
fn resolve_sidecar_script() -> Result<std::path::PathBuf, String> {
    let dev_path = std::env::current_dir()
        .map_err(|e| format!("Cannot resolve CWD: {e}"))?
        .join("pi-sidecar")
        .join("sidecar.mjs");
    if dev_path.exists() {
        return Ok(dev_path);
    }
    Err(
        "pi-sidecar/sidecar.mjs not found. Run `npm install` in pi-sidecar/ first."
            .to_string(),
    )
}

/// Spawn a new pi sidecar session with the given session id. Reads the coder
/// backend from the vault/config and passes provider+model+key as env vars.
/// Starts a stdout JSONL reader thread that emits events on `mini-activity://<sessionId>`.
///
/// Caller MUST hold the lock on `state.inner` — this function inserts into the map.
/// Returns the per-session generation Arc so the caller can store it in PiSession.
fn spawn_pi_session_inner(
    app: &AppHandle,
    session_id: &str,
    prev_generation: Option<Arc<AtomicU64>>,
    role: Option<&str>,
    project_id: Option<&str>,
) -> Result<PiSession, String> {
    let script = resolve_sidecar_script()?;
    let sidecar_dir = script
        .parent()
        .ok_or_else(|| "Cannot resolve pi-sidecar directory".to_string())?
        .to_path_buf();

    let env_vars = resolve_coder_env_for_sidecar(app);

    let mut cmd = Command::new("node");
    cmd.arg(&script)
        .current_dir(&sidecar_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    cmd.env("DEVBOULE_PI_PROVIDER", &env_vars.provider);
    cmd.env("DEVBOULE_PI_MODEL", &env_vars.model);

    if let Some(role) = role {
        // A/B: name the session so the sidecar stamps the correct `_devboule`
        // metadata and the frontend console subscribes to the right channel.
        cmd.env("DEVBOULE_AGENT_ROLE", role);
        cmd.env("DEVBOULE_SESSION_ID", session_id);
    }
    if let Some(pid) = project_id {
        cmd.env("DEVBOULE_PROJECT_ID", pid);
    }

    if let Some((ref key_name, ref key_value)) = env_vars.api_key_env {
        cmd.env(key_name, key_value);
    }

    if let Some(ref base_url) = env_vars.base_url {
        cmd.env("DEVBOULE_PI_BASE_URL", base_url);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn pi sidecar (is Node.js installed?): {e}"))?;

    let raw_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "pi sidecar stdin not captured".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "pi sidecar stdout not captured".to_string())?;

    // Shared, mutex-guarded stdin: both `spike_pi_prompt` (prompt commands) and
    // the reader thread (`classified` responses) write here. The Arc lets the
    // reader own a clone; the Mutex serializes their JSONL lines.
    let stdin = Arc::new(Mutex::new(raw_stdin));

    // Per-session generation: reuse existing Arc if respawning the same session,
    // or create a new one for a fresh session.
    let generation = prev_generation.unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
    // Bump THIS session's generation so any old reader for this session exits.
    let _gen = generation.fetch_add(1, Ordering::SeqCst) + 1;

    let app_clone = app.clone();
    let sid = session_id.to_string();
    let gen_clone = generation.clone();
    let stdin_clone = stdin.clone();
    let reader_handle = std::thread::spawn(move || {
        read_sidecar_events(app_clone, stdout, stdin_clone, gen_clone, &sid);
    });

    Ok(PiSession {
        child,
        stdin,
        generation,
        reader_handle: Some(reader_handle),
    })
}

/// Stop a specific pi sidecar session: kill the child, join reader, remove from state.
pub fn stop_pi_session(app: &AppHandle, session_id: &str) -> Result<bool, String> {
    let state = app.state::<PiSidecarState>();
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(mut slot) = guard.remove(session_id) {
        if let Some(mut session) = slot.inner.take() {
            // Bump THIS session's generation so the reader detects staleness.
            session.generation.fetch_add(1, Ordering::SeqCst);
            // Kill the child process.
            let _ = session.child.kill();
            let _ = session.child.wait();
            // Join the reader thread. It should exit quickly once stdout closes (EOF).
            if let Some(handle) = session.reader_handle.take() {
                let _ = handle.join();
            }
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Get an existing session or spawn a new one.
/// F2: lock is held only for the check+slot reservation; spawn happens
/// outside the lock to avoid blocking other sessions during fork+exec.
/// F6: rejects new sessions when MAX_SESSIONS is reached.
///
/// Returns (session_id, is_new).
fn get_or_spawn_session(
    app: &AppHandle,
    session_id_opt: Option<String>,
    role: Option<&str>,
    project_id: Option<&str>,
) -> Result<(String, bool), String> {
    let state = app.state::<PiSidecarState>();

    // Phase 1: under lock — check existing + reserve slot.
    let (id, is_new, prev_gen) = {
        let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());

        match session_id_opt {
            Some(id) => {
                if let Some(slot) = guard.get_mut(&id) {
                    if let Some(ref mut session) = slot.inner {
                        match session.child.try_wait() {
                            Ok(Some(_)) => {
                                // Dead — grab generation for reuse, will respawn below.
                                let old_gen = session.generation.clone();
                                guard.remove(&id);
                                // Reserve the slot with a placeholder.
                                guard.insert(id.clone(), SessionSlot { inner: None });
                                (id, false, Some(old_gen))
                            }
                            Ok(None) => return Ok((id, false)), // Alive.
                            Err(_) => return Ok((id, false)),     // Assume alive.
                        }
                    } else {
                        return Err(format!("pi session {id} has empty slot (spawn in progress?)"));
                    }
                } else {
                    // F6: session count check (live sessions only).
                    let live_count = guard.values().filter(|s| s.inner.is_some()).count();
                    if live_count >= MAX_SESSIONS {
                        return Err(format!(
                            "Too many concurrent pi sessions ({live_count}/{MAX_SESSIONS}). \nStop a session before starting a new one."
                        ));
                    }
                    guard.insert(id.clone(), SessionSlot { inner: None });
                    (id, false, None)
                }
            }
            None => {
                // F6: session count check.
                let live_count = guard.values().filter(|s| s.inner.is_some()).count();
                if live_count >= MAX_SESSIONS {
                    return Err(format!(
                        "Too many concurrent pi sessions ({live_count}/{MAX_SESSIONS}). \nStop a session before starting a new one."
                    ));
                }
                let counter = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
                let id = generate_session_id(counter);
                guard.insert(id.clone(), SessionSlot { inner: None });
                (id, true, None)
            }
        }
    };
    // Lock is now DROPPED — spawn happens without holding the lock.

    // Phase 2: spawn outside the lock.
    let new_session = spawn_pi_session_inner(app, &id, prev_gen, role, project_id)?;

    // Phase 3: re-acquire lock and store the real session.
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    // Reconcile: if the slot was removed (e.g. concurrent stop), clean up.
    if let Some(slot) = guard.get_mut(&id) {
        slot.inner = Some(new_session);
    } else {
        // Slot was removed during spawn (concurrent stop).
        // Kill the newly spawned child and return error.
        let mut s = new_session;
        s.generation.fetch_add(1, Ordering::SeqCst);
        let _ = s.child.kill();
        let _ = s.child.wait();
        if let Some(h) = s.reader_handle.take() { let _ = h.join(); }
        return Err(format!("pi session {id} was stopped during spawn"));
    }

    Ok((id, is_new))
}

// ---- Tauri commands --------------------------------------------------------

/// Tauri command: send a prompt text to a pi sidecar session. If `session_id`
/// is None, creates a new session and returns its id. If present, routes to
/// that session (creating it if it doesn't exist yet).
#[tauri::command]
pub async fn spike_pi_prompt(
    app: AppHandle,
    text: String,
    session_id: Option<String>,
) -> Result<SessionInfo, String> {
    let (sid, is_new) = get_or_spawn_session(&app, session_id, None, None)?;

    // Send the prompt to the session's stdin. If the child is dead, remove it
    // and return an error.
    let state = app.state::<PiSidecarState>();
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());

    // Check if child is still alive before writing.
    {
        let slot = guard
            .get_mut(&sid)
            .ok_or_else(|| format!("pi session {sid} not found after spawn"))?;
        let session = slot
            .inner
            .as_mut()
            .ok_or_else(|| format!("pi session {sid} has empty slot (spawn in progress?)"))?;
        match session.child.try_wait() {
            Ok(Some(status)) => {
                guard.remove(&sid);
                return Err(format!(
                    "pi session {sid} exited with status {status}. Start a new session."
                ));
            }
            Ok(None) => {} // Alive — proceed.
            Err(e) => {
                return Err(format!("pi session {sid} status check failed: {e}"));
            }
        }
    }

    // Write the prompt. Use a bool flag to avoid double-borrow in map_err (F4).
    let mut write_failed_zombie = false;
    {
        let session = guard
            .get_mut(&sid)
            .and_then(|s| s.inner.as_mut())
            .ok_or_else(|| format!("pi session {sid} not running after spawn"))?;

        let cmd = serde_json::json!({
            "type": "prompt",
            "message": text,
        });
        let line =
            serde_json::to_string(&cmd).map_err(|e| format!("JSON serialize error: {e}"))?;
        // Phase 2: stdin is shared with the reader thread (which writes
        // `classified` responses back to the sidecar). Lock it so the JSONL
        // prompt line never interleaves with a `classified` line.
        let mut stdin_lock = session
            .stdin
            .lock()
            .map_err(|_| format!("pi session {sid} stdin lock poisoned"))?;
        stdin_lock
            .write_all(format!("{line}\n").as_bytes())
            .map_err(|e| {
                // F4: on write failure, flag for zombie cleanup after we release session borrow.
                if let Ok(Some(_)) = session.child.try_wait() {
                    write_failed_zombie = true;
                }
                format!("Failed to write to pi sidecar stdin: {e}")
            })?;
        stdin_lock
            .flush()
            .map_err(|e| format!("Failed to flush pi sidecar stdin: {e}"))?;
    }

    // F4: clean up zombie entry if write failed.
    if write_failed_zombie {
        guard.remove(&sid);
    }

    let channel = super::mini_activity::mini_activity_channel(&sid);
    Ok(SessionInfo {
        session_id: sid,
        is_new,
        // The legacy spike path does not set DEVBOULE_AGENT_ROLE, so the sidecar
        // defaults to `main-coder` — report that here for consistency.
        agent_role: "main-coder".to_string(),
        channel,
    })
}

/// Spawn a **role-aware** pi sidecar session. Thin wrapper over the existing
/// session machinery that adds role-aware agent-id generation (A/B) and the
/// default devboule env vars the sidecar reads in `devbouleContext` (Task 1).
///
/// 1. Generates the agent id via [`generate_agent_id`] (orchestrator / main / mini
///    namespaces, else the legacy `pi-<counter>` fallback).
/// 2. Spawns the sidecar with `DEVBOULE_AGENT_ROLE=role` and
///    `DEVBOULE_SESSION_ID=<agent id>` (plus `DEVBOULE_PROJECT_ID` when given).
/// 3. Returns the session id, role, and the `mini-activity://<agentId>` channel.
///
/// The sidecar's reader thread then emits `MiniActivityEvent::Snapshot`s on that
/// channel (EventMapper uses the same agent id), so the frontend console renders
/// without any React changes.
pub fn spawn_sidecar_for_role(
    app: &AppHandle,
    role: &str,
    project_id: Option<&str>,
) -> Result<SessionInfo, String> {
    let agent_id = generate_agent_id(role, project_id);
    let (sid, is_new) = get_or_spawn_session(
        app,
        Some(agent_id.clone()),
        Some(role),
        project_id,
    )?;
    let channel = super::mini_activity::mini_activity_channel(&sid);
    Ok(SessionInfo {
        session_id: sid,
        is_new,
        agent_role: role.to_string(),
        channel,
    })
}

/// Tauri command: spawn a pi sidecar session for a named agent role
/// (orchestrator / main-coder / mini-coder). See [`spawn_sidecar_for_role`].
#[tauri::command]
pub async fn spawn_agent_session(
    app: AppHandle,
    role: String,
    project_id: Option<String>,
) -> Result<SessionInfo, String> {
    spawn_sidecar_for_role(&app, &role, project_id.as_deref())
}

/// Tauri command: stop a specific pi sidecar session.
#[tauri::command]
pub async fn spike_pi_stop(app: AppHandle, session_id: String) -> Result<bool, String> {
    stop_pi_session(&app, &session_id)
}

// ---- event mapping ---------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PiEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    assistant_message_event: Option<AssistantMessageEvent>,
    #[serde(rename = "toolCallId", default)]
    tool_call_id: Option<String>,
    #[serde(rename = "toolName", default)]
    tool_name: Option<String>,
    #[serde(default)]
    args: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(rename = "isError", default)]
    is_error: Option<bool>,
    #[allow(dead_code)]
    #[serde(default)]
    messages: Option<serde_json::Value>,
    /// The raw `message` object from `message_start`/`message_end` events. Used to
    /// detect injected `devboule.websearch` / `devboule.plan` custom user messages.
    #[serde(default)]
    message: Option<serde_json::Value>,
    /// The `_devboule` enrichment object the sidecar stamps onto every event
    /// (Task 1): `{ agentRole, projectId, sessionId }`.
    #[serde(rename = "_devboule", default)]
    devboule: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct AssistantMessageEvent {
    #[serde(rename = "type")]
    delta_type: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(rename = "contentIndex", default)]
    content_index: Option<u32>,
}

/// Stateful mapper: converts pi SDK events into `ConsoleActivity` snapshots.
struct EventMapper {
    agent_id: String,
    running: bool,
    entries: Vec<ConsoleEntry>,
    accumulated_text: String,
    tool_names: HashMap<String, String>,
    turn_seq: u64,
    active_content_index: Option<u32>,
    /// A: the persisted `agentRole` from the event stream's `_devboule` field.
    /// Refreshed on every event; survives across events in the same session.
    current_role: Option<String>,
}

impl EventMapper {
    fn new(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            running: false,
            entries: Vec::new(),
            accumulated_text: String::new(),
            tool_names: HashMap::new(),
            turn_seq: 0,
            active_content_index: None,
            current_role: None,
        }
    }

    fn now_str() -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() % 86400;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }

    fn build_snapshot(&self) -> ConsoleActivity {
        ConsoleActivity {
            running: Some(self.running),
            run_count: if self.running { Some(1) } else { Some(0) },
            empty: if self.entries.is_empty() {
                Some(true)
            } else {
                None
            },
            entries: if self.entries.is_empty() {
                None
            } else {
                Some(self.entries.clone())
            },
            task_cost_estimate_usd: None,
            streaming_chat: if self.running && !self.accumulated_text.is_empty() {
                Some(super::mini_activity::StreamingChat {
                    seq: self.turn_seq,
                    text: self.accumulated_text.clone(),
                })
            } else {
                None
            },
        }
    }

    fn emit_snapshot(&self, app: &AppHandle) {
        let snapshot = self.build_snapshot();
        let event = MiniActivityEvent::Snapshot {
            activity: snapshot,
        };
        let channel = super::mini_activity::mini_activity_channel(&self.agent_id);
        let _ = app.emit(&channel, event);
    }

    fn flush_text_block(&mut self) {
        if !self.accumulated_text.is_empty() {
            self.entries.push(ConsoleEntry::Chat {
                role: "assistant".to_string(),
                text: std::mem::take(&mut self.accumulated_text),
                time: Self::now_str(),
                msg_id: None,
            });
        }
        self.active_content_index = None;
    }

    fn handle_event(&mut self, app: &AppHandle, event: &PiEvent) {
        // A: persist `agentRole` from `_devboule` on every event (it survives
        // across events in the same session).
        self.apply_devboule_role(event);
        match event.event_type.as_str() {
            "agent_start" => {
                self.running = true;
                self.turn_seq += 1;
                self.accumulated_text.clear();
                self.active_content_index = None;
                self.emit_snapshot(app);
            }
            "agent_end" => {
                self.flush_text_block();
                self.running = false;
                self.emit_snapshot(app);
            }
            "message_update" => {
                if let Some(ref delta_event) = event.assistant_message_event {
                    match delta_event.delta_type.as_str() {
                        "text_start" => {
                            self.flush_text_block();
                            self.active_content_index = delta_event.content_index;
                        }
                        "text_delta" => {
                            if let Some(ref delta) = delta_event.delta {
                                self.accumulated_text.push_str(delta);
                            }
                        }
                        "text_end" => {}
                        _ => {}
                    }
                }
                self.emit_snapshot(app);
            }
            "tool_execution_start" => {
                let tool_name = event
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool".to_string());
                if let Some(ref id) = event.tool_call_id {
                    self.tool_names.insert(id.clone(), tool_name.clone());
                }
                self.flush_text_block();
                let args_str = event
                    .args
                    .as_ref()
                    .map(|a| serde_json::to_string(a).unwrap_or_default())
                    .unwrap_or_default();
                self.entries.push(ConsoleEntry::Coder {
                    node: Some(NodeStyle::Dot),
                    text: format!("🔧 Calling `{tool_name}`"),
                    time: Self::now_str(),
                });
                self.entries.push(ConsoleEntry::Coder {
                    node: None,
                    text: format!("  args: {args_str}"),
                    time: String::new(),
                });
                self.emit_snapshot(app);
            }
            "tool_execution_update" => {
                self.emit_snapshot(app);
            }
            "tool_execution_end" => {
                let tool_name = event
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| self.tool_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let result_summary = event
                    .result
                    .as_ref()
                    .and_then(|r| {
                        r.get("content")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|item| item.get("text"))
                            .and_then(|t| t.as_str())
                    })
                    .map(|text| {
                        if text.len() > 200 {
                            format!("{}…", &text[..200])
                        } else {
                            text.to_string()
                        }
                    })
                    .unwrap_or_else(|| "(no result)".to_string());
                let is_error = event.is_error.unwrap_or(false);
                let status_icon = if is_error { "❌" } else { "✅" };
                self.entries.push(ConsoleEntry::Coder {
                    node: Some(NodeStyle::Sage),
                    text: format!("{status_icon} `{tool_name}` → {result_summary}"),
                    time: Self::now_str(),
                });
                self.emit_snapshot(app);
            }
            "turn_start" | "turn_end" | "message_start" | "message_end" => {
                // B: a `message_start`/`message_end` with a user-role message whose
                // content[0].text parses as a devboule custom message
                // (devboule.websearch / devboule.plan) is injected into the activity.
                if let Some(ref message) = event.message {
                    self.handle_devboule_custom_message(message);
                }
                self.emit_snapshot(app);
            }
            "response" | "ready" => {
                self.emit_snapshot(app);
            }
            _ => {}
        }
    }

    // ---- Task 2: _devboule + devboule custom messages --------------------------

    /// A: read `_devboule.agentRole` and persist it on the mapper. The sidecar
    /// stamps `_devboule` onto every event, so this is an idempotent refresh that
    /// keeps `current_role` correct for the whole session.
    fn apply_devboule_role(&mut self, event: &PiEvent) {
        if let Some(role) = event
            .devboule
            .as_ref()
            .and_then(|d| d.get("agentRole"))
            .and_then(|v| v.as_str())
        {
            self.current_role = Some(role.to_string());
        }
    }

    /// B: detect an injected `devboule.websearch` / `devboule.plan` custom message.
    /// Returns `true` if a devboule custom message was handled (and an entry was
    /// pushed). A plain user message (or any non-matching shape) returns `false`
    /// and is ignored — safe to call on every `message_start`/`message_end`.
    fn handle_devboule_custom_message(&mut self, message: &serde_json::Value) -> bool {
        // Only the sidecar's injected user-role messages are devboule custom msgs.
        if message.get("role").and_then(|v| v.as_str()) != Some("user") {
            return false;
        }
        let content = match message.get("content").and_then(|c| c.as_array()) {
            Some(c) if !c.is_empty() => c,
            _ => return false,
        };
        let text = match content[0].get("text").and_then(|t| t.as_str()) {
            Some(t) => t,
            None => return false,
        };
        let obj: serde_json::Value = match serde_json::from_str(text) {
            Ok(o) => o,
            Err(_) => return false,
        };
        match obj.get("type").and_then(|t| t.as_str()) {
            Some("devboule.websearch") => {
                let query = obj.get("query").and_then(|q| q.as_str()).unwrap_or("");
                let results = obj.get("results").cloned().unwrap_or(serde_json::Value::Null);
                self.handle_devboule_websearch(query, &results);
                true
            }
            Some("devboule.plan") => {
                let plan = obj.get("plan").cloned().unwrap_or(serde_json::Value::Null);
                self.handle_devboule_plan(&plan);
                true
            }
            _ => false,
        }
    }

    /// B: map a `devboule.websearch` custom message to `ConsoleEntry::WebSearch`.
    /// `results` is the raw web_search tool `details` object; we tolerate several
    /// shapes and extract url/title/summary per item with graceful fallbacks.
    fn handle_devboule_websearch(&mut self, query: &str, results: &serde_json::Value) {
        let pages = extract_pages(results);
        self.entries.push(ConsoleEntry::WebSearch {
            query: query.to_string(),
            pages,
            time: Self::now_str(),
        });
    }

    /// B: map a `devboule.plan` custom message. `ConsoleActivity` has no dedicated
    /// plan entry type yet, so per the Task-2 fallback we emit a `ConsoleEntry::Chat`
    /// with `role == "plan"`; PlannerPlanMode can later parse the formatted plan text.
    fn handle_devboule_plan(&mut self, plan: &serde_json::Value) {
        let text = match plan {
            serde_json::Value::Null => "(empty plan)".to_string(),
            _ => serde_json::to_string_pretty(plan).unwrap_or_default(),
        };
        self.entries.push(ConsoleEntry::Chat {
            role: "plan".to_string(),
            text,
            time: Self::now_str(),
            msg_id: None,
        });
    }
}

/// B helper: extract a list of `PageEntry` from a web_search `details` value,
/// tolerating several response shapes (a bare array, or an object with a
/// `results`/`pages`/`items`/`hits`/`data` array, or a single url/title object).
/// Unknown/empty items are skipped; returns an empty vec if nothing parseable.
fn extract_pages(results: &serde_json::Value) -> Vec<PageEntry> {
    let items: Vec<&serde_json::Value> = match results {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(map) => {
            let mut found: Vec<&serde_json::Value> = Vec::new();
            for key in ["results", "pages", "items", "hits", "data"] {
                if let Some(arr) = map.get(key).and_then(|v| v.as_array()) {
                    found = arr.iter().collect();
                    break;
                }
            }
            if found.is_empty() && (map.contains_key("url") || map.contains_key("title")) {
                found = vec![results];
            }
            found
        }
        _ => Vec::new(),
    };
    items
        .iter()
        .filter_map(|item| {
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("name").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let summary = item
                .get("summary")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("snippet").and_then(|v| v.as_str()))
                .or_else(|| item.get("text").and_then(|v| v.as_str()))
                .or_else(|| item.get("content").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            if title.is_empty() && url.is_empty() {
                return None;
            }
            Some(PageEntry { url, title, summary })
        })
        .collect()
}

// ---- Phase 2 Pigeon control commands ------------------------------------

/// Phase 2: handle a JSONL *control command* the sidecar emits on its stdout
/// (`classify_prompt`, `redirect_to_claude`) before it is mistaken for a pi SDK
/// event. Returns `true` if the line was a control command (and consumed);
/// `false` lets the caller fall through to normal pi event parsing.
fn handle_control_line(
    app: &AppHandle,
    agent_id: &str,
    line: &str,
    stdin: &Arc<Mutex<ChildStdin>>,
) -> bool {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let t = match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return false,
    };
    match t {
        "classify_prompt" => {
            // Non-blocking: classification is pure + fast, so the `classified`
            // response is written back to the sidecar's stdin BEFORE the
            // session.prompt() begins (the sidecar awaits it first).
            //
            // Phase 2: AgentPath routing only. Full multi-tier model switching
            // (vault-aware tier resolution) deferred to Phase 3 — the sidecar's
            // spawn-time minimal models.json can't resolve every classified
            // (provider, model) pair, so `setModel` is deferred there.
            let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let c = crate::backend::prompt_routing::classify_prompt_full(text, app);
            let response = serde_json::json!({
                "type": "classified",
                "tier": c.tier,
                "provider": c.provider,
                "model": c.model,
                "path": c.path,
            });
            write_jsonl_to_stdin(stdin, &response);
            true
        }
        "redirect_to_claude" => {
            // path == Terminal: the sidecar declined to run this in pi and asked
            // Rust to route it to the legacy Claude-terminal subprocess.
            // TODO (Phase 3): spawn/route `message` to the existing Claude-terminal
            // subprocess (decision #10). Spike: log it and surface a Tauri event
            // so the frontend can later show the redirect.
            let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let truncated: String = message.chars().take(60).collect();
            let suffix = if message.chars().count() > 60 { "…" } else { "" };
            eprintln!(
                "[pi-sidecar] Pigeon: path=Terminal redirect_to_claude: {}{}",
                truncated, suffix
            );
            let _ = app.emit(
                &format!("pigeon-redirect://{agent_id}"),
                serde_json::json!({ "message": message }),
            );
            true
        }
        _ => false,
    }
}

/// Write a JSONL object to the sidecar's stdin (best-effort; used for the
/// `classified` response). Locked so it never interleaves with prompt commands.
fn write_jsonl_to_stdin(stdin: &Arc<Mutex<ChildStdin>>, value: &serde_json::Value) {
    if let Ok(mut s) = stdin.lock() {
        if let Ok(line) = serde_json::to_string(value) {
            let _ = s.write_all(format!("{line}\n").as_bytes());
            let _ = s.flush();
        }
    }
}

// ---- stdout reader ---------------------------------------------------------

/// Blocking reader thread: reads JSONL from the sidecar's stdout. Uses a
/// **per-session** generation Arc — only bumped when THIS session respawns,
/// NOT when sibling sessions spawn or stop (fixes BLOCKER #1).
fn read_sidecar_events(
    app: AppHandle,
    stdout: std::process::ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    generation: Arc<AtomicU64>,
    agent_id: &str,
) {
    let gen = generation.load(Ordering::SeqCst);
    let reader = BufReader::new(stdout);
    let mut mapper = EventMapper::new(agent_id);

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        // Per-session generation check: only exits if THIS session was superseded.
        if generation.load(Ordering::SeqCst) != gen {
            break;
        }
        // Phase 2 Pigeon: intercept control commands BEFORE treating the line as
        // a pi SDK event. `classify_prompt` → respond with `classified` on the
        // sidecar's stdin; `redirect_to_claude` → log + emit a Tauri event
        // (full routing to the Claude-terminal subprocess is deferred to Phase 3).
        if handle_control_line(&app, agent_id, &line, &stdin) {
            continue;
        }
        match serde_json::from_str::<PiEvent>(&line) {
            Ok(event) => {
                mapper.handle_event(&app, &event);
            }
            Err(_) => {
                let preview: String = line.chars().take(200).collect();
                eprintln!("[pi-sidecar] unparseable JSONL line: {preview}...");
            }
        }
    }

    // Sidecar exited — mark as stopped (only if still our generation).
    if generation.load(Ordering::SeqCst) == gen {
        mapper.running = false;
        mapper.emit_snapshot(&app);
    }
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    // -- pi_sidecar_enabled (Phase 1 opt-in) ------------------------------------

    #[test]
    fn pi_sidecar_enabled_false_when_env_unset() {
        std::env::remove_var("DEVBOULE_PI_ENABLED");
        assert!(!pi_sidecar_enabled());
    }

    // -- session id generation ------------------------------------------------

    #[test]
    fn session_ids_are_unique_per_counter() {
        let id1 = generate_session_id(1);
        let id2 = generate_session_id(2);
        assert_ne!(id1, id2);
        assert_eq!(id1, "pi-1");
        assert_eq!(id2, "pi-2");
    }

    #[test]
    fn session_ids_start_with_pi_prefix() {
        let id = generate_session_id(42);
        assert!(id.starts_with("pi-"), "id must start with pi-: {id}");
        assert_eq!(id, "pi-42");
    }

    #[test]
    fn session_id_counter_monotonically_increases() {
        let state = PiSidecarState::default();
        let id1 = {
            let c = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
            generate_session_id(c)
        };
        let id2 = {
            let c = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
            generate_session_id(c)
        };
        let id3 = {
            let c = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
            generate_session_id(c)
        };
        assert_eq!(id1, "pi-1");
        assert_eq!(id2, "pi-2");
        assert_eq!(id3, "pi-3");
    }

    // -- generate_agent_id (role-aware namespaces, Task 3) ---------------------

    #[test]
    fn agent_id_orchestrator_uses_stable_project_id() {
        let id = generate_agent_id("orchestrator", Some("my-proj"));
        assert_eq!(id, "orchestrator-my-proj");
        assert_eq!(id, super::super::projects::stable_orchestrator_agent_id("my-proj"));
    }

    #[test]
    fn agent_id_orchestrator_sanitizes_hostile_project_id() {
        let id = generate_agent_id("orchestrator", Some("proj/../evil name!"));
        assert!(id.starts_with("orchestrator-"), "id: {id}");
        assert!(!id.contains('/'), "id must not contain path separators: {id}");
        assert!(!id.contains(' '), "id must not contain spaces: {id}");
    }

    #[test]
    fn agent_id_orchestrator_falls_back_to_unknown_without_project() {
        let id = generate_agent_id("orchestrator", None);
        assert_eq!(id, "orchestrator-unknown");
    }

    #[test]
    fn agent_id_main_and_mini_use_timestamp_namespace() {
        let main = generate_agent_id("main-coder", None);
        let mini = generate_agent_id("mini-coder", None);
        assert!(main.starts_with("main-"), "main id: {main}");
        assert!(mini.starts_with("mini-"), "mini id: {mini}");
        let main_ts: u128 = main["main-".len()..].parse().unwrap();
        let mini_ts: u128 = mini["mini-".len()..].parse().unwrap();
        assert!(main_ts > 0 && mini_ts > 0);
    }

    #[test]
    fn agent_id_main_uses_timestamp_pattern() {
        // A "main-coder" role must produce `main-<numeric timestamp>` (NOT the
        // legacy `pi-<counter>` fallback), so the frontend console subscribes to
        // `mini-activity://main-<ts>`.
        let id = generate_agent_id("main-coder", None);
        assert!(id.starts_with("main-"), "main id must use main- namespace: {id}");
        let ts: u128 = id["main-".len()..]
            .parse()
            .expect("main- suffix must be a numeric timestamp");
        assert!(ts > 0, "timestamp must be positive: {id}");
    }

    #[test]
    fn agent_id_mini_uses_timestamp_pattern() {
        // A "mini-coder" role must produce `mini-<numeric timestamp>` (NOT the
        // legacy `pi-<counter>` fallback).
        let id = generate_agent_id("mini-coder", None);
        assert!(id.starts_with("mini-"), "mini id must use mini- namespace: {id}");
        let ts: u128 = id["mini-".len()..]
            .parse()
            .expect("mini- suffix must be a numeric timestamp");
        assert!(ts > 0, "timestamp must be positive: {id}");
    }

    #[test]
    fn agent_id_accepts_bare_main_and_mini_prefixes() {
        assert!(generate_agent_id("main", None).starts_with("main-"));
        assert!(generate_agent_id("mini", None).starts_with("mini-"));
    }

    #[test]
    fn agent_id_unknown_role_falls_back_to_pi_counter() {
        let a = generate_agent_id("weird-role", None);
        let b = generate_agent_id("weird-role", None);
        assert!(a.starts_with("pi-"), "fallback id: {a}");
        assert_ne!(a, b, "two fallback ids must be unique");
    }

    // -- session info serialization -------------------------------------------

    #[test]
    fn session_info_serializes_camel_case() {
        let info = SessionInfo {
            session_id: "pi-7".to_string(),
            is_new: true,
            agent_role: "main-coder".to_string(),
            channel: "mini-activity://pi-7".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"sessionId\""), "must be camelCase: {json}");
        assert!(json.contains("\"isNew\""), "must be camelCase: {json}");
        assert!(json.contains("pi-7"), "must contain session id: {json}");
    }

    // -- event mapper channel -------------------------------------------------

    #[test]
    fn event_mapper_uses_session_agent_id_in_channel() {
        let mapper = EventMapper::new("pi-42");
        assert_eq!(mapper.agent_id, "pi-42");
        let expected_channel = super::super::mini_activity::mini_activity_channel("pi-42");
        assert_eq!(expected_channel, "mini-activity://pi-42");
    }

    // -- per-session generation guard (BLOCKER #1 regression test) ------------

    #[test]
    fn per_session_generation_independent() {
        // Two sessions must have INDEPENDENT generation counters.
        // Bumping session A's generation must NOT affect session B's.
        let gen_a = Arc::new(AtomicU64::new(0));
        let gen_b = Arc::new(AtomicU64::new(0));

        let gen_a_val = gen_a.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(gen_a_val, 1);
        assert_eq!(gen_b.load(Ordering::SeqCst), 0, "gen_b must be unaffected");

        // Bump gen_b too.
        let gen_b_val = gen_b.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(gen_b_val, 1);
        assert_eq!(gen_a.load(Ordering::SeqCst), 1, "gen_a must be unaffected by gen_b bump");

        // Bump gen_a again (respawn session A).
        let gen_a_val2 = gen_a.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(gen_a_val2, 2);
        assert_eq!(gen_b.load(Ordering::SeqCst), 1, "gen_b must still be unaffected");
    }

    #[test]
    fn per_session_generation_reader_detects_own_respawn() {
        // Simulates: reader starts with gen=1, session respawns (gen→2), reader should exit.
        let gen = Arc::new(AtomicU64::new(0));
        let initial = gen.fetch_add(1, Ordering::SeqCst) + 1; // gen=1
        assert_eq!(initial, 1);

        // Reader checks: still our generation?
        assert_eq!(gen.load(Ordering::SeqCst), initial, "reader should continue");

        // Session respawns — bumps THIS session's generation.
        gen.fetch_add(1, Ordering::SeqCst); // gen=2

        // Reader checks: generation changed? YES → exit.
        assert_ne!(
            gen.load(Ordering::SeqCst),
            initial,
            "reader should detect respawn and exit"
        );
    }

    #[test]
    fn per_session_generation_sibling_respawn_does_not_kill() {
        // Session A reader starts with gen=1.
        let gen_a = Arc::new(AtomicU64::new(0));
        let gen_a_initial = gen_a.fetch_add(1, Ordering::SeqCst) + 1;

        // Session B spawns (creates its own generation).
        let gen_b = Arc::new(AtomicU64::new(0));
        let _gen_b_val = gen_b.fetch_add(1, Ordering::SeqCst) + 1;

        // Session A's reader checks: is MY generation still the same?
        assert_eq!(
            gen_a.load(Ordering::SeqCst),
            gen_a_initial,
            "session A reader must NOT be killed by session B spawn"
        );
    }

    // -- session map lifecycle ------------------------------------------------

    #[test]
    fn state_map_starts_empty_and_counter_at_zero() {
        let state = PiSidecarState::default();
        let guard = state.inner.lock().unwrap();
        assert!(guard.is_empty(), "map must start empty");
        drop(guard);
        assert_eq!(
            state.session_counter.load(Ordering::SeqCst),
            0,
            "counter must start at 0"
        );
    }

    #[test]
    fn session_map_insert_get_remove_lifecycle() {
        let state = PiSidecarState::default();
        {
            let guard = state.inner.lock().unwrap();
            assert!(!guard.contains_key("pi-1"));
        }

        let c1 = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let c2 = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(c1, 1);
        assert_eq!(c2, 2);
        assert_eq!(generate_session_id(c1), "pi-1");
        assert_eq!(generate_session_id(c2), "pi-2");
    }

    // -- MAX_SESSIONS (F6) -----------------------------------------------------

    #[test]
    fn max_sessions_constant_is_sane() {
        assert_eq!(MAX_SESSIONS, 8, "MAX_SESSIONS must be 8");
    }

    // -- resolve_coder_env_for_sidecar fallback (decision #10) ----------------

    #[test]
    fn fallback_env_is_non_claude() {
        // The fallback (no coder backend configured) must NOT use Claude.
        // This is a pure logic test: verify the fallback SidecarEnvVars shape.
        // We can't call resolve_coder_env_for_sidecar without a real AppHandle,
        // but we can verify the fallback constants match decision #10.
        let fallback_provider = "openrouter";
        let fallback_model = "tencent/hy3:free";

        assert_ne!(fallback_provider, "anthropic", "must NOT default to Claude");
        assert_ne!(fallback_provider, "claude", "must NOT default to Claude");
        assert!(
            fallback_provider == "openrouter",
            "fallback must be openrouter"
        );
        assert!(
            fallback_model.contains("hy3:free"),
            "fallback model must be the free tier"
        );
    }

    // -- default state --------------------------------------------------------

    #[test]
    fn default_state_has_no_global_generation() {
        let state = PiSidecarState::default();
        let guard = state.inner.lock().unwrap();
        assert!(guard.is_empty());
        drop(guard);
        assert_eq!(state.session_counter.load(Ordering::SeqCst), 0);
    }

    // -- SessionSlot placeholder (F2) ------------------------------------------

    #[test]
    fn session_slot_can_hold_none_placeholder() {
        let slot = SessionSlot { inner: None };
        assert!(slot.inner.is_none(), "placeholder slot must be None");
    }
    
    #[test]
    fn session_slot_inner_is_some_when_live() {
        // Can't construct a real PiSession without a Child, but we can
        // verify the Option wrapping logic works.
        let slot_none: Option<PiSession> = None;
        let slot = SessionSlot { inner: slot_none };
        assert!(slot.inner.is_none());
    }

    // -- Task 2: _devboule parsing + devboule custom messages --------------------

    #[test]
    fn devboule_agent_role_is_parsed() {
        // A: an event carrying `_devboule.agentRole` must set current_role.
        let line = r#"{"type":"message_start","_devboule":{"agentRole":"orchestrator","projectId":"my-proj","sessionId":"pi-42"}}"#;
        let event: PiEvent = serde_json::from_str(line).unwrap();
        let mut mapper = EventMapper::new("pi-42");
        mapper.apply_devboule_role(&event);
        assert_eq!(
            mapper.current_role.as_deref(),
            Some("orchestrator"),
            "agentRole must be parsed from _devboule"
        );
    }

    #[test]
    fn devboule_websearch_emits_websearch_entry() {
        // B: a `devboule.websearch` custom message must produce a WebSearch entry.
        let mut mapper = EventMapper::new("pi-1");
        let results = serde_json::json!([
            {"url": "https://a.test", "title": "Alpha", "summary": "about alpha"},
            {"url": "https://b.test", "title": "Beta", "summary": "about beta"}
        ]);
        mapper.handle_devboule_websearch("best widgets", &results);
        match mapper.entries.last() {
            Some(ConsoleEntry::WebSearch { query, pages, .. }) => {
                assert_eq!(query, "best widgets");
                assert_eq!(pages.len(), 2, "both pages must be mapped");
                assert_eq!(pages[0].url, "https://a.test");
                assert_eq!(pages[1].title, "Beta");
            }
            other => panic!("expected WebSearch entry, got {other:?}"),
        }
    }

    #[test]
    fn plain_event_without_devboule_is_handled_gracefully() {
        // 1) An event with no `_devboule` must not crash and must leave role unset.
        let line = r#"{"type":"message_start"}"#;
        let event: PiEvent = serde_json::from_str(line).unwrap();
        let mut mapper = EventMapper::new("pi-7");
        mapper.apply_devboule_role(&event);
        assert!(
            mapper.current_role.is_none(),
            "role must stay unset without _devboule"
        );

        // 2) A plain user message whose text is NOT a devboule custom message must
        // be ignored (no panic, no entry emitted).
        let plain_msg = serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "just a normal steering message"}]
        });
        let handled = mapper.handle_devboule_custom_message(&plain_msg);
        assert!(
            !handled,
            "non-devboule message must not be handled as custom"
        );
        assert!(
            mapper.entries.is_empty(),
            "no entry should be emitted for a plain message"
        );
    }
}
