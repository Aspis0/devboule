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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tauri::Manager;

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

    // Send the opening goal as the first user turn (so the cloud orchestrator starts on-task).
    if let Some(goal) = initial_goal.filter(|g| !g.trim().is_empty()) {
        if let Ok(mut w) = stdin.lock() {
            let _ = w.write_all(encode_user_turn(provider, goal).as_bytes());
            let _ = w.flush();
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
    // Clone the stdin handle + provider under the map lock, then DROP the map lock.
    let (stdin, provider) = {
        let map = sessions.inner.lock().map_err(|_| "state lock poisoned".to_string())?;
        let session = map
            .get(agent_id)
            .ok_or_else(|| "no live cloud orchestrator for this agent".to_string())?;
        if session.exited.load(Ordering::SeqCst) {
            return Err("the cloud orchestrator has exited".to_string());
        }
        (Arc::clone(&session.stdin), session.provider)
    };
    let encoded = encode_user_turn(provider, message);
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

/// True if a live (non-exited) duplex session exists for `agent_id`.
pub fn has_live_duplex(sessions: &CloudDuplexSessions, agent_id: &str) -> bool {
    sessions
        .inner
        .lock()
        .ok()
        .and_then(|m| m.get(agent_id).map(|s| !s.exited.load(Ordering::SeqCst)))
        .unwrap_or(false)
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
}
