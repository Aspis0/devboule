//! Phase D: run a CLOUD orchestrator CLI (Claude / Codex) in its DUPLEX structured-streaming
//! mode — piped stdin/stdout, NOT a PTY — normalize its event stream into the SAME activity
//! bridge `.jsonl` the local orchestrator writes, and route the planner chat's messages to its
//! stdin. By writing the bridge file, `start_activity_tail` + the Stage + durable
//! `read_activity_chat` are all reused unchanged: a cloud orchestrator drives the identical
//! planner Stage (chat / token-streaming / websearch) as the local one.
//!
//! The reader thread owns a per-session normalizer ([`ClaudeNormalizer`] / [`CodexNormalizer`]),
//! appends its emitted bridge lines to the activity file, and on EOF REAPS the child + removes
//! itself from the registry (no zombie / no thread leak). Steering writes a provider-encoded user
//! turn to the child's stdin. The child handle lives behind a shared `Arc<Mutex<Option<Child>>>`
//! so whichever of {reader EOF, explicit kill} runs first reaps it exactly once.
//!
//! ⚠️ The Claude path uses REAL captured event shapes. The Codex path is from the documented
//! app-server protocol (no local Codex to capture) and the stdin handshake/encoding is
//! best-effort — both the Codex normalizer and [`encode_user_turn`]'s Codex arm must be validated
//! against real `codex app-server` output in e2e (the app-server likely needs an
//! `initialize`/`newThread` handshake before `sendUserMessage`, which is NOT implemented).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{Emitter, Manager};

use super::cloud_claude::ClaudeNormalizer;
use super::cloud_codex::CodexNormalizer;

/// Which cloud CLI + protocol this session speaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    /// Parse the client id used elsewhere (`"claude"` / `"codex"`).
    pub fn from_client(client: &str) -> Option<Self> {
        match client {
            "claude" => Some(Provider::Claude),
            "codex" => Some(Provider::Codex),
            _ => None,
        }
    }
}

/// Max wait for each step of the Codex `initialize` → `thread/start` handshake before the
/// driver gives up (a hung handshake must not park the driver thread forever).
const CODEX_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Max wait for a user approval decision before the per-approval waiter thread auto-declines.
/// Verified-open Codex bug #21982: an approval request may never resolve, so a turn must never
/// hang forever — 120s is well under Codex's ~300s turn timeout, so we reply (decline) in time.
const CODEX_APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Max concurrent Codex approval-waiter threads (each parks up to CODEX_APPROVAL_TIMEOUT).
/// Beyond this we decline immediately instead of spawning, bounding thread/FD usage against a
/// misbehaving or adversarial Codex that spams `requestApproval`.
const MAX_INFLIGHT_APPROVALS: usize = 32;

/// Process-wide monotonic source of per-session nonces. Each `CodexClient::new()` claims one, so
/// two sessions for the SAME `agent_id` (e.g. an immediate relaunch) get distinct `approval_id`
/// namespaces — an old lingering waiter can never `cancel()` the new session's identically-keyed
/// waiter (per-client request ids restart at 1 on relaunch).
static CODEX_SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

/// A provider-agnostic normalizer the reader loop drives.
enum Normalizer {
    Claude(ClaudeNormalizer),
    Codex(CodexNormalizer),
}

impl Normalizer {
    fn feed(&mut self, line: &str) -> Vec<String> {
        match self {
            Normalizer::Claude(n) => n.feed_line(line),
            Normalizer::Codex(n) => n.feed_line(line),
        }
    }
}

/// Encode ONE user turn for the provider's stdin stream. PURE + testable.
/// - Claude `--input-format stream-json`: a `user` NDJSON message line.
/// - Codex app-server: a JSON-RPC `sendUserMessage` notification (best-effort; see module note).
pub fn encode_user_turn(provider: Provider, msg: &str) -> String {
    let mut line = match provider {
        Provider::Claude => serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": msg }] }
        })
        .to_string(),
        Provider::Codex => serde_json::json!({
            "jsonrpc": "2.0",
            "method": "sendUserMessage",
            "params": { "items": [{ "type": "text", "text": msg }] }
        })
        .to_string(),
    };
    line.push('\n');
    line
}

/// A live duplex session: the child (shared so the reader OR kill reaps it once), its stdin (for
/// steering), the reader thread, and an exited flag.
struct DuplexSession {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<ChildStdin>>,
    reader: Option<JoinHandle<()>>,
    exited: Arc<AtomicBool>,
    provider: Provider,
    /// Owning project id — carried so the reader thread can stamp `ConsentRequest.project_id`
    /// when a Codex approval server-request arrives.
    #[allow(dead_code)]
    project_id: String,
    /// Codex JSON-RPC correlator. `Some` only for Codex sessions; `None` for Claude (the
    /// Claude path keeps its byte-identical NDJSON behaviour and never touches this).
    codex: Option<Arc<CodexClient>>,
}

/// Tauri-managed map of agent_id -> live duplex session. Registered in lib.rs via `.manage(...)`.
#[derive(Default)]
pub struct CloudDuplexSessions {
    inner: Mutex<HashMap<String, DuplexSession>>,
}

impl CloudDuplexSessions {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Append one bridge line (+newline) to the activity file in ONE atomic `write_all` (so a partial
/// write can never leave a half-line the tail parser would choke on). Best-effort: any I/O error
/// no-ops, so the read loop never dies on a transient FS hiccup.
fn append_bridge_line(path: &std::path::Path, line: &str) {
    if path.as_os_str().is_empty() {
        return;
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let mut buf = String::with_capacity(line.len() + 1);
        buf.push_str(line);
        buf.push('\n');
        let _ = f.write_all(buf.as_bytes());
    }
}

/// Reap (kill + wait) the shared child exactly once, taking it out of the `Option`. Idempotent.
fn reap_child(child: &Arc<Mutex<Option<Child>>>, kill: bool) {
    if let Ok(mut guard) = child.lock() {
        if let Some(mut c) = guard.take() {
            if kill {
                let _ = c.kill();
            }
            let _ = c.wait(); // reap — no zombie
        }
    }
}

/// Spawn `program args` as a PIPED child speaking the duplex protocol for `provider`, stream its
/// normalized activity into `activity_file`, and (if `initial_goal` is set) send it as the first
/// user turn. The session is registered under `agent_id` (an existing one is killed+replaced).
#[allow(clippy::too_many_arguments)]
pub fn spawn_cloud_duplex(
    app: &tauri::AppHandle,
    sessions: &CloudDuplexSessions,
    agent_id: &str,
    provider: Provider,
    program: &str,
    args: &[String],
    envs: &[(String, String)],
    cwd: &std::path::Path,
    activity_file: PathBuf,
    initial_goal: Option<&str>,
    project_id: &str,
    model: Option<&str>,
    codex_policy: Option<crate::backend::broker::CodexThreadPolicy>,
) -> Result<(), String> {
    // Clean relaunch: kill any prior session for this id first.
    kill_cloud_duplex(app, sessions, agent_id);

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // stderr → a milestone line via a side reader (so a launch failure is visible, not silent).
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {program}: {e}"))?;

    let child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "child stdin unavailable".to_string())?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| "child stdout unavailable".to_string())?;
    let child_stderr = child.stderr.take();
    let stdin = Arc::new(Mutex::new(child_stdin));
    let child = Arc::new(Mutex::new(Some(child)));

    // Codex sessions get a JSON-RPC correlator shared between the handshake driver, the reader
    // dispatcher, and the steering path. Claude sessions keep `None` (NDJSON, no correlation).
    let codex: Option<Arc<CodexClient>> = match provider {
        Provider::Codex => Some(Arc::new(CodexClient::new())),
        Provider::Claude => None,
    };

    match provider {
        // Claude: send the opening goal as the first user turn inline (byte-identical to before).
        Provider::Claude => {
            if let Some(goal) = initial_goal.filter(|g| !g.trim().is_empty()) {
                if let Ok(mut w) = stdin.lock() {
                    let _ = w.write_all(encode_user_turn(provider, goal).as_bytes());
                    let _ = w.flush();
                }
            }
        }
        // Codex: the app-server requires an `initialize` → `thread/start` handshake BEFORE any
        // turn. That handshake blocks on responses, so it runs on its own driver thread (never on
        // this spawn path, and never on the reader thread). It sends the opening goal as the first
        // `turn/start` only AFTER the thread id is known.
        Provider::Codex => {
            // Build all owned clones BEFORE spawning so the driver borrows nothing transient.
            let codex_driver = codex.clone().expect("codex client is Some for Codex provider");
            let stdin_driver = stdin.clone();
            let activity_file_driver = activity_file.clone();
            let cwd_string = cwd.to_string_lossy().to_string();
            let model_owned: Option<String> = model.map(str::to_string);
            // The policy is required to open a Codex thread; default to a safe Ask/root-only policy
            // if the caller passed None (should not happen in production, but never panic here).
            let policy_owned = codex_policy.unwrap_or_else(|| {
                crate::backend::broker::resolve_codex_thread_policy(
                    crate::backend::broker::SandboxMode::Ask,
                    &cwd_string,
                    &[],
                    false,
                )
            });
            let initial_goal_owned: Option<String> = initial_goal
                .filter(|g| !g.trim().is_empty())
                .map(str::to_string);

            let _ = std::thread::Builder::new()
                .name(format!("cloud-duplex-codex-handshake-{agent_id}"))
                .spawn(move || {
                    codex_handshake_driver(
                        codex_driver,
                        stdin_driver,
                        activity_file_driver,
                        cwd_string,
                        model_owned,
                        policy_owned,
                        initial_goal_owned,
                    );
                });
        }
    }

    let exited = Arc::new(AtomicBool::new(false));

    // stderr → a single milestone line so a CLI launch failure (bad key, model not found, denied
    // by --permission-mode) is visible in the Stage instead of a silent blank panel.
    if let Some(stderr) = child_stderr {
        let app = app.clone();
        let agent_id = agent_id.to_string();
        let activity_file = activity_file.clone();
        let _ = std::thread::Builder::new()
            .name(format!("cloud-duplex-err-{agent_id}"))
            .spawn(move || {
                let _ = &app;
                let _ = &agent_id;
                let mut surfaced = false;
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let t = line.trim();
                    if t.is_empty() || surfaced {
                        continue;
                    }
                    let label: String = format!("⚠ CLI: {}", t.chars().take(160).collect::<String>());
                    let bridge =
                        serde_json::json!({"kind":"milestone","text":label,"node":"terra"}).to_string();
                    append_bridge_line(&activity_file, &bridge);
                    surfaced = true; // one error line is enough; keep draining to EOF
                }
            });
    }

    let reader = {
        let app = app.clone();
        let agent_id = agent_id.to_string();
        let exited = exited.clone();
        let child = child.clone();
        // A separate handle for the error path (the one above is moved into the thread closure).
        let child_err = child.clone();
        let activity_file = activity_file.clone();
        // Codex-only: the correlator (for responses) + a stdin handle (to write approval results).
        // `None` for Claude — the reader then takes the byte-identical normalizer-only path.
        let reader_codex = codex.clone();
        let reader_stdin = stdin.clone();
        let reader_project_id = project_id.to_string();
        let mut normalizer = match provider {
            Provider::Claude => Normalizer::Claude(ClaudeNormalizer::new(0)),
            Provider::Codex => Normalizer::Codex(CodexNormalizer::new(0)),
        };
        std::thread::Builder::new()
            .name(format!("cloud-duplex-{agent_id}"))
            .spawn(move || {
                // Keep ONE append handle open for the session (avoids an open+close syscall per
                // token); fall back to a per-line open if it can't be opened up front.
                let mut sink = if activity_file.as_os_str().is_empty() {
                    None
                } else {
                    OpenOptions::new().create(true).append(true).open(&activity_file).ok()
                };
                for line in BufReader::new(child_stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    // Codex JSON-RPC dispatch: peek the line; route responses to the correlator and
                    // approval server-requests to the bridge. Everything else (and any non-JSON
                    // line) falls through to the normalizer. The reader must NEVER block on a user
                    // decision, so approvals are handed to a short-lived waiter thread.
                    if let Some(codex) = reader_codex.as_ref() {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                            if is_response(&v) {
                                if let Some(id) = msg_id(&v) {
                                    codex.complete_response(id, v);
                                }
                                continue;
                            }
                            if is_server_request(&v) {
                                let is_approval = msg_method(&v)
                                    .map(|m| m.ends_with("requestApproval"))
                                    .unwrap_or(false);
                                if is_approval {
                                    handle_codex_approval(
                                        &app,
                                        codex,
                                        &agent_id,
                                        &reader_project_id,
                                        &reader_stdin,
                                        &activity_file,
                                        &v,
                                    );
                                } else if let Some(id_val) =
                                    v.get("id").filter(|i| !i.is_null())
                                {
                                    // EVERY server-request needs a reply or the Codex turn hangs
                                    // until its internal timeout. We only implement approvals;
                                    // answer anything else with method-not-found (JSON-RPC -32601).
                                    // Echo the raw id (string or number) so a string-id request is
                                    // matched correctly by the app-server.
                                    write_codex_line(
                                        &reader_stdin,
                                        &encode_error_response(
                                            id_val,
                                            -32601,
                                            "method not supported by the devboule host",
                                        ),
                                    );
                                }
                                continue;
                            }
                            // A parseable notification → normalizer.
                        }
                        // Non-JSON line → fall through to the normalizer (existing path).
                    }
                    for bridge in normalizer.feed(&line) {
                        match sink.as_mut() {
                            Some(f) => {
                                let mut buf = bridge;
                                buf.push('\n');
                                let _ = f.write_all(buf.as_bytes());
                            }
                            None => append_bridge_line(&activity_file, &bridge),
                        }
                    }
                }
                // EOF: the child closed stdout (exited / was killed). Reap it (no-op if kill
                // already took it), remove ourselves from the registry, and mark the UI closed.
                // NOTE: any in-flight Codex approval-waiter threads are NOT cancelled here; they
                // linger up to CODEX_APPROVAL_TIMEOUT (120s) before auto-declining, then write to
                // the dead child's stdin (the write fails harmlessly). A future slice may track the
                // session's pending approval_ids and `cancel()` them on kill to drop the waiters
                // eagerly.
                exited.store(true, Ordering::SeqCst);
                reap_child(&child, false);
                if let Some(sessions) = app.try_state::<CloudDuplexSessions>() {
                    if let Ok(mut map) = sessions.inner.lock() {
                        map.remove(&agent_id);
                    }
                }
                crate::backend::agents::mark_agent_session_closed_public(&app, &agent_id);
            })
            .map_err(|e| {
                // Reader thread couldn't start — reap the child so it isn't left running with the
                // API key in its env, then fail the launch.
                reap_child(&child_err, true);
                format!("failed to start reader thread: {e}")
            })?
    };

    let session = DuplexSession {
        child,
        stdin,
        reader: Some(reader),
        exited,
        provider,
        project_id: project_id.to_string(),
        codex,
    };
    match sessions.inner.lock() {
        Ok(mut map) => {
            map.insert(agent_id.to_string(), session);
            Ok(())
        }
        Err(_) => {
            // Can't register (poisoned lock) — kill the child so it can't keep running
            // uncontrollable with the API key, and report the failure (never a false Ok).
            reap_child(&session.child, true);
            Err("could not register the cloud orchestrator session (state lock poisoned)".into())
        }
    }
}

/// Write a steer/user message to a live duplex child's stdin. Returns Err if there is no live
/// session for `agent_id` or the write fails. The map lock is released BEFORE the (blocking) pipe
/// write so it can never deadlock against `kill_cloud_duplex`.
pub fn cloud_duplex_send(
    sessions: &CloudDuplexSessions,
    agent_id: &str,
    message: &str,
) -> Result<(), String> {
    // Clone the stdin handle + provider + Codex correlator under the map lock, then DROP the lock
    // BEFORE the (blocking) pipe write so it can never deadlock against `kill_cloud_duplex`.
    let (stdin, provider, codex) = {
        let map = sessions.inner.lock().map_err(|_| "state lock poisoned".to_string())?;
        let session = map
            .get(agent_id)
            .ok_or_else(|| "no live cloud orchestrator for this agent".to_string())?;
        if session.exited.load(Ordering::SeqCst) {
            return Err("the cloud orchestrator has exited".to_string());
        }
        (Arc::clone(&session.stdin), session.provider, session.codex.clone())
    };
    let encoded = match provider {
        Provider::Claude => encode_user_turn(provider, message),
        Provider::Codex => {
            // Codex turns are `turn/start` into the open thread. If the handshake has not yet
            // produced a thread id, the thread is not ready — fail clearly instead of writing a
            // malformed turn the app-server would reject.
            let codex = codex.ok_or_else(|| "the Codex session has no client".to_string())?;
            let tid = codex
                .thread_id()
                .ok_or_else(|| "the Codex thread is not ready yet".to_string())?;
            let id = codex.alloc_id();
            encode_turn_start(id, &tid, message)
        }
    };
    let mut w = stdin.lock().map_err(|_| "stdin lock poisoned".to_string())?;
    w.write_all(encoded.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    w.flush().map_err(|e| format!("flush failed: {e}"))?;
    Ok(())
}

/// Kill + reap a duplex child (idempotent; no-op if absent). The child handle is shared with the
/// reader thread, so whichever runs first reaps it exactly once.
pub fn kill_cloud_duplex(app: &tauri::AppHandle, sessions: &CloudDuplexSessions, agent_id: &str) {
    let session = sessions.inner.lock().ok().and_then(|mut m| m.remove(agent_id));
    if let Some(mut session) = session {
        session.exited.store(true, Ordering::SeqCst);
        // Kill via the shared handle (the OS then closes the child's stdout → the reader hits EOF
        // and exits). No map lock is held here, so the reader can remove itself without blocking.
        reap_child(&session.child, true);
        if let Some(reader) = session.reader.take() {
            let _ = reader.join();
        }
    }
    crate::backend::agents::mark_agent_session_closed_public(app, agent_id);
}

/// IPC: send a planner-chat message to a live cloud DUPLEX orchestrator's stdin (the cloud
/// counterpart of `orchestrator_steer`, which writes to the local orchestrator's steer file).
#[tauri::command]
pub fn project_cloud_orchestrator_send(
    app: tauri::AppHandle,
    agent_id: String,
    message: String,
) -> Result<(), String> {
    let sessions = app.state::<CloudDuplexSessions>();
    cloud_duplex_send(&sessions, &agent_id, &message)
}

/// Ask a live Codex duplex session to compact its thread context (`thread/compact/start`).
/// Only valid for Codex with a completed handshake (a thread id). The map lock is dropped
/// before the (blocking) pipe write, mirroring `cloud_duplex_send`.
pub fn cloud_duplex_compact(sessions: &CloudDuplexSessions, agent_id: &str) -> Result<(), String> {
    let (stdin, codex) = {
        let map = sessions.inner.lock().map_err(|_| "state lock poisoned".to_string())?;
        let session = map
            .get(agent_id)
            .ok_or_else(|| "no live cloud orchestrator for this agent".to_string())?;
        if session.exited.load(Ordering::SeqCst) {
            return Err("the cloud orchestrator has exited".to_string());
        }
        (Arc::clone(&session.stdin), session.codex.clone())
    };
    let codex = codex.ok_or_else(|| "compact is only supported for Codex sessions".to_string())?;
    let tid = codex
        .thread_id()
        .ok_or_else(|| "the Codex thread is not ready yet".to_string())?;
    let id = codex.alloc_id();
    let encoded = encode_compact(id, &tid);
    let mut w = stdin.lock().map_err(|_| "stdin lock poisoned".to_string())?;
    w.write_all(encoded.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    w.flush().map_err(|e| format!("flush failed: {e}"))?;
    Ok(())
}

/// IPC: compact a live Codex duplex session's context (frontend Compact button for Codex).
#[tauri::command]
pub fn project_cloud_compact(app: tauri::AppHandle, agent_id: String) -> Result<(), String> {
    let sessions = app.state::<CloudDuplexSessions>();
    cloud_duplex_compact(&sessions, &agent_id)
}

/// True if a live (non-exited) duplex session exists for `agent_id`.
pub fn has_live_duplex(sessions: &CloudDuplexSessions, agent_id: &str) -> bool {
    sessions
        .inner
        .lock()
        .ok()
        .and_then(|m| m.get(agent_id).map(|s| !s.exited.load(Ordering::SeqCst)))
        .unwrap_or(false)
}

// ──────────────────────────────────────────────────────────────────────────────
// Codex handshake driver + approval bridge (Slice 5a)
// ──────────────────────────────────────────────────────────────────────────────

/// Append a milestone bridge line to the activity file (operator-visible Stage note).
fn append_codex_milestone(activity_file: &std::path::Path, text: &str) {
    let json = serde_json::json!({ "kind": "milestone", "text": text, "node": "terra" }).to_string();
    append_bridge_line(activity_file, &json);
}

/// Write one encoded JSON-RPC line to the shared stdin under its lock. Best-effort: a dead child
/// makes the write fail harmlessly. Returns `false` if the stdin lock could not be acquired.
fn write_codex_line(stdin: &Arc<Mutex<ChildStdin>>, line: &str) -> bool {
    if let Ok(mut w) = stdin.lock() {
        let _ = w.write_all(line.as_bytes());
        let _ = w.flush();
        true
    } else {
        false
    }
}

/// Drive the Codex app-server handshake on its OWN thread: `initialize` → `thread/start` →
/// (optional) opening `turn/start`. Each request blocks on its response with a timeout so a hung
/// app-server can never park this thread forever. On any timeout / missing thread id it appends a
/// milestone and returns (the session stays alive but cannot take turns until relaunched).
#[allow(clippy::too_many_arguments)]
fn codex_handshake_driver(
    codex: Arc<CodexClient>,
    stdin: Arc<Mutex<ChildStdin>>,
    activity_file: PathBuf,
    cwd: String,
    model: Option<String>,
    policy: crate::backend::broker::CodexThreadPolicy,
    initial_goal: Option<String>,
) {
    // 1. initialize
    let id1 = codex.alloc_id();
    let rx1 = codex.register_response(id1);
    if !write_codex_line(&stdin, &encode_initialize(id1)) {
        // stdin lock poisoned — fail fast instead of waiting the full 30s for a reply that
        // can never come (nothing was written).
        append_codex_milestone(&activity_file, "⚠ Codex handshake failed: could not write initialize");
        return;
    }
    if rx1.recv_timeout(CODEX_HANDSHAKE_TIMEOUT).is_err() {
        append_codex_milestone(&activity_file, "⚠ Codex handshake timed out (initialize)");
        return;
    }

    // 2. thread/start → learn the thread id (tolerant of threadId / thread_id / id shapes).
    let id2 = codex.alloc_id();
    let rx2 = codex.register_response(id2);
    if !write_codex_line(
        &stdin,
        &encode_thread_start(id2, &cwd, model.as_deref(), &policy),
    ) {
        append_codex_milestone(&activity_file, "⚠ Codex handshake failed: could not write thread/start");
        return;
    }
    let resp = match rx2.recv_timeout(CODEX_HANDSHAKE_TIMEOUT) {
        Ok(resp) => resp,
        Err(_) => {
            append_codex_milestone(&activity_file, "⚠ Codex handshake timed out (thread/start)");
            return;
        }
    };
    // v2 shape is `result.thread.id`; keep the flat fallbacks for protocol drift. (All
    // unverified — confirm against a live app-server.)
    let tid = resp["result"]["thread"]["id"]
        .as_str()
        .or_else(|| resp["result"]["threadId"].as_str())
        .or_else(|| resp["result"]["thread_id"].as_str())
        .or_else(|| resp["result"]["id"].as_str());
    let Some(tid) = tid else {
        append_codex_milestone(
            &activity_file,
            "⚠ Codex handshake failed: thread/start returned no thread id",
        );
        return;
    };
    let tid = tid.to_string();
    codex.set_thread_id(tid.clone());

    // 3. opening goal as the first turn (fire-and-forget — no need to await the turn response;
    //    its streamed notifications drive the Stage like any other turn).
    if let Some(goal) = initial_goal.filter(|g| !g.trim().is_empty()) {
        let id3 = codex.alloc_id();
        write_codex_line(&stdin, &encode_turn_start(id3, &tid, &goal));
    }
}

/// Extracted, machine-readable fields of a Codex approval `params` object.
struct ApprovalParams {
    kind: crate::backend::broker::ConsentKind,
    detail: String,
    path: Option<String>,
}

/// PURE extractor: map an approval server-request's `method` + `params` to a consent kind, a
/// human-readable `detail`, and a machine-readable `path`. Tolerant of the documented-but-unstable
/// app-server shapes (validated live later). No I/O — unit-testable.
fn extract_approval_params(method: &str, params: &serde_json::Value) -> ApprovalParams {
    if method.contains("fileChange") {
        // Patch (file change) approval.
        let detail = params["reason"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("File change requested")
            .chars()
            .take(200)
            .collect::<String>();
        let path = params["grantRoot"]
            .as_str()
            .or_else(|| params["path"].as_str())
            .map(str::to_string);
        ApprovalParams {
            kind: crate::backend::broker::ConsentKind::Patch,
            detail,
            path,
        }
    } else {
        // Exec (command execution / permissions / anything else) approval.
        // `command` may be a JSON array of strings (join with ' ') OR a plain string; also try `cmd`.
        let command = command_to_string(&params["command"])
            .or_else(|| command_to_string(&params["cmd"]))
            .unwrap_or_default();
        let detail: String = command.chars().take(200).collect();
        let path = params["cwd"].as_str().map(str::to_string);
        ApprovalParams {
            kind: crate::backend::broker::ConsentKind::Exec,
            detail,
            path,
        }
    }
}

/// Render a `command` JSON value to a single string: a string as-is, or an array of string parts
/// joined with spaces. Returns `None` for any other shape (null / object / array-of-non-strings).
fn command_to_string(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = v.as_array() {
        let parts: Vec<&str> = arr.iter().filter_map(|p| p.as_str()).collect();
        if parts.is_empty() {
            return None;
        }
        return Some(parts.join(" "));
    }
    None
}

/// Handle a Codex approval server-request WITHOUT blocking the reader thread. Registers a
/// `CloudConsentState` waiter, emits the `ConsentRequest` to the frontend, and spawns a SHORT-LIVED
/// waiter thread that blocks on the user's decision (with a hard timeout → auto-decline) and writes
/// the JSON-RPC result back on the shared stdin. The reader returns immediately and keeps draining.
fn handle_codex_approval(
    app: &tauri::AppHandle,
    codex: &Arc<CodexClient>,
    agent_id: &str,
    project_id: &str,
    stdin: &Arc<Mutex<ChildStdin>>,
    activity_file: &std::path::Path,
    v: &serde_json::Value,
) {
    // Capture the RAW JSON-RPC id (string OR number per JSON-RPC 2.0). We must echo it verbatim in
    // the response; coercing it to a `u64` here would drop string ids and emit a non-matching id.
    let Some(id_val) = v.get("id").filter(|i| !i.is_null()) else { return };
    let id_owned = id_val.clone(); // serde_json::Value: Clone — moved into the waiter closure.
    let id_str = jsonrpc_id_str(id_val);
    let method = msg_method(v).unwrap_or("");
    let params = &v["params"];
    let extracted = extract_approval_params(method, params);
    // Per-session nonce makes the approval_id unique across a relaunch of the same agent_id: a
    // fresh CodexClient restarts request ids at 1, so without the nonce an old lingering waiter's
    // 120s-timeout `cancel()` could remove the NEW session's identically-named waiter.
    let approval_id = format!("{agent_id}:{nonce}:{id_str}", nonce = codex.session_nonce());

    // Backpressure (this runs on the SINGLE reader thread, so the count check + increment is
    // race-free): if too many approvals are already awaiting a human, decline immediately
    // rather than spawning an unbounded number of 120s waiter threads (a misbehaving / adversarial
    // Codex could otherwise exhaust the OS thread limit).
    if codex.in_flight_approvals() >= MAX_INFLIGHT_APPROVALS {
        append_codex_milestone(
            &activity_file.to_path_buf(),
            "⚠ Too many pending Codex approvals — declined",
        );
        write_codex_line(
            stdin,
            &encode_approval_result(&id_owned, CodexApprovalReply::Decline.as_wire()),
        );
        return;
    }

    // No broker → we cannot prompt → decline immediately so the turn does not hang.
    let Some(cloud_consent) =
        app.try_state::<crate::backend::broker::CloudConsentState>()
    else {
        write_codex_line(
            stdin,
            &encode_approval_result(&id_owned, CodexApprovalReply::Decline.as_wire()),
        );
        return;
    };

    // Register the waiter BEFORE emitting so a near-instant `respond_cloud_consent` cannot race
    // ahead of the registration and be lost.
    let rx = cloud_consent.register(&approval_id);

    let req = crate::backend::broker::ConsentRequest {
        kind: extracted.kind,
        project_id: project_id.to_string(),
        agent_id: agent_id.to_string(),
        detail: extracted.detail,
        path: extracted.path,
        approval_id: Some(approval_id.clone()),
    };
    let _ = app.emit("sandbox://consent-request", req);
    // Surface a "waiting" note in the activity Stage (the dispatcher no longer feeds the approval
    // server-request to the normalizer, which previously emitted this).
    append_codex_milestone(activity_file, "⏳ Codex is waiting for your approval");

    // Spawn the waiter so the reader keeps draining stdout (an approval must NEVER block reads).
    codex.inc_approval();
    let app_waiter = app.clone();
    let codex_waiter = codex.clone();
    let stdin_waiter = stdin.clone();
    let activity_file = activity_file.to_path_buf();
    // Clones for the spawn-failure path: the closure below MOVES `id_owned` and `approval_id`, so
    // they are unavailable afterwards regardless of whether the thread actually started.
    let id_fallback = id_owned.clone();
    let approval_id_fallback = approval_id.clone();
    let spawn_res = std::thread::Builder::new()
        .name(format!("cloud-duplex-codex-approval-{agent_id}-{id_str}"))
        .spawn(move || {
            let reply = match rx.recv_timeout(CODEX_APPROVAL_TIMEOUT) {
                Ok(decision) => CodexApprovalReply::from_decision(&decision),
                Err(_) => {
                    // Timed out (or the sender was dropped on session kill). Clean up the pending
                    // entry and fail-closed: decline so the Codex turn is never left hanging.
                    if let Some(cc) = app_waiter
                        .try_state::<crate::backend::broker::CloudConsentState>()
                    {
                        cc.cancel(&approval_id);
                    }
                    append_codex_milestone(&activity_file, "⚠ Codex approval timed out — declined");
                    CodexApprovalReply::Decline
                }
            };
            write_codex_line(
                &stdin_waiter,
                &encode_approval_result(&id_owned, reply.as_wire()),
            );
            codex_waiter.dec_approval();
        });

    // If the waiter thread failed to spawn, the closure (which owns the `dec_approval` + the reply
    // write) never runs. Undo the increment ourselves so the counter cannot leak permanently — a
    // leak would drive `in_flight_approvals` past MAX_INFLIGHT_APPROVALS and auto-decline EVERY
    // future approval (a self-inflicted DoS). Also write an inline decline so this turn does not
    // hang, and cancel the just-registered waiter so no stale entry lingers.
    if spawn_res.is_err() {
        codex.dec_approval();
        cloud_consent.cancel(&approval_id_fallback);
        write_codex_line(
            stdin,
            &encode_approval_result(&id_fallback, CodexApprovalReply::Decline.as_wire()),
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Codex app-server JSON-RPC client (Slice 5a)
//
// Codex speaks JSON-RPC 2.0 over newline-delimited stdio. We drive the handshake
// (initialize → thread/start → turn/start), correlate our request ids to responses,
// and answer server-initiated approval requests. PURE encoders + classify helpers +
// a per-session `CodexClient` correlator below; the reader-thread dispatcher and the
// approval bridge live in `spawn_cloud_duplex`.
// ──────────────────────────────────────────────────────────────────────────────

/// `initialize` handshake request (id usually 1).
pub fn encode_initialize(id: u64) -> String {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "clientInfo": { "name": "devboule", "title": "devboule", "version": "0.1.0" },
            "capabilities": {}
        }
    });
    format!("{json}\n")
}

/// `thread/start` request: opens a thread with the resolved sandbox/approval policy.
///
/// Emits the documented v2 params shape: `approvalPolicy` (string), `sandbox` as the plain
/// string `"workspaceWrite"` (NOT a tagged object — that is the response shape), the writable
/// roots in a SEPARATE `runtimeWorkspaceRoots` array, and a best-effort `networkAccess`.
/// ⚠️ These exact field names/casing are from the documented-but-unstable protocol and MUST be
/// validated against a live `codex app-server` (the owner's eyes).
pub fn encode_thread_start(
    id: u64,
    cwd: &str,
    model: Option<&str>,
    policy: &crate::backend::broker::CodexThreadPolicy,
) -> String {
    let mut params = serde_json::json!({
        "cwd": cwd,
        "approvalPolicy": policy.approval_policy.as_wire(),
        "sandbox": "workspaceWrite",
        "runtimeWorkspaceRoots": policy.writable_roots,
        "networkAccess": policy.network_access,
    });
    if let Some(m) = model {
        params["model"] = serde_json::json!(m);
    }
    // Slice 5c: per-project agent controls (⚠️ field names unverified — confirm live).
    if let Some(effort) = policy.effort.as_deref().filter(|s| !s.trim().is_empty()) {
        params["modelReasoningEffort"] = serde_json::json!(effort);
    }
    if let Some(di) = policy
        .developer_instructions
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        params["developerInstructions"] = serde_json::json!(di);
    }
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "thread/start",
        "params": params
    });
    format!("{json}\n")
}

/// `turn/start` request: send a user turn into an open thread.
pub fn encode_turn_start(id: u64, thread_id: &str, text: &str) -> String {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "turn/start",
        "params": { "threadId": thread_id, "input": [{ "type": "text", "text": text }] }
    });
    format!("{json}\n")
}

/// JSON-RPC *response* to a server-initiated approval request. The `id` is echoed VERBATIM from
/// the request — JSON-RPC 2.0 requires the response id to equal the request id by type and value,
/// so a string request id must be answered with the same string (never coerced to a number).
pub fn encode_approval_result(id: &serde_json::Value, decision_wire: &str) -> String {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "decision": decision_wire }
    });
    format!("{json}\n")
}

/// JSON-RPC *error* response — sent for any server-request we don't implement, so a Codex
/// turn never hangs waiting on a reply we'd otherwise never send. The `id` is echoed VERBATIM
/// (string or number) for the same type-fidelity reason as [`encode_approval_result`].
pub fn encode_error_response(id: &serde_json::Value, code: i64, message: &str) -> String {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    format!("{json}\n")
}

/// `thread/compact/start` request: ask Codex to compact the thread context.
pub fn encode_compact(id: u64, thread_id: &str) -> String {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "thread/compact/start",
        "params": { "threadId": thread_id }
    });
    format!("{json}\n")
}

fn msg_id(v: &serde_json::Value) -> Option<u64> {
    v.get("id").and_then(|v| v.as_u64())
}

fn msg_method(v: &serde_json::Value) -> Option<&str> {
    v.get("method").and_then(|v| v.as_str())
}

/// Render a JSON-RPC id value to a stable string for use inside an opaque `approval_id`.
/// JSON-RPC 2.0 ids may be a number OR a string; a number renders as its integer/string
/// form, a string renders as itself, and any other shape falls back to its JSON text. PURE.
fn jsonrpc_id_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// A reply to one of OUR requests: has `id` + (`result`|`error`) and no `method`. OUR outbound
/// ids are always `u64` (from [`CodexClient::alloc_id`]), so response correlation stays `u64`-based.
fn is_response(v: &serde_json::Value) -> bool {
    msg_id(v).is_some()
        && (v.get("result").is_some() || v.get("error").is_some())
        && msg_method(v).is_none()
}

/// A server→client request (needs a response): has a non-null `id` (ANY JSON-RPC type — string or
/// number per JSON-RPC 2.0) and a `method`. We must NOT require a `u64` id here: a server-initiated
/// `requestApproval` with a string id would otherwise be misclassified as a notification, dropped,
/// and leave the Codex turn hanging.
fn is_server_request(v: &serde_json::Value) -> bool {
    v.get("id").map_or(false, |i| !i.is_null()) && msg_method(v).is_some()
}

/// A one-way notification: has `method` and no `id`. The dispatcher routes notifications by
/// elimination (not a response, not a server-request → normalizer), so this predicate is only
/// used in tests today; kept for completeness and symmetry with the other classifiers.
#[allow(dead_code)]
fn is_notification(v: &serde_json::Value) -> bool {
    // id-type-agnostic (mirror is_server_request): a notification has a method and NO id of any
    // type (absent or null). Using `msg_id` (u64) here would misclassify a string-id server
    // request as a notification — adversarial-verify C1.
    msg_method(v).is_some() && v.get("id").map_or(true, |i| i.is_null())
}

/// The session-scoped decision we send back as the `result` of an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexApprovalReply {
    Accept,
    AcceptForSession,
    Decline,
}

impl CodexApprovalReply {
    pub fn as_wire(self) -> &'static str {
        match self {
            CodexApprovalReply::Accept => "accept",
            CodexApprovalReply::AcceptForSession => "acceptForSession",
            CodexApprovalReply::Decline => "decline",
        }
    }

    pub fn from_decision(d: &crate::backend::broker::ConsentDecision) -> Self {
        match d {
            crate::backend::broker::ConsentDecision::AllowRemember => {
                CodexApprovalReply::AcceptForSession
            }
            crate::backend::broker::ConsentDecision::AllowOnce => CodexApprovalReply::Accept,
            crate::backend::broker::ConsentDecision::Deny => CodexApprovalReply::Decline,
        }
    }
}

/// Per-session correlator for OUR outbound JSON-RPC requests (initialize / thread/start /
/// turn/start / compact). Shared (`Arc`) between the handshake driver thread, the reader
/// dispatcher, and the steering path. Uses `std::sync::mpsc` to match this file's all-`std::thread`
/// design. The mutex is only held for the brief map mutation — never across a blocking `recv`.
pub struct CodexClient {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>,
    thread_id: Mutex<Option<String>>,
    /// Count of approval-waiter threads currently blocked on a human decision. Bounded by
    /// `MAX_INFLIGHT_APPROVALS` so a misbehaving Codex cannot exhaust the OS thread limit.
    in_flight_approvals: AtomicUsize,
    /// Per-session nonce (from `CODEX_SESSION_SEQ`) that disambiguates `approval_id`s across a
    /// relaunch of the same `agent_id`. Distinct per `CodexClient`; never changes after `new()`.
    session_nonce: u64,
}

impl CodexClient {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            thread_id: Mutex::new(None),
            in_flight_approvals: AtomicUsize::new(0),
            session_nonce: CODEX_SESSION_SEQ.fetch_add(1, Ordering::SeqCst),
        }
    }

    /// Allocate a fresh, monotonically increasing request id.
    pub fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// The unique per-session nonce used to namespace this session's `approval_id`s.
    pub fn session_nonce(&self) -> u64 {
        self.session_nonce
    }

    /// Number of approval-waiter threads currently awaiting a human decision.
    pub fn in_flight_approvals(&self) -> usize {
        self.in_flight_approvals.load(Ordering::SeqCst)
    }

    /// Mark one approval-waiter thread as started (called on the single reader thread).
    pub fn inc_approval(&self) {
        self.in_flight_approvals.fetch_add(1, Ordering::SeqCst);
    }

    /// Mark one approval-waiter thread as finished (called as the waiter exits).
    pub fn dec_approval(&self) {
        self.in_flight_approvals.fetch_sub(1, Ordering::SeqCst);
    }

    /// Register a waiter for the response to request `id`; returns the `Receiver` to block on.
    pub fn register_response(&self, id: u64) -> mpsc::Receiver<serde_json::Value> {
        let (tx, rx) = mpsc::channel();
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.insert(id, tx);
        rx
    }

    /// Deliver a response value to its waiter. Returns true if a waiter existed.
    pub fn complete_response(&self, id: u64, value: serde_json::Value) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = pending.remove(&id) {
            let _ = tx.send(value);
            true
        } else {
            false
        }
    }

    pub fn set_thread_id(&self, id: String) {
        let mut tid = self.thread_id.lock().unwrap_or_else(|e| e.into_inner());
        *tid = Some(id);
    }

    pub fn thread_id(&self) -> Option<String> {
        self.thread_id.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Default for CodexClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn claude_user_turn_is_a_stream_json_user_message() {
        let line = encode_user_turn(Provider::Claude, "ciao");
        assert!(line.ends_with('\n'));
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"][0]["type"], "text");
        assert_eq!(v["message"]["content"][0]["text"], "ciao");
    }

    #[test]
    fn codex_user_turn_is_a_jsonrpc_send_message() {
        let line = encode_user_turn(Provider::Codex, "hola");
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["method"], "sendUserMessage");
        assert_eq!(v["params"]["items"][0]["text"], "hola");
    }

    #[test]
    fn provider_from_client_maps_known_clients() {
        assert_eq!(Provider::from_client("claude"), Some(Provider::Claude));
        assert_eq!(Provider::from_client("codex"), Some(Provider::Codex));
        assert_eq!(Provider::from_client("orchestrator"), None);
    }

    #[test]
    fn user_turn_escapes_newlines_and_quotes_to_one_line() {
        let line = encode_user_turn(Provider::Claude, "a\"b\nc");
        assert_eq!(line.matches('\n').count(), 1, "exactly the trailing newline");
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["message"]["content"][0]["text"], "a\"b\nc");
    }

    // ── Codex app-server JSON-RPC helpers (Slice 5a) ──────────────────────────

    #[test]
    fn encode_initialize_is_jsonrpc() {
        let s = encode_initialize(1);
        assert!(s.ends_with('\n'));
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["id"], 1);
        assert_eq!(v["jsonrpc"], "2.0");
    }

    #[test]
    fn encode_turn_start_has_thread_id_and_text() {
        let s = encode_turn_start(3, "thr1", "hi");
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["method"], "turn/start");
        assert_eq!(v["params"]["threadId"], "thr1");
        assert_eq!(v["params"]["input"][0]["text"], "hi");
    }

    #[test]
    fn encode_thread_start_embeds_policy() {
        use crate::backend::broker::{CodexApprovalPolicy, CodexThreadPolicy};
        let policy = CodexThreadPolicy {
            approval_policy: CodexApprovalPolicy::OnRequest,
            writable_roots: vec!["/r".into(), "/extra".into()],
            network_access: false,
            effort: Some("high".into()),
            developer_instructions: None,
        };
        let s = encode_thread_start(2, "/r", None, &policy);
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        // approvalPolicy is a plain string; sandbox is the plain string "workspaceWrite";
        // writable roots live in a SEPARATE runtimeWorkspaceRoots array (post-review shape).
        assert_eq!(v["params"]["approvalPolicy"], "onRequest");
        assert_eq!(v["params"]["sandbox"], "workspaceWrite");
        assert_eq!(v["params"]["runtimeWorkspaceRoots"][0], "/r");
        assert_eq!(v["params"]["runtimeWorkspaceRoots"][1], "/extra");
        assert_eq!(v["params"]["networkAccess"], false);
        assert_eq!(v["params"]["cwd"], "/r");
        // Slice 5c: effort is emitted when set; developer_instructions omitted when None.
        assert_eq!(v["params"]["modelReasoningEffort"], "high");
        assert!(v["params"].get("developerInstructions").is_none());
    }

    #[test]
    fn encode_approval_result_shape() {
        let s = encode_approval_result(&serde_json::json!(7), "accept");
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["result"]["decision"], "accept");
    }

    #[test]
    fn encode_approval_result_echoes_string_id_verbatim() {
        // JSON-RPC ids may be strings; the response id must equal the request id by type+value.
        let s = encode_approval_result(&serde_json::json!("req-abc"), "decline");
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["id"], "req-abc");
        assert!(v["id"].is_string(), "string id must NOT be coerced to a number");
        assert_eq!(v["result"]["decision"], "decline");
    }

    #[test]
    fn encode_error_response_shape() {
        let s = encode_error_response(&serde_json::json!(9), -32601, "nope");
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["id"], 9);
        assert_eq!(v["error"]["code"], -32601);
        assert_eq!(v["error"]["message"], "nope");
    }

    #[test]
    fn encode_error_response_echoes_string_id_verbatim() {
        let s = encode_error_response(&serde_json::json!("srv-1"), -32601, "nope");
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["id"], "srv-1");
        assert!(v["id"].is_string());
    }

    #[test]
    fn jsonrpc_id_str_renders_number_and_string_forms() {
        assert_eq!(jsonrpc_id_str(&serde_json::json!(42)), "42");
        assert_eq!(jsonrpc_id_str(&serde_json::json!("req-abc")), "req-abc");
        // A non-number/non-string id falls back to its JSON text (defensive; shouldn't occur).
        assert_eq!(jsonrpc_id_str(&serde_json::json!([1, 2])), "[1,2]");
    }

    #[test]
    fn codex_client_in_flight_counter() {
        let c = CodexClient::new();
        assert_eq!(c.in_flight_approvals(), 0);
        c.inc_approval();
        c.inc_approval();
        assert_eq!(c.in_flight_approvals(), 2);
        c.dec_approval();
        assert_eq!(c.in_flight_approvals(), 1);
    }

    #[test]
    fn encode_compact_targets_thread() {
        let s = encode_compact(4, "thrX");
        let v: Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(v["method"], "thread/compact/start");
        assert_eq!(v["params"]["threadId"], "thrX");
    }

    #[test]
    fn classify_distinguishes_message_classes() {
        let response = serde_json::json!({"id": 1, "result": {}});
        let server_req = serde_json::json!({"id": 2, "method": "item/x/requestApproval"});
        let notification = serde_json::json!({"method": "item/y"});
        assert!(is_response(&response) && !is_response(&server_req) && !is_response(&notification));
        assert!(
            !is_server_request(&response)
                && is_server_request(&server_req)
                && !is_server_request(&notification)
        );
        assert!(
            !is_notification(&response)
                && !is_notification(&server_req)
                && is_notification(&notification)
        );
    }

    #[test]
    fn is_server_request_accepts_string_id() {
        // JSON-RPC 2.0 allows string ids on server-initiated requests. A string-id approval must
        // be classified as a server-request (not a notification) or the Codex turn would hang.
        let str_id_req = serde_json::json!({"id": "req-abc", "method": "item/x/requestApproval"});
        assert!(is_server_request(&str_id_req));
        assert!(!is_response(&str_id_req));
        // A null id is NOT a server-request (it's effectively id-less).
        let null_id = serde_json::json!({"id": null, "method": "item/y"});
        assert!(!is_server_request(&null_id));
    }

    #[test]
    fn codex_session_nonce_is_unique_per_client() {
        // Two CodexClients (e.g. a relaunch of the same agent) must get distinct nonces so their
        // approval_id namespaces never collide even when per-client request ids both restart at 1.
        let a = CodexClient::new();
        let b = CodexClient::new();
        assert_ne!(a.session_nonce(), b.session_nonce());
        assert_eq!(a.alloc_id(), 1);
        assert_eq!(b.alloc_id(), 1, "per-client ids still restart at 1");
    }

    #[test]
    fn approval_reply_wire_and_from_decision() {
        use crate::backend::broker::ConsentDecision;
        assert_eq!(
            CodexApprovalReply::from_decision(&ConsentDecision::AllowOnce),
            CodexApprovalReply::Accept
        );
        assert_eq!(
            CodexApprovalReply::from_decision(&ConsentDecision::AllowRemember),
            CodexApprovalReply::AcceptForSession
        );
        assert_eq!(
            CodexApprovalReply::from_decision(&ConsentDecision::Deny),
            CodexApprovalReply::Decline
        );
        assert_eq!(CodexApprovalReply::Accept.as_wire(), "accept");
        assert_eq!(CodexApprovalReply::AcceptForSession.as_wire(), "acceptForSession");
        assert_eq!(CodexApprovalReply::Decline.as_wire(), "decline");
    }

    #[test]
    fn codex_client_alloc_id_increments() {
        let client = CodexClient::new();
        assert_eq!(client.alloc_id(), 1);
        assert_eq!(client.alloc_id(), 2);
    }

    // ── Approval-param extractor (Slice 5a) ───────────────────────────────────

    #[test]
    fn extract_exec_command_array_joins_with_spaces() {
        let params = serde_json::json!({ "command": ["cargo", "build"], "cwd": "/x" });
        let p = extract_approval_params("item/x/requestApproval", &params);
        assert_eq!(p.kind, crate::backend::broker::ConsentKind::Exec);
        assert_eq!(p.detail, "cargo build");
        assert_eq!(p.path.as_deref(), Some("/x"));
    }

    #[test]
    fn extract_exec_command_string_is_used_verbatim() {
        let params = serde_json::json!({ "command": "rm -rf node_modules", "cwd": "/y" });
        let p = extract_approval_params("commandExecution/requestApproval", &params);
        assert_eq!(p.kind, crate::backend::broker::ConsentKind::Exec);
        assert_eq!(p.detail, "rm -rf node_modules");
        assert_eq!(p.path.as_deref(), Some("/y"));
    }

    #[test]
    fn extract_exec_falls_back_to_cmd_field() {
        let params = serde_json::json!({ "cmd": ["ls", "-la"] });
        let p = extract_approval_params("anything/requestApproval", &params);
        assert_eq!(p.detail, "ls -la");
        assert!(p.path.is_none());
    }

    #[test]
    fn extract_file_change_is_patch_with_grant_root() {
        let params = serde_json::json!({ "grantRoot": "/g", "reason": "r" });
        let p = extract_approval_params("item/fileChange/requestApproval", &params);
        assert_eq!(p.kind, crate::backend::broker::ConsentKind::Patch);
        assert_eq!(p.detail, "r");
        assert_eq!(p.path.as_deref(), Some("/g"));
    }

    #[test]
    fn extract_file_change_falls_back_to_default_detail_and_path() {
        let params = serde_json::json!({ "path": "/p" });
        let p = extract_approval_params("fileChange/requestApproval", &params);
        assert_eq!(p.kind, crate::backend::broker::ConsentKind::Patch);
        assert_eq!(p.detail, "File change requested");
        assert_eq!(p.path.as_deref(), Some("/p"));
    }

    #[test]
    fn extract_exec_truncates_detail_to_200_chars() {
        let long = "a".repeat(500);
        let params = serde_json::json!({ "command": long });
        let p = extract_approval_params("x/requestApproval", &params);
        assert_eq!(p.detail.chars().count(), 200);
    }

    #[test]
    fn command_to_string_handles_shapes() {
        assert_eq!(command_to_string(&serde_json::json!("a b")).as_deref(), Some("a b"));
        assert_eq!(
            command_to_string(&serde_json::json!(["a", "b"])).as_deref(),
            Some("a b")
        );
        assert!(command_to_string(&serde_json::json!(null)).is_none());
        assert!(command_to_string(&serde_json::json!([])).is_none());
        assert!(command_to_string(&serde_json::json!({ "k": "v" })).is_none());
    }

    #[test]
    fn codex_client_register_then_complete() {
        let client = CodexClient::new();
        let rx = client.register_response(5);
        assert!(client.complete_response(5, serde_json::json!({"ok": true})));
        assert_eq!(rx.recv().unwrap(), serde_json::json!({"ok": true}));
        assert!(!client.complete_response(5, serde_json::json!({"ok": false})));
    }
}
