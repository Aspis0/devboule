//! App-hosted PTY subsystem for launched agents.
//!
//! Instead of (or in addition to) spawning a detached external console window,
//! an agent can be hosted INSIDE the app: its shell runs under a pseudo-terminal
//! (ConPTY on Windows, openpty on unix/macOS via `portable-pty`) and its output
//! is streamed to the frontend as Tauri events while a memory-only ring buffer
//! keeps the recent scrollback for late-joining viewers. The frontend writes
//! keystrokes back through `agent_pty_write` and resizes via `agent_pty_resize`.
//!
//! The studied reference for the PTY plumbing here is terax-ai (Apache-2.0),
//! which uses the same Tauri 2 + `portable-pty` stack. Patterns adopted from it
//! are attributed inline at their call sites:
//!   - native_pty_system()/openpty/PtySize creation and CommandBuilder spawn
//!   - a reader thread reading the master into a bounded buffer
//!   - a ChildKiller-based kill guard so a half-initialised session never leaks
//!     a live child
//!   - take_writer()/try_clone_reader() split for input vs. output
//!
//! SECURITY/PRIVACY: the ring buffer lives ONLY in memory, is never persisted or
//! logged, and the streamed event payload carries the raw terminal bytes (which
//! may include the launch token / prompt the agent printed) — so it must never be
//! written to disk. All command error strings are static; no prompt-file path or
//! env value is ever embedded in a returned error (detail goes to the log only).
//!
//! LOCKING ORDER (read before touching any command body):
//!   - The sessions-map lock (`AgentPtySessions::inner`) is NEVER held across a
//!     blocking PTY I/O call (`write_all`/`flush`/`resize`/`read`). Holding it
//!     across blocking I/O deadlocks against the reader-EOF teardown, which must
//!     take the SAME map lock to remove the dead session.
//!   - Per-session blocking endpoints are therefore behind their own Arc<Mutex<..>>
//!     (`writer`, `master`). Every command does: lock map → clone out the Arc(s)
//!     (or check existence) → UNLOCK the map → take the per-endpoint lock → do the
//!     blocking I/O. The map lock and an endpoint lock are never held at once.
//!   - Teardown removes the session struct from the map (dropping the map's Arc
//!     refs) and then drops the endpoints. A concurrent in-flight write that cloned
//!     the writer Arc keeps it alive briefly; that is fine — the underlying pipe is
//!     already closed, so the in-flight write simply errors and returns.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

use super::state::BackendState;

/// Maximum scrollback retained in memory per session (256 KiB). Oldest bytes are
/// dropped first so a long-running, chatty agent can never grow this without
/// bound. Memory-only; never persisted.
const RING_CAPACITY: usize = 256 * 1024;

/// Reader thread read-buffer size. Adopted from terax-ai (Apache-2.0), which uses
/// a 16 KiB master read buffer.
const READ_CHUNK: usize = 16 * 1024;

/// Initial PTY geometry. The frontend resizes to its real dimensions as soon as
/// the xterm viewer mounts (`agent_pty_resize`).
const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 32;

/// Append `data` to a capacity-bounded byte ring, dropping the OLDEST bytes first
/// so `ring.len()` never exceeds `cap`. Pure and side-effect-free so the cap/
/// drop-oldest policy is unit-testable without a real PTY.
///
/// Edge cases handled:
///   - `cap == 0`: the ring is cleared and nothing is retained.
///   - `data.len() >= cap`: only the last `cap` bytes of `data` are kept (the
///     existing ring is fully evicted, mirroring terax-ai's hard-reset-on-overflow
///     intent but as a sliding window rather than a notice).
pub fn push_capped(ring: &mut VecDeque<u8>, data: &[u8], cap: usize) {
    if cap == 0 {
        ring.clear();
        return;
    }
    if data.len() >= cap {
        // The incoming chunk alone fills (or overfills) the ring: keep only its
        // tail and discard everything older.
        ring.clear();
        ring.extend(&data[data.len() - cap..]);
        return;
    }
    // Evict oldest bytes until the new chunk fits within the cap.
    let overflow = (ring.len() + data.len()).saturating_sub(cap);
    if overflow > 0 {
        ring.drain(..overflow);
    }
    ring.extend(data);
}

/// One live app-hosted PTY session. The master end stays open for the writer and
/// for resize; the reader thread owns its own cloned reader. `exited` flips true
/// once the child is reaped or the reader hits EOF/error, so commands can tell a
/// dead session from a live one without blocking on `wait`.
pub struct PtySession {
    /// Master end, behind its own Arc<Mutex> so `agent_pty_resize` can resize it
    /// WITHOUT holding the sessions-map lock across the (blocking) resize syscall
    /// (see the LOCKING ORDER note at the top of this file). Teardown drops the
    /// map's Arc ref; the last ref drop closes the master.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// ChildKiller handle (clone of the child) used by `agent_pty_kill`; killing
    /// is idempotent. Kept separate from `child` so kill and wait can be ordered.
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// The spawned child, guarded so it can be `wait`ed exactly once on kill to
    /// reap it (no zombie). `Option` so we can `take` it on the first kill.
    child: Option<Box<dyn Child + Send + Sync>>,
    /// Input endpoint, behind its own Arc<Mutex> so `agent_pty_write` does the
    /// blocking `write_all`/`flush` WITHOUT holding the sessions-map lock.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader_handle: Option<JoinHandle<()>>,
    ring: Arc<Mutex<VecDeque<u8>>>,
    exited: Arc<AtomicBool>,
}

/// Tauri-managed map of agent_id -> live PTY session. Registered in lib.rs next to
/// `BackendState`/`PolisState` via `.manage(...)`.
#[derive(Default)]
pub struct AgentPtySessions {
    inner: Mutex<HashMap<String, PtySession>>,
}

impl AgentPtySessions {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Event payload streamed to `agent-terminal://<agent_id>`. Either a `data` chunk
/// (utf8-lossy of the raw master bytes) or the `exited: true` sentinel emitted
/// once when the session ends. Both fields are optional so the same event channel
/// carries both shapes; the frontend treats `exited == Some(true)` as terminal.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exited: Option<bool>,
}

/// Per-agent event channel name. Centralised so the producer (reader thread) and
/// the frontend listener never drift.
fn terminal_event_name(agent_id: &str) -> String {
    format!("agent-terminal://{agent_id}")
}

/// Spawn an app-hosted PTY for `agent_id` running `command` and start streaming
/// its output. The session is inserted into `sessions` keyed by `agent_id`; an
/// existing session for the same id is killed+replaced so a relaunch is clean.
///
/// `command` must already carry program + args + env + cwd (the launch path in
/// projects.rs builds it identically to the external path). On any failure the
/// just-spawned child is killed before returning so no orphan is left.
///
/// Pattern adapted from terax-ai (Apache-2.0): native_pty_system()/openpty with
/// an explicit PtySize, CommandBuilder spawn into the slave, take_writer()/
/// try_clone_reader() split, and a kill-on-early-failure guard.
pub fn spawn_agent_pty(
    app: &tauri::AppHandle,
    sessions: &AgentPtySessions,
    agent_id: &str,
    command: CommandBuilder,
) -> Result<(), String> {
    // FRONT-END CONTRACT (Phase 6): on Windows, ConPTY emits a Device Status
    // Report query (`ESC [ 6 n`, cursor position) at startup and STALLS its render
    // pipeline until the controlling terminal replies. xterm.js answers DSR out of
    // the box, so the viewer must pump our streamed bytes into an xterm instance
    // and wire xterm's `onData` back to `agent_pty_write`; a viewer that only
    // displays bytes without replying would see the child produce no output. (The
    // ignored integration test answers DSR manually to prove the pipe end-to-end.)
    //
    // Replace any prior session for this id (clean relaunch). Best-effort kill.
    kill_session_in_map(sessions, agent_id);

    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| {
            log_pty_detail("openpty failed", &e.to_string());
            "Could not open the agent terminal.".to_string()
        })?;

    let mut child = pair.slave.spawn_command(command).map_err(|e| {
        log_pty_detail("spawn_command failed", &e.to_string());
        "Could not start the agent in the app terminal.".to_string()
    })?;

    // From here on a failure must not leak the live child: capture a killer first.
    let killer = child.clone_killer();

    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(e) => {
            let mut killer = killer;
            let _ = killer.kill();
            let _ = child.wait();
            log_pty_detail("take_writer failed", &e.to_string());
            return Err("Could not attach to the agent terminal.".to_string());
        }
    };

    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(e) => {
            let mut killer = killer;
            let _ = killer.kill();
            let _ = child.wait();
            log_pty_detail("try_clone_reader failed", &e.to_string());
            return Err("Could not read from the agent terminal.".to_string());
        }
    };

    let ring: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
    let exited = Arc::new(AtomicBool::new(false));
    let master = Arc::new(Mutex::new(pair.master));
    let writer = Arc::new(Mutex::new(writer));

    // FIX 2 (insert-before-read): insert the session into the map BEFORE spawning
    // the reader thread. If the child exits in <1ms, the reader's EOF cleanup runs
    // `map.remove(agent_id)`; were the insert to happen AFTER that, the session
    // would be orphaned in the map forever (and stop_agent would misroute). By
    // inserting first (with no reader handle yet) the remove always finds and tears
    // down the right session; we then store the reader handle under the lock.
    let session = PtySession {
        master,
        killer,
        child: Some(child),
        writer,
        reader_handle: None,
        ring: Arc::clone(&ring),
        exited: Arc::clone(&exited),
    };
    {
        let mut map = sessions
            .inner
            .lock()
            .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
        map.insert(agent_id.to_string(), session);
    }

    // Reader thread: master -> ring buffer + per-chunk Tauri event. On EOF/error
    // it flips `exited`, emits the sentinel, marks the agent session closed, and
    // removes itself from the managed map. Adopted shape from terax-ai's reader
    // thread (read into a fixed buffer, append, notify), adapted to emit Tauri
    // events directly and to keep a bounded ring instead of an unbounded queue.
    let app_for_thread = app.clone();
    let agent_for_thread = agent_id.to_string();
    let reader_handle = match std::thread::Builder::new()
        .name(format!("agent-pty-{agent_id}"))
        .spawn(move || {
            reader_loop(app_for_thread, agent_for_thread, reader, ring, exited);
        }) {
        Ok(handle) => handle,
        Err(e) => {
            // The reader never started. Tear the just-inserted session down (kill +
            // reap + drop endpoints) so the live child is not leaked and the map
            // does not keep a reader-less, EOF-blind session.
            log_pty_detail("reader thread spawn failed", &e.to_string());
            kill_session_in_map(sessions, agent_id);
            return Err("Could not start the agent terminal reader.".to_string());
        }
    };

    // Store the reader handle under the lock. If the reader already EOF'd and
    // removed the session (fast child exit), there is nothing to store — join the
    // orphaned handle here so the thread is not leaked.
    {
        let mut map = sessions
            .inner
            .lock()
            .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
        match map.get_mut(agent_id) {
            Some(session) => session.reader_handle = Some(reader_handle),
            None => {
                let _ = reader_handle.join();
            }
        }
    }
    Ok(())
}

/// Reader thread body. Reads the master until EOF/error, appending to the ring
/// and emitting a `data` event per chunk; on exit emits the `exited` sentinel,
/// marks the agent session closed (best-effort), and removes the session from the
/// managed map so `agent_pty_list` no longer reports it.
fn reader_loop(
    app: tauri::AppHandle,
    agent_id: String,
    mut reader: Box<dyn Read + Send>,
    ring: Arc<Mutex<VecDeque<u8>>>,
    exited: Arc<AtomicBool>,
) {
    let event_name = terminal_event_name(&agent_id);
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break, // EOF: child closed the pty.
            Ok(n) => {
                let chunk = &buf[..n];
                if let Ok(mut ring) = ring.lock() {
                    push_capped(&mut ring, chunk, RING_CAPACITY);
                }
                // utf8-lossy so a chunk split mid-codepoint never panics; xterm on
                // the frontend reassembles the stream.
                let text = String::from_utf8_lossy(chunk).into_owned();
                let _ = app.emit(
                    &event_name,
                    TerminalEvent {
                        data: Some(text),
                        exited: None,
                    },
                );
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break, // read error (closed pty); treat as exit.
        }
    }

    exited.store(true, Ordering::SeqCst);
    let _ = app.emit(
        &event_name,
        TerminalEvent {
            data: None,
            exited: Some(true),
        },
    );

    // Best-effort: drop the session from the managed map and reflect the closure in
    // the shared agent live-state. Errors are swallowed — the UI already saw the
    // `exited` sentinel, and a missing state file is not fatal here.
    //
    // ORDER (FIX 8): remove the session from the map BEFORE marking the live-state
    // closed. agent_pty_list reads the map; if we marked the state closed first, a
    // viewer polling between the two steps would see a "closed" status while the
    // PTY session is still listed as live — a one-poll inconsistency. Removing from
    // the map first closes that window.
    if let Some(sessions) = app.try_state::<AgentPtySessions>() {
        let removed = sessions
            .inner
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&agent_id));
        if let Some(mut session) = removed {
            // We reached EOF because the child exited on its own. REAP it (wait)
            // so no zombie/defunct process is left — `Child`'s Drop does not
            // guarantee a wait. Detach our own handle first so we never try to
            // join the thread we are currently running on.
            session.reader_handle = None;
            if let Some(mut child) = session.child.take() {
                let _ = child.wait();
            }
            // master/writer/killer drop here, closing the (already-dead) pty.
        }
    }
    super::agents::mark_agent_session_closed_public(&app, &agent_id);
}

/// Lossy snapshot of the ring buffer for a late-joining viewer (the scrollback to
/// render before live events take over).
#[tauri::command]
pub fn agent_pty_snapshot(
    state: State<'_, BackendState>,
    sessions: State<'_, AgentPtySessions>,
    agent_id: String,
) -> Result<String, String> {
    state.ensure_unlocked()?;
    validate_agent_id(&agent_id)?;
    let map = sessions
        .inner
        .lock()
        .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
    let session = map
        .get(&agent_id)
        .ok_or_else(|| "No app terminal for this agent.".to_string())?;
    let ring = session
        .ring
        .lock()
        .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
    let bytes: Vec<u8> = ring.iter().copied().collect();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Maximum bytes accepted by a single `agent_pty_write`. Keystrokes/paste are tiny;
/// this caps a malicious/buggy frontend from forcing an unbounded write into the
/// pty. 64 KiB comfortably covers any realistic paste while bounding the call.
const MAX_WRITE_BYTES: usize = 64 * 1024;

/// Write raw bytes (keystrokes/paste) from the frontend to the agent's pty input.
#[tauri::command]
pub fn agent_pty_write(
    state: State<'_, BackendState>,
    sessions: State<'_, AgentPtySessions>,
    agent_id: String,
    data: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    validate_agent_id(&agent_id)?;
    if data.len() > MAX_WRITE_BYTES {
        return Err("write too large".to_string());
    }
    // LOCKING ORDER: take the map lock ONLY to clone out the writer Arc, then drop
    // it BEFORE the blocking write/flush (see the note at the top of this file).
    let writer = {
        let map = sessions
            .inner
            .lock()
            .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
        let session = map
            .get(&agent_id)
            .ok_or_else(|| "No app terminal for this agent.".to_string())?;
        Arc::clone(&session.writer)
    };
    let mut writer = writer
        .lock()
        .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
    writer.write_all(data.as_bytes()).map_err(|e| {
        log_pty_detail("pty write failed", &e.to_string());
        "Could not send input to the agent terminal.".to_string()
    })?;
    writer.flush().map_err(|e| {
        log_pty_detail("pty flush failed", &e.to_string());
        "Could not send input to the agent terminal.".to_string()
    })?;
    Ok(())
}

/// Frame a single conversational message for an agent PTY: a PTY treats EVERY '\r'/'\n' as
/// "submit line", so a multi-line message would fire multiple premature submits. We therefore
/// (1) strip trailing newlines, (2) replace every INTERNAL '\r'/'\n' with a single space so the
/// whole message is one line, then (3) append exactly one '\r' (the single Enter that submits it).
pub(crate) fn frame_agent_message(message: &str) -> String {
    let trimmed = message.trim_end_matches(['\r', '\n']);
    let replaced: String = trimmed
        .chars()
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect();
    format!("{}\r", replaced)
}

/// Send a conversational message (one human turn) to a cloud agent running in an
/// app-hosted PTY (Claude/Codex). The structured counterpart to `project_cloud_orchestrator_send`
/// for the PTY path: frames the text as `message + "\r"` and writes it via the same
/// writer path as `agent_pty_write` (no new transport — reuse).
#[tauri::command]
pub fn agent_pty_send_message(
    state: State<'_, BackendState>,
    sessions: State<'_, AgentPtySessions>,
    agent_id: String,
    message: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    validate_agent_id(&agent_id)?;
    if message.trim().is_empty() {
        return Err("empty message".to_string());
    }
    // `>=`: the framed string appends one '\r', so an exactly-MAX message would write
    // MAX+1 bytes. Bound the framed write to MAX_WRITE_BYTES.
    if message.len() >= MAX_WRITE_BYTES {
        return Err("write too large".to_string());
    }
    let data = frame_agent_message(&message);
    let writer = {
        let map = sessions
            .inner
            .lock()
            .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
        let session = map
            .get(&agent_id)
            .ok_or_else(|| "No app terminal for this agent.".to_string())?;
        Arc::clone(&session.writer)
    };
    let mut writer = writer
        .lock()
        .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
    writer.write_all(data.as_bytes()).map_err(|e| {
        log_pty_detail("pty write failed", &e.to_string());
        "Could not send input to the agent terminal.".to_string()
    })?;
    writer.flush().map_err(|e| {
        log_pty_detail("pty flush failed", &e.to_string());
        "Could not send input to the agent terminal.".to_string()
    })?;
    Ok(())
}

/// Resize the agent's pty to the viewer's current geometry.
#[tauri::command]
pub fn agent_pty_resize(
    state: State<'_, BackendState>,
    sessions: State<'_, AgentPtySessions>,
    agent_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    validate_agent_id(&agent_id)?;
    // Guard against a zero dimension (a momentarily-hidden viewer can report 0):
    // a 0x0 ConPTY resize is rejected by the OS and would error needlessly.
    let cols = cols.max(1);
    let rows = rows.max(1);
    // LOCKING ORDER: clone out the master Arc under the map lock, drop the map lock,
    // THEN do the blocking resize (see the note at the top of this file).
    let master = {
        let map = sessions
            .inner
            .lock()
            .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
        let session = map
            .get(&agent_id)
            .ok_or_else(|| "No app terminal for this agent.".to_string())?;
        Arc::clone(&session.master)
    };
    let master = master
        .lock()
        .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
    master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| {
            log_pty_detail("pty resize failed", &e.to_string());
            "Could not resize the agent terminal.".to_string()
        })
}

/// Active agent_ids that currently have a live app-hosted PTY session, so the UI
/// knows which agents to offer an in-app terminal viewer for. Cheap (a key clone).
#[tauri::command]
pub fn agent_pty_list(
    state: State<'_, BackendState>,
    sessions: State<'_, AgentPtySessions>,
) -> Result<Vec<String>, String> {
    state.ensure_unlocked()?;
    let map = sessions
        .inner
        .lock()
        .map_err(|_| "Agent terminal state is unavailable.".to_string())?;
    Ok(map.keys().cloned().collect())
}

/// Kill an app-hosted agent: kill the child, then `wait` it so it is reaped (no
/// zombie), join the reader thread best-effort with a timeout, drop the session,
/// and mark the agent session closed. Idempotent: a missing session is success.
#[tauri::command]
pub fn agent_pty_kill(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    sessions: State<'_, AgentPtySessions>,
    agent_id: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    validate_agent_id(&agent_id)?;
    kill_session_in_map(&sessions, &agent_id);
    // Reflect closure in the shared live-state too (the reader thread also does
    // this on EOF, but an explicit kill should not depend on the reader winning
    // the race). Best-effort.
    super::agents::mark_agent_session_closed_public(&app, &agent_id);
    Ok(())
}

/// Kill + reap an app-hosted agent's PTY session by id, resolving the managed
/// state from the AppHandle. Used by `stop_agent` (agents.rs) when the ledger
/// host is "app", so the stop path does not need the `AgentPtySessions` State in
/// its own signature. Best-effort and idempotent: a missing session/state is a
/// no-op. Does NOT touch the live-state file — the caller (`stop_agent`) already
/// marks the session closed.
pub fn kill_agent_pty(app: &tauri::AppHandle, agent_id: &str) {
    if let Some(sessions) = app.try_state::<AgentPtySessions>() {
        kill_session_in_map(&sessions, agent_id);
    }
}

/// True iff a live app-hosted PTY session exists for `agent_id`. Used by the ledger
/// pruner (agents.rs) to decide whether an app-hosted entry is still alive without
/// an OS window/pid to probe. A poisoned map lock is treated as "exists" (fail-safe:
/// never report a session gone when we cannot read the map).
pub fn pty_session_exists(sessions: &AgentPtySessions, agent_id: &str) -> bool {
    match sessions.inner.lock() {
        Ok(map) => map.contains_key(agent_id),
        Err(_) => true,
    }
}

/// Remove a session from the map (if present) and tear it down: kill -> wait (reap)
/// -> best-effort join the reader. Used by `agent_pty_kill`, relaunch-replace, and
/// the app-exit reaper. Never blocks longer than the bounded reader join.
fn kill_session_in_map(sessions: &AgentPtySessions, agent_id: &str) {
    let session = {
        let Ok(mut map) = sessions.inner.lock() else {
            return;
        };
        map.remove(agent_id)
    };
    if let Some(session) = session {
        teardown_session(session);
    }
}

/// Kill + reap one session. ORDER MATTERS, and the Windows ConPTY ordering is the
/// subtle part (the integration test encodes the same lesson):
///   1) kill the child (idempotent);
///   2) drop the master AND writer FIRST — on Windows the pseudoconsole host
///      process stays alive while the master handle is open and `Child::wait`
///      waits on that host, so waiting while still holding the master deadlocks.
///      Dropping the master also closes the pty so the reader's blocking read
///      returns EOF and the thread can finish;
///   3) THEN wait() to reap the child (no zombie/defunct process);
///   4) THEN best-effort bounded-join the (now-unblocked) reader thread.
fn teardown_session(session: PtySession) {
    session.exited.store(true, Ordering::SeqCst);
    let PtySession {
        master,
        mut killer,
        child,
        writer,
        reader_handle,
        exited: _,
        ring: _,
    } = session;
    // 1) Kill (idempotent).
    let _ = killer.kill();
    // 2) Drop both PTY ends BEFORE wait() to avoid the ConPTY host deadlock and to
    //    unblock the reader (EOF). `writer` holds a master-side handle too. These
    //    are Arc<Mutex<..>>: dropping our refs closes the pty once they are the LAST
    //    refs. The map ref is already gone (the session was removed before teardown)
    //    so the only other possible holder is a transient in-flight write/resize
    //    that cloned the Arc; that call closes its ref the instant it returns (and
    //    its I/O errors out against the killed child), so EOF still arrives.
    drop(writer);
    drop(master);
    // 3) Reap so no zombie is left behind. With the pty closed and the child
    //    killed, this returns promptly.
    if let Some(mut child) = child {
        let _ = child.wait();
    }
    // 4) Best-effort bounded join of the reader thread. We cannot truly time-box a
    //    std JoinHandle, but by this point the master is dropped so the reader is
    //    unblocked; spin briefly waiting for it to finish, then give up rather than
    //    block teardown forever.
    //
    //    FIX 7 (app-quit hang): the spin budget is 150ms, not 500ms. By step 2 the
    //    master/writer are dropped and step 3's wait() has returned, so the reader's
    //    blocking read has ALREADY hit EOF and the thread is finishing — it almost
    //    always reports finished on the first poll. The old 500ms was a worst-case
    //    that, multiplied by N sessions in the sequential app-exit reaper, added up
    //    to a multi-second quit hang. 150ms keeps a generous margin for the EOF to
    //    propagate while bounding the per-session quit cost to ~0.15s; if a reader
    //    still is not done we leave it (the master is dropped, so it exits
    //    imminently on its own) rather than block app teardown.
    if let Some(handle) = reader_handle {
        let deadline = Instant::now() + READER_JOIN_BUDGET;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
        // If still not finished, leave it: the master is dropped, so it will exit
        // imminently on its own; we do not block app teardown on it.
    }
}

/// Per-session bounded wait for the reader thread to finish during teardown. Kept
/// small (FIX 7) so the sequential app-exit reaper does not accumulate a multi-second
/// quit hang across N sessions; the reader is already unblocked (master dropped) by
/// the time we wait, so it normally finishes on the first poll.
const READER_JOIN_BUDGET: Duration = Duration::from_millis(150);

/// App-exit reaper: kill + reap EVERY live app-hosted PTY child. PAST-LESSON item
/// — on Windows a child does not die with the parent, and a dev Ctrl-C must not
/// orphan agent shells. Called from the lib.rs RunEvent::Exit handler.
pub fn kill_all_on_exit(app: &tauri::AppHandle) {
    let Some(sessions) = app.try_state::<AgentPtySessions>() else {
        return;
    };
    let drained: Vec<PtySession> = {
        let Ok(mut map) = sessions.inner.lock() else {
            return;
        };
        map.drain().map(|(_, session)| session).collect()
    };
    for session in drained {
        teardown_session(session);
    }
}

/// Validate an agent_id before it is used in an event channel name or as a map key.
/// The id is concatenated into `agent-terminal://<id>` and used across commands, so
/// we restrict it to a safe allowlist. projects.rs generates ids as
/// `"{role}-{millis}"` (role is a normalized lowercase word, millis is digits) and
/// also accepts a caller-supplied id; the allowlist `[A-Za-z0-9._-]{1,64}` covers
/// the generated shape with margin while rejecting `/`, whitespace, control chars,
/// and anything that could smuggle an unexpected channel name.
pub fn validate_agent_id(agent_id: &str) -> Result<(), String> {
    if agent_id.is_empty() || agent_id.len() > 64 {
        return Err("Invalid agent id.".to_string());
    }
    if agent_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err("Invalid agent id.".to_string())
    }
}

/// Log a PTY error detail WITHOUT surfacing it to the user (the returned error
/// strings are static). Mirrors the oracle-command sanitize discipline: static
/// user message, detail to the log only. The detail here is only portable-pty's own
/// I/O error text (never a prompt-file path or env value), but to stay defensive we
/// still strip absolute drive-letter paths and truncate, rather than reaching into
/// the oracle module for its private sanitizer (awkward cross-module coupling).
fn log_pty_detail(context: &str, detail: &str) {
    eprintln!("[agent_pty] {context}: {}", sanitize_detail(detail));
}

/// Truncate to 200 chars and blank out anything that looks like a Windows
/// drive-letter absolute path (`C:\...` / `C:/...`), regex-free. Defensive only —
/// the inputs are portable-pty I/O error strings, not our own paths.
fn sanitize_detail(detail: &str) -> String {
    let mut out = String::with_capacity(detail.len().min(200));
    let bytes = detail.as_bytes();
    let mut i = 0;
    // Track the emitted char count incrementally; `out.chars().count()` per
    // iteration was O(n^2) on long inputs (FIX 9).
    let mut char_count = 0usize;
    while i < bytes.len() && char_count < 200 {
        // Detect a drive-letter path start: <ascii letter> ':' ('\' | '/').
        if i + 2 < bytes.len()
            && bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && (bytes[i + 2] == b'\\' || bytes[i + 2] == b'/')
        {
            out.push_str("<path>");
            char_count += "<path>".chars().count();
            // Skip until whitespace, quote, or end (consume the whole path token).
            i += 3;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && bytes[i] != b'"'
                && bytes[i] != b'\''
            {
                i += 1;
            }
            continue;
        }
        // Copy the next whole UTF-8 char so we never split a codepoint.
        let ch = detail[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        char_count += 1;
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_agent_message_appends_a_single_carriage_return() {
        // A structured "send message" frames the text as one conversational turn:
        // the message followed by a single Enter (carriage return) for the PTY.
        assert_eq!(frame_agent_message("narrow to the parser"), "narrow to the parser\r");
    }

    #[test]
    fn frame_agent_message_strips_existing_trailing_newlines_to_avoid_double_submit() {
        // The frontend may include a trailing newline; we must not submit twice.
        assert_eq!(frame_agent_message("use Auth0\n"), "use Auth0\r");
        assert_eq!(frame_agent_message("use Auth0\r\n"), "use Auth0\r");
        assert_eq!(frame_agent_message("use Auth0\r"), "use Auth0\r");
    }

    #[test]
    fn frame_agent_message_collapses_internal_newlines_into_one_pty_line() {
        // A PTY treats every \r/\n as Enter, so internal newlines must become spaces —
        // otherwise a multi-line message fires several premature, truncated submits.
        assert_eq!(frame_agent_message("multi\nline body\n"), "multi line body\r");
        // Each internal \r and \n maps to its own space (a \r\n pair -> two spaces);
        // harmless for a PTY line, and never a premature submit.
        assert_eq!(frame_agent_message("a\r\nb\r\n"), "a  b\r");
    }

    #[test]
    fn push_capped_keeps_within_capacity_dropping_oldest() {
        let mut ring = VecDeque::new();
        push_capped(&mut ring, b"abcd", 8);
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), b"abcd");

        // Pushing past the cap drops the oldest bytes, keeping the newest tail.
        push_capped(&mut ring, b"efghij", 8);
        assert_eq!(ring.len(), 8);
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), b"cdefghij");
    }

    #[test]
    fn push_capped_chunk_larger_than_cap_keeps_only_tail() {
        let mut ring = VecDeque::new();
        push_capped(&mut ring, b"hello", 16);
        // A single chunk that alone exceeds the cap evicts everything older and
        // keeps only the last `cap` bytes of the chunk.
        // "0123456789abcdefXYZ" is 19 bytes; with cap 8 only its last 8 bytes
        // ("bcdefXYZ") survive and everything older (incl. the prior "hello") is
        // evicted.
        push_capped(&mut ring, b"0123456789abcdefXYZ", 8);
        assert_eq!(ring.len(), 8);
        assert_eq!(
            ring.iter().copied().collect::<Vec<_>>(),
            b"bcdefXYZ".to_vec()
        );
    }

    #[test]
    fn push_capped_zero_cap_retains_nothing() {
        let mut ring = VecDeque::new();
        push_capped(&mut ring, b"abc", 0);
        assert!(ring.is_empty());
    }

    #[test]
    fn push_capped_exact_fit_then_one_more() {
        let mut ring = VecDeque::new();
        push_capped(&mut ring, b"abcd", 4);
        assert_eq!(ring.len(), 4);
        push_capped(&mut ring, b"e", 4);
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), b"bcde");
    }

    #[test]
    fn validate_agent_id_accepts_generated_shape() {
        // The projects.rs-generated form: "{role}-{millis}".
        assert!(validate_agent_id("coder-1717459200000").is_ok());
        assert!(validate_agent_id("reviewer-7f").is_ok());
        assert!(validate_agent_id("a").is_ok());
        assert!(validate_agent_id("agent.id_1-2").is_ok());
        assert!(validate_agent_id(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn validate_agent_id_rejects_unsafe_or_oversized() {
        assert!(validate_agent_id("").is_err());
        assert!(validate_agent_id(&"x".repeat(65)).is_err());
        // Path/channel-smuggling and whitespace/control chars are rejected.
        assert!(validate_agent_id("coder/../evil").is_err());
        assert!(validate_agent_id("agent terminal").is_err());
        assert!(validate_agent_id("a:b").is_err());
        assert!(validate_agent_id("a\nb").is_err());
        assert!(validate_agent_id("emoji😀").is_err());
    }

    #[test]
    fn sanitize_detail_strips_drive_letter_paths_and_truncates() {
        let s = sanitize_detail("open failed at C:\\Users\\me\\secret.txt now");
        assert!(!s.contains("Users"), "got: {s}");
        assert!(s.contains("<path>"));
        assert!(s.contains("open failed"));
        // Forward-slash drive paths too.
        let s2 = sanitize_detail("err D:/data/token.json end");
        assert!(!s2.contains("data"));
        assert!(s2.contains("<path>"));
        // Truncation to 200 chars.
        let long = "z".repeat(500);
        assert_eq!(sanitize_detail(&long).chars().count(), 200);
    }

    #[test]
    fn sanitize_detail_counts_multibyte_chars_correctly() {
        // FIX 9: the incremental char count must count CHARS, not bytes. A string
        // of multibyte chars must still be truncated to 200 chars (not 200 bytes).
        let long = "é".repeat(500); // 2 bytes each, 1 char each
        let out = sanitize_detail(&long);
        assert_eq!(out.chars().count(), 200);
        // And a sub-200 input is preserved verbatim.
        let short = "héllo wörld";
        assert_eq!(sanitize_detail(short), short);
    }

    #[test]
    fn reader_join_budget_is_small_enough_to_avoid_quit_hang() {
        // FIX 7: the per-session reader-join budget must stay small so the
        // sequential app-exit reaper does not accumulate a multi-second quit hang.
        assert!(READER_JOIN_BUDGET <= Duration::from_millis(200));
    }

    #[test]
    fn pty_session_exists_false_for_absent_id() {
        let sessions = AgentPtySessions::new();
        assert!(!pty_session_exists(&sessions, "coder-1"));
    }

    #[test]
    fn terminal_event_name_is_namespaced_per_agent() {
        assert_eq!(terminal_event_name("coder-7f"), "agent-terminal://coder-7f");
    }

    #[test]
    fn terminal_event_serializes_data_and_exited_variants() {
        let data = TerminalEvent {
            data: Some("hi".into()),
            exited: None,
        };
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"data\":\"hi\""));
        assert!(!json.contains("exited"));

        let exit = TerminalEvent {
            data: None,
            exited: Some(true),
        };
        let json = serde_json::to_string(&exit).unwrap();
        assert!(json.contains("\"exited\":true"));
        assert!(!json.contains("data"));
    }

    // INTEGRATION: spawn a real child through portable-pty (NOT the Tauri command
    // layer, which needs an AppHandle), read until EOF, and assert the ring buffer
    // captured the output and the child was reaped without a zombie. Windows-only
    // here (`cmd /c echo`); ignored by default so CI without a console does not
    // flake, but RUN LOCALLY with `cargo test -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "spawns a real PTY child; run locally with --ignored"]
    fn real_pty_echo_is_captured_and_child_reaped() {
        use std::time::Instant;

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: INITIAL_ROWS,
                cols: INITIAL_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        // A unique marker printed via PowerShell Write-Host, invoked directly (no
        // nested `cmd /c` quoting). Write-Host writes straight to the console
        // buffer ConPTY streams out, so the marker is reliably captured. The marker
        // is unusual enough not to collide with the ConPTY init escape sequences.
        let mut cmd = CommandBuilder::new("powershell.exe");
        cmd.args(["-NoProfile", "-Command", "Write-Host ASPISPTYMARKER"]);
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        // Drop our slave copy so only the child holds the write end.
        drop(pair.slave);

        // ConPTY emits a Device Status Report (`ESC [ 6 n`, cursor-position query)
        // at startup and STALLS its render pipeline until the controlling terminal
        // replies. A real terminal (and the app's xterm front-end) answers it; in
        // this headless test we must answer it ourselves or no child output is ever
        // streamed. Write a cursor-position report back on the master writer.
        let mut writer = pair.master.take_writer().expect("writer");
        let _ = writer.write_all(b"\x1b[1;1R");
        let _ = writer.flush();

        // Reader thread on a cloned reader (its own handle), exactly as production.
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let ring_for_thread = Arc::clone(&ring);
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut r = ring_for_thread.lock().unwrap();
                        push_capped(&mut r, &buf[..n], RING_CAPACITY);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        // ORDERING (the ConPTY subtlety this test exists to prove):
        //   - `child.wait()` BLOCKS while the master handle is open (the ConPTY
        //     host stays alive), so we must NOT wait before closing the master;
        //   - closing the master immediately EOFs the reader before cmd.exe's
        //     output is rendered into the ConPTY screen and streamed out.
        // So we POLL the ring (the reader keeps draining) until the echoed text
        // appears, with a generous timeout, THEN close the master, join the reader,
        // and reap the (already-exited) child. Production never needs this dance:
        // teardown force-kills first (child already gone) and drops the master
        // before wait().
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut captured = String::new();
        while Instant::now() < deadline {
            let bytes: Vec<u8> = ring.lock().unwrap().iter().copied().collect();
            captured = String::from_utf8_lossy(&bytes).into_owned();
            if captured.contains("ASPISPTYMARKER") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        // Close the master so the reader drains and EOFs, then join it.
        drop(pair.master);
        let join_deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() && Instant::now() < join_deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(handle.is_finished(), "reader thread should reach EOF");
        handle.join().expect("reader join");

        // Reap the child (no zombie). With the master closed it returns immediately.
        let status = child.wait().expect("wait");
        let _ = status;

        // Re-read the final ring (it may have grown after the poll loop broke).
        let final_bytes: Vec<u8> = ring.lock().unwrap().iter().copied().collect();
        let final_text = String::from_utf8_lossy(&final_bytes);
        let text = if final_text.contains("ASPISPTYMARKER") {
            final_text.into_owned()
        } else {
            captured
        };
        assert!(
            text.contains("ASPISPTYMARKER"),
            "ring buffer should contain echoed output, got: {text:?}"
        );
    }

    // INTEGRATION (FIX 2): a child that exits immediately must NOT orphan its
    // session in the managed map. We build a real PtySession around a `cmd /c exit
    // 0` child (the fastest-exiting process we can spawn), insert it into the map
    // exactly as `spawn_agent_pty` now does (insert-before-read), tear it down via
    // `kill_session_in_map`, and assert the map is EMPTY afterwards — i.e. the
    // remove always finds the inserted session. The pre-fix code inserted AFTER
    // spawning the reader, so a fast EOF could remove(None) then the late insert
    // would strand the session forever. Windows-only; ignored by default.
    #[cfg(windows)]
    #[test]
    #[ignore = "spawns a real PTY child; run locally with --ignored"]
    fn fast_exit_child_does_not_orphan_session_in_map() {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: INITIAL_ROWS,
                cols: INITIAL_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.args(["/c", "exit", "0"]);
        let child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);

        let killer = child.clone_killer();
        let writer = pair.master.take_writer().expect("writer");
        let session = PtySession {
            master: Arc::new(Mutex::new(pair.master)),
            killer,
            child: Some(child),
            writer: Arc::new(Mutex::new(writer)),
            // No reader thread in this test — we exercise the map insert/remove +
            // teardown ordering, not the reader EOF path (which needs an AppHandle).
            reader_handle: None,
            ring: Arc::new(Mutex::new(VecDeque::new())),
            exited: Arc::new(AtomicBool::new(false)),
        };

        let sessions = AgentPtySessions::new();
        // Insert FIRST (the fixed ordering), even though the child may already be
        // dead by now.
        sessions
            .inner
            .lock()
            .unwrap()
            .insert("coder-fast".to_string(), session);

        // Give the child a moment to actually exit so wait() in teardown reaps it.
        std::thread::sleep(Duration::from_millis(50));

        kill_session_in_map(&sessions, "coder-fast");

        let map = sessions.inner.lock().unwrap();
        assert!(
            map.is_empty(),
            "fast-exit session must not be orphaned in the map after teardown"
        );
    }
}
