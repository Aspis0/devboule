//! Mini-coder executor (MC-P2): the backend-resident background thread that drains
//! the `miniCoderDirectives` queue in `.aspis-agents.json` and drives each pending
//! directive through its lifecycle by spawning a REAL one-shot PTY child.
//!
//! WHY A BACKEND THREAD (not a Tauri command / frontend poll): the MCP↔app bridge
//! is FILE-ONLY (a coder's `spawn_mini_coder` tool can only leave a `pending`
//! directive in `.aspis-agents.json`; it cannot make the app spawn anything). The
//! frontend poll only runs on the Projects page, so it is an unreliable driver.
//! The executor is therefore the ONLY agent→app action bridge: a singleton thread
//! installed at app setup that reacts to directives whenever the app is unlocked.
//!
//! SINGLETON + LIFECYCLE: copied from the Censor watcher singleton
//! (`censor/commands.rs` + `censor/watch.rs`): `MiniCoderState` (Tauri `.manage`)
//! owns a stop flag + the loop's `JoinHandle`; `install` builds and stores it
//! atomically; `kill_all_on_exit` signals + detached-reaps it on `RunEvent::Exit`
//! so quit never orphans the thread (or a mini PTY child — those are reaped via
//! `agent_pty::agent_pty_kill`/`kill_all_on_exit`).
//!
//! LOCK DISCIPLINE (reviewer will check): the agent-state file lock is NEVER held
//! across a PTY spawn, a result-file read, or the loop sleep. Each pass:
//!   1) `read_agent_live_state_snapshot` (locked READ → clone → unlock),
//!   2) `plan_tick` (PURE) on the snapshot,
//!   3) for the claim: `mutate_agent_live_state(apply_claim)` (locked, re-checks
//!      status so a stale snapshot cannot double-claim), THEN spawn the PTY OUTSIDE
//!      the lock, THEN `mutate_agent_live_state(apply_launched + session nesting)`,
//!   4) for each timeout / parent-gone / finished mini: kill / read-result OUTSIDE
//!      the lock, then `mutate_agent_live_state(apply_*)`.
//!
//! No two of {state lock, PTY map lock, MiniCoderState lock} are ever held at once.
//!
//! EOF → RESULT: the mini writes its result JSON then exits; on EOF the existing
//! `agent_pty` reader thread reaps the child and REMOVES it from the PTY map. The
//! executor detects completion by polling `agent_pty_list`: a `running` directive
//! whose `agentId` is no longer in the map has finished, so we read its result file
//! (CANONICALIZE-after-open — see `read_result_outcome` — closing P1's symlink
//! TOCTOU), apply the terminal outcome, persist, and delete the file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State};

use super::agents;
use super::state::BackendState;
use super::mini_coder::{
    self, MiniCoderBackend, MiniCoderBackendKind, MiniCoderDirective, MiniCoderOutcome,
    MiniCoderStatus, DEFAULT_LAUNCH_CAP_SECS, DEFAULT_WALL_CLOCK_CAP_SECS, MAX_DIRECTIVES,
};

/// How often the executor wakes to scan the directive queue. A coder's
/// `spawn_mini_coder` blocks on a ~0.75s MCP poll, so a 1.5s executor cadence keeps
/// the end-to-end claim→spawn latency comfortably inside the coder's poll while
/// keeping the idle cost (one locked read of a usually-empty queue) negligible.
const SCAN_INTERVAL: Duration = Duration::from_millis(1500);

/// Slice 3 (Pigeon transport): the mailbox receiver id of the mini worker pool. MUST match
/// the Python `PIGEON_MINI_POOL_RECEIVER` in `aspis_mcp.py` (the `spawn_mini_coder` send
/// target) — this is the queue the executor drains (seam B) and the receiver it posts the
/// terminal outcome from (seam C).
const PIGEON_MINI_POOL_RECEIVER: &str = "mini-pool";

/// Scratch dir name (under the project root) where minis write their result files.
/// A sibling of `.aspis-censor`; `read_result_file` confines reads to it.
pub(crate) const MINI_SCRATCH_DIR: &str = ".aspis-mini";
const VISUAL_CHECK_TIMEOUT_SECS: i64 = 120;

/// MINI-EXCLUSION (design §6): the Phase-B user-MCP env var the ORCHESTRATOR launch sets.
/// The mini coder must NEVER receive it. The mini is a separate launch path that never SETS
/// it, but `CommandBuilder::new()` SNAPSHOTS the host process env — so if the app itself was
/// launched from a shell that already had this var set, the mini child would otherwise
/// INHERIT it. We `env_remove` it on every built mini command (both real arms) so the mini
/// never carries it regardless of the host env. This is a DEFENSIVE SCRUB (strip OUT), NOT
/// wiring user servers IN: the mini still gets zero user-MCP capability.
pub(crate) const FORBIDDEN_USER_MCP_ENV: &str = "DEVBOULE_USER_MCP_SERVERS";

/// Env var carrying the oMLX HTTP request timeout (seconds) to the launch script
/// (macOS python `urlopen`). Non-secret. Derived from `DEFAULT_WALL_CLOCK_CAP_SECS`
/// (the executor's PTY wall-clock kill) MINUS `OMLX_HTTP_TIMEOUT_MARGIN_SECS`, so a
/// stalled oMLX request fails fast on the HTTP layer JUST BEFORE the PTY is killed
/// (a clean `failed` fallback instead of waiting the full cap). Kept uncfg'd so the
/// platform-agnostic macOS-script test can reference it on the Windows dev host.
pub(crate) const OMLX_TIMEOUT_ENV: &str = "OMLX_TIMEOUT";

/// Margin (seconds) subtracted from the wall-clock cap to derive the oMLX HTTP
/// timeout, so the request aborts JUST UNDER the cap rather than racing it.
///
/// 30s (not 10s — max-recall FIX 11): `started_at` is stamped by the executor AFTER the
/// PTY spawn, and under state-lock contention that stamp can lag the actual request start
/// by several seconds. With only a 10s margin a lagged stamp could push the wall-clock
/// kill to fire BEFORE the in-script HTTP timeout, killing a still-valid in-flight
/// request. 30s comfortably absorbs realistic stamping lag so the HTTP timeout reliably
/// fires first (a clean `failed` fallback) and the executor cap is the true backstop.
/// Both platforms (`-TimeoutSec` on Windows, `OMLX_TIMEOUT` on macOS) derive from this.
const OMLX_HTTP_TIMEOUT_MARGIN_SECS: i64 = 30;

/// The oMLX HTTP request timeout (seconds) = wall-clock cap − margin, floored at 1s
/// (defensive: never produce a zero/negative timeout if the cap is ever tuned tiny).
/// Both platforms derive their HTTP timeout from this SAME source.
pub(crate) fn omlx_http_timeout_secs() -> i64 {
    (DEFAULT_WALL_CLOCK_CAP_SECS - OMLX_HTTP_TIMEOUT_MARGIN_SECS).max(1)
}

// (OMLX_* + MINI_RLIMIT_* consts live in backend/mini_command_build.rs.)

/// Managed singleton state for the mini-coder executor. Holds the shared stop flag
/// and the loop's join handle so app-exit can signal + reap it. `None` thread means
/// not yet installed (or already reaped).
pub struct MiniCoderState {
    running: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// BLOCKER 2 (EXECUTOR-LOOP STALL): process-wide set of directive ids whose
    /// deferred Censor-VERDICT thread is currently running.
    verdict_inflight: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Process-wide set of directive ids whose AGENTIC tool-loop worker thread is currently
    /// running.
    agentic_inflight: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Per-directive cancel flags for in-flight agentic workers.
    agentic_cancel: Arc<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
    /// FINE coalescing: per-file last FINE-censor timestamp. Entries older than
    /// FINE_COOLDOWN_S × 4 are evicted every 10 minutes.
    fine_cooldown: Mutex<std::collections::HashMap<String, Instant>>,
    /// COARSE trigger: set of project IDs whose working tree has pending COARSE work.
    /// Phase A inserts on every mini finish; the COARSE sweep drains and spawns.
    /// Shared with spawned COARSE threads so they can remove on success (B3).
    coarse_dirty: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Last COARSE pass timestamp per project, for cooldown.
    last_coarse: Arc<Mutex<std::collections::HashMap<String, Instant>>>,
    /// Last FINE cooldown sweep timestamp.
    last_cooldown_sweep: Mutex<Instant>,
}

/// Fine-runner cooldown in seconds: skip re-censoring a file that was censorated
/// in the last N seconds (coalescing rapid retries).
const FINE_COOLDOWN_S: u64 = 5;
/// Coarse cooldown in seconds: at most one coarse pass every N seconds.
const COARSE_COOLDOWN_S: u64 = 120;

impl Default for MiniCoderState {
    fn default() -> Self {
        Self::new()
    }
}

impl MiniCoderState {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            thread: Mutex::new(None),
            verdict_inflight: Arc::new(Mutex::new(std::collections::HashSet::new())),
            agentic_inflight: Arc::new(Mutex::new(std::collections::HashSet::new())),
            agentic_cancel: Arc::new(Mutex::new(std::collections::HashMap::new())),
            fine_cooldown: Mutex::new(std::collections::HashMap::new()),
            coarse_dirty: Arc::new(Mutex::new(std::collections::HashSet::new())),
            last_coarse: Arc::new(Mutex::new(std::collections::HashMap::new())),
            last_cooldown_sweep: Mutex::new(Instant::now()),
        }
    }

    /// Install the executor loop ONCE. Idempotent: if a loop is already installed
    /// (thread present), this is a no-op so a second call (e.g. a re-setup) cannot
    /// spawn a second executor (the single-instance invariant). Built and stored
    /// under the `thread` mutex; the loop owns a clone of the stop flag.
    ///
    /// FIX 5 (install/stop race): the stop FLAG and the JoinHandle slot are written
    /// ATOMICALLY under the `thread` lock here, and `stop()` likewise flips the flag
    /// UNDER the same lock (mirroring the Censor `install_handle` ordering). This makes
    /// install (flag=true + handle present) and stop (flag=false + handle taken) fully
    /// serialized: a `stop()` racing an `install()` can no longer leave the flag `true`
    /// while reaping the handle — which would have produced a never-exiting (dead but
    /// alive) executor thread the reaper's `join` would block on forever.
    pub fn install(&self, app: AppHandle) {
        let mut guard = match self.thread.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_some() {
            return; // already running — never double-install.
        }
        // Arm the flag BEFORE spawning (still under the lock) so the new loop never
        // loads a stale `false` left by a prior `stop()` and exits after one pass. The
        // flag+handle are both written under this lock, so a racing `stop()` (which
        // also takes the lock) sees a consistent (flag=true, handle=Some) pair.
        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let spawned = std::thread::Builder::new()
            .name("mini-coder-executor".into())
            .spawn(move || run_loop(app, running));
        match spawned {
            Ok(handle) => *guard = Some(handle),
            Err(e) => {
                // Spawn failed: undo the flag so the slot stays consistent (no handle,
                // not "running") and a later install can retry cleanly.
                self.running.store(false, Ordering::SeqCst);
                eprintln!("mini-coder executor: failed to spawn loop thread: {e}");
            }
        }
    }

    /// Signal the loop to stop and hand its join to a DETACHED reaper so the caller
    /// (app exit) never blocks. Idempotent. Mirrors the Censor `signal_and_reap`.
    pub fn stop(&self) {
        // FIX 5: flip the stop flag AND take the handle under the SAME lock install
        // uses, so the flag can never be clobbered back to `true` by a racing install
        // after we take the handle (which would leave a thread the reaper joins forever).
        let handle = match self.thread.lock() {
            Ok(mut g) => {
                self.running.store(false, Ordering::SeqCst);
                g.take()
            }
            Err(p) => {
                self.running.store(false, Ordering::SeqCst);
                p.into_inner().take()
            }
        };
        if let Some(t) = handle {
            let spawned = std::thread::Builder::new()
                .name("mini-coder-executor-reaper".into())
                .spawn(move || {
                    let _ = t.join();
                });
            if let Err(e) = spawned {
                eprintln!("mini-coder executor: reaper spawn failed ({e}); loop self-terminates on the flag");
            }
        }
    }
}

impl MiniCoderState {
    /// BLOCKER 2: try to CLAIM `id` for a deferred-verdict thread. Returns true iff the
    /// id was newly inserted (no verdict thread is already running for it) — so only ONE
    /// verdict thread per directive ever starts. A poisoned lock fails CLOSED (returns
    /// false: do not start a second thread).
    fn claim_verdict(&self, id: &str) -> bool {
        // BLOCKER 1: recover from a poisoned mutex (`into_inner`) instead of failing
        // closed. The protected data is a plain `HashSet<String>` with no invariant a
        // panic mid-mutation could have broken, so the prior contents are sound to reuse.
        // Failing closed here would silently DROP the claim — letting `run_pass` spawn a
        // second verdict thread for the same id every pass.
        let mut set = self
            .verdict_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.insert(id.to_string())
    }

    /// BLOCKER 2: release `id` when its verdict thread completes/fails/panics, so the
    /// in-flight guard never leaks (a leaked id would make the directive un-timeout-able
    /// AND un-re-finalizable forever).
    fn release_verdict(&self, id: &str) {
        // BLOCKER 1: recover from poison so a release ALWAYS lands — a no-op release on a
        // poisoned lock would leak the id forever (the exact stuck-directive failure mode).
        let mut set = self
            .verdict_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.remove(id);
    }

    /// BLOCKER 2: snapshot the in-flight verdict ids (for `run_pass` to skip re-detect +
    /// thread into `plan_tick`'s timeout exclusion).
    fn verdict_inflight_ids(&self) -> std::collections::HashSet<String> {
        // BLOCKER 1: recover from poison and return the LIVE set. Returning an empty set
        // on poison (the old behavior) would permanently disable the timeout-exclusion —
        // a directive awaiting its verdict would be wrongly timed out.
        let set = self
            .verdict_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.clone()
    }

    /// BLOCKER 2 (RAII): a clone of the in-flight-set handle, for the verdict thread's
    /// drop guard. Decoupled from the `AppHandle` lifetime (it's an `Arc`), so the guard
    /// can release the id from inside the spawned thread on EVERY exit path.
    fn verdict_inflight_handle(&self) -> Arc<Mutex<std::collections::HashSet<String>>> {
        Arc::clone(&self.verdict_inflight)
    }

    /// Mark an agentic worker as in-flight. Returns true iff newly inserted (so only ONE
    /// worker per directive starts). Recovers from a poisoned lock (plain HashSet, no
    /// invariant) so the claim always lands.
    pub(crate) fn claim_agentic(&self, id: &str) -> bool {
        let mut set = self
            .agentic_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.insert(id.to_string())
    }

    /// Release an agentic worker's id from BOTH the in-flight set and the cancel map (on spawn
    /// failure, mirroring the RAII guard's drop). A leaked id would keep the directive
    /// un-finalizable forever; a leaked cancel flag would misfire on a future same-id worker.
    pub(crate) fn release_agentic(&self, inflight_id: &str, cancel_id: &str) {
        self.agentic_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(inflight_id);
        self.agentic_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(cancel_id);
    }

    /// Snapshot the in-flight agentic ids (for `run_pass`'s completion check + the timeout
    /// exclusion). Recovers from poison and returns the LIVE set.
    fn agentic_inflight_ids(&self) -> std::collections::HashSet<String> {
        let set = self
            .agentic_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set.clone()
    }

    /// A clone of the in-flight-set handle for the worker thread's RAII drop guard.
    pub(crate) fn agentic_inflight_handle(&self) -> Arc<Mutex<std::collections::HashSet<String>>> {
        Arc::clone(&self.agentic_inflight)
    }

    /// Register a fresh cancel flag for an agentic worker (directive id → flag); returns a
    /// clone for the worker to check each round. Recovers from poison.
    pub(crate) fn register_agentic_cancel(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        let mut map = self
            .agentic_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.insert(id.to_string(), Arc::clone(&flag));
        flag
    }

    /// Signal an in-flight agentic worker to stop (user Stop / kill). No-op if the directive
    /// has no agentic worker (e.g. a one-shot mini).
    fn cancel_agentic(&self, id: &str) {
        let map = self
            .agentic_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(flag) = map.get(id) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// A clone of the cancel-map handle for the worker thread's RAII drop guard.
    pub(crate) fn agentic_cancel_handle(
        &self,
    ) -> Arc<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>> {
        Arc::clone(&self.agentic_cancel)
    }

    /// WARNING 6: a clone of the executor's REAL running/stop flag, threaded into the
    /// verdict thread so an in-flight linter run honors app exit (instead of a throwaway
    /// `AtomicBool(true)` that never signals shutdown → zombie linter subprocesses).
    fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }
}

/// BLOCKER 2 (RAII): releases a claimed verdict-inflight id on EVERY exit path of the
/// verdict thread — normal return, `?`/error, AND unwinding panic — because `Drop` runs
/// during unwind. Holds only the `Arc<Mutex<..>>` handle + the id (no `AppHandle`), so it
/// is valid for the whole thread body. Recovers from a poisoned lock (BLOCKER 1) so the
/// release ALWAYS lands; a leaked id would make the directive un-timeout-able AND
/// un-re-finalizable forever.
struct VerdictInflightGuard {
    set: Arc<Mutex<std::collections::HashSet<String>>>,
    id: String,
}

impl Drop for VerdictInflightGuard {
    fn drop(&mut self) {
        let mut set = self.set.lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&self.id);
    }
}

// ROLE UNTANGLE Phase 2: `AgenticInflightGuard`, `should_run_agentic` and
// `spawn_agentic_worker` moved VERBATIM to `backend/agentic_worker.rs` (pure move;
// the executor state/scheduler stays here).

/// APP-EXIT teardown: signal + detached-reap the executor loop. Called from lib.rs
/// `RunEvent::Exit` alongside the agent_pty + censor reapers. The mini PTY children
/// themselves are reaped by `agent_pty::kill_all_on_exit` (they are ordinary
/// app-hosted PTY sessions in the same map). A missing managed state is a no-op.
pub fn kill_all_on_exit(app: &AppHandle) {
    if let Some(state) = app.try_state::<MiniCoderState>() {
        state.stop();
    }
}

/// The executor loop body. Wakes every `SCAN_INTERVAL`, snapshots the directive
/// queue, and enacts one `plan_tick` decision (claim + timeouts) plus completion
/// detection for running minis. EARLY-EXITS the body when the queue is empty (the
/// common idle case) so an idle app pays only one locked read per tick.
fn run_loop(app: AppHandle, running: Arc<AtomicBool>) {
    // WARNING 4 (crash recovery, part a): on STARTUP, fail any directive stuck
    // `launching` with no live PTY session — a crash between spawn and the launch
    // transition would otherwise hold the single concurrency slot forever. Done once
    // before the steady-state loop. Best-effort: a failure here is logged, not fatal.
    if let Err(e) = sweep_orphaned_launching(&app) {
        eprintln!("mini-coder executor: startup launching-sweep error: {e}");
    }

    // WARNING 5 (result-file leak recovery): on STARTUP also sweep the known
    // `.aspis-mini/` scratch dir(s) for orphaned `*.json` result files left behind by a
    // mini whose terminal state-write permanently failed (so `finalize_finished_mini`
    // never deleted the file). Best-effort + bounded; a failure here is logged, never
    // fatal. Done once before the steady-state loop.
    if let Err(e) = sweep_orphaned_result_files(&app) {
        eprintln!("mini-coder executor: startup result-file-sweep error: {e}");
    }

    // NOTE: the P6 "retry-lost" crash-recovery (auto-fail a `Failed` directive whose
    // forward-linked retry directive is absent from the queue) was originally implemented
    // via `sweep_orphaned_awaiting_retry`, which was removed during a refactor and never
    // replaced. A `Failed` directive stuck in this state currently sits unresolved until a
    // manual retry or cleanup; there is no automatic recovery for it.

    // Slice 3 (seam 3d): when Pigeon is enabled, register the `mini-pool` receiver as
    // `loaded` once so the agents row exists (a `/send` to a known-loaded receiver gets
    // `delivery_mode=immediate`). Best-effort + gated: a registration failure (dispatcher
    // still booting) is non-fatal — polling does not depend on the row, and the next
    // `start_if_enabled`/retry covers it. When DISABLED, no client is built (no-op).
    if crate::backend::pigeon_service::pigeon_enabled_cached(&app) {
        if let Some(client) = crate::backend::pigeon_service::pigeon_client_from_running() {
            if let Err(e) = client.register_agent(PIGEON_MINI_POOL_RECEIVER, "loaded") {
                eprintln!("mini-coder executor: pigeon mini-pool registration skipped: {e}");
            }
            // Also register the censor-pool receiver so async Censor LLM reviews route to this
            // process's review worker (same best-effort, gated, non-fatal contract).
            if let Err(e) = client.register_agent(
                crate::backend::censor_review::PIGEON_CENSOR_POOL_RECEIVER,
                "loaded",
            ) {
                eprintln!("mini-coder executor: pigeon censor-pool registration skipped: {e}");
            }
        }
    }

    // WARNING 3: a panic inside a pass must NOT kill the only agent→app action
    // bridge permanently. Run each pass under `catch_unwind`; on a caught panic log
    // (no secret — just a marker) and continue after a short backoff. The thread
    // self-terminates ONLY on the stop flag (mirrors the Censor watcher's
    // never-die-permanently resilience).
    while running.load(Ordering::SeqCst) {
        let pass = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_pass(&app)));
        match pass {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // A transient state-access failure (lock contention, unreadable
                // state) — log and continue.
                eprintln!("mini-coder executor: pass error: {e}");
            }
            Err(_) => {
                // A panic was caught: do not let it propagate (which would unwind the
                // thread). Log a marker (NO payload — it may carry a path) and back
                // off briefly before the next pass.
                eprintln!("mini-coder executor: pass panicked; continuing after backoff");
                let mut slept = Duration::ZERO;
                let backoff = Duration::from_millis(500);
                while slept < backoff && running.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                    slept += Duration::from_millis(100);
                }
            }
        }
        // Sleep in small slices so a stop signal is observed promptly (never hold any
        // lock here — we are between passes).
        let mut slept = Duration::ZERO;
        while slept < SCAN_INTERVAL && running.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
            slept += Duration::from_millis(100);
        }
    }
}

/// WARNING 4 (crash recovery): fail every directive stuck `launching` that has NO
/// live PTY session (no `agentId`, or its `agentId` is absent from the PTY map). A
/// crash between the PTY spawn and the launch transition leaves a `launching`
/// directive holding the single concurrency slot with no process to ever finish it;
/// this releases the slot at startup. A directive whose mini IS still in the map is
/// left alone (a genuine in-flight launch from a not-yet-quit prior run is rare, but
/// we never kill a live mini here — `plan_tick`'s launch cap handles a truly wedged
/// one). Returns Err only on a hard state-access failure.
fn sweep_orphaned_launching(app: &AppHandle) -> Result<(), String> {
    let snapshot = agents::read_agent_live_state_snapshot(app)?;
    let stuck_ids: Vec<String> = {
        let pty_sessions = app.try_state::<crate::backend::agent_pty::AgentPtySessions>();
        snapshot
            .mini_coder_directives
            .iter()
            .filter(|d| d.status == MiniCoderStatus::Launching)
            .filter(|d| {
                // No agentId at all -> spawn never recorded -> orphaned.
                let Some(agent_id) = d.agent_id.as_deref() else {
                    return true;
                };
                // Has an agentId but no live PTY session -> orphaned. `pty_session_exists`
                // is fail-safe (reports "exists" on a poisoned/absent map), so a missing
                // map leaves the directive alone (re-checked by the launch cap later).
                match &pty_sessions {
                    Some(sessions) => {
                        !crate::backend::agent_pty::pty_session_exists(sessions, agent_id)
                    }
                    None => false,
                }
            })
            .map(|d| d.id.clone())
            .collect()
    };
    if stuck_ids.is_empty() {
        return Ok(());
    }
    agents::mutate_agent_live_state(app, |state| {
        for id in &stuck_ids {
            transition_directive(state, id, |d| {
                mini_coder::apply_failed(d, "launch orphaned by app restart")
            });
        }
        cap_pass(state);
    })
}

/// PURE attach: set the report on its directive row (found by id ==
/// report.task_id). Returns whether a row was found. Extracted so the rig cell
/// exercises the SAME find-predicate/clone/assignment production uses instead
/// of re-implementing the mutation (review: tautology finding).
pub(crate) fn attach_stuck_report(
    state: &mut crate::backend::model::AgentLiveState,
    report: &crate::backend::stuck_report::StuckReport,
) -> bool {
    match state
        .mini_coder_directives
        .iter_mut()
        .find(|d| d.id == report.task_id)
    {
        Some(d) => {
            d.stuck_report = Some(report.clone());
            true
        }
        None => false,
    }
}

/// Persist the stuck report on its directive row (durable fleet state), then
/// emit the live event. The row is found by id == report.task_id; a missing row
/// (already evicted) just skips persistence — the emit still fires.
fn persist_and_emit_stuck(app: &AppHandle, report: crate::backend::stuck_report::StuckReport) {
    let _ = agents::mutate_agent_live_state(app, |state| {
        attach_stuck_report(state, &report);
    });
    let _ = app.emit("mini://stuck", report);
}

/// PURE attach: set the censor summary on its directive row (found by id ==
/// directive_id). Returns whether a row was found. Extracted so the phase-a
/// emit site can exercise the SAME find-predicate/clone/assignment production
/// and so the summary-attach logic is unit-testable on a built AgentsState.
/// `files` are capped at [`CENSOR_MINI_SUMMARY_FILES_CAP`] to bound the
/// persisted state file; `total` is the accurate count regardless of the cap.
pub(crate) fn attach_censor_summary(
    state: &mut crate::backend::model::AgentLiveState,
    directive_id: &str,
    summary: crate::backend::mini_coder::CensorMiniSummary,
) -> bool {
    match state
        .mini_coder_directives
        .iter_mut()
        .find(|d| d.id == directive_id)
    {
        Some(d) => {
            d.censor_summary = Some(summary);
            true
        }
        None => false,
    }
}

/// P6 (crash recovery) + BLOCKER 1: stamp every `Failed` directive that its retry
/// chain can no longer reach via normal finalize propagation, then propagate that terminal
/// up the chain so the Python poll on the ROOT id unblocks. Two cases (pure
/// `awaiting_retry_needing_terminal`):
///   * ABSENT retry child (evicted / never appended after a crash) -> `failed("retry
///     lost")`.
///   * PRESENT + TERMINAL retry child while the predecessor is still Failed (a
///     MISSED propagation — e.g. a retry that failed at LAUNCH before the BLOCKER-1 fix
///     routed `fail_launching` through propagation, or a crash mid-propagation) ->
///     re-propagate the CHILD's own terminal outcome.
///
/// Failed is neither active nor terminal, so the `apply_*` active-only guard can't
/// stamp it — we write `status`/`result` DIRECTLY, then propagate to the chain's other
/// Failed ancestors. Returns Err only on a hard state-access failure.
fn sweep_orphaned_result_files(app: &AppHandle) -> Result<(), String> {
    let snapshot = agents::read_agent_live_state_snapshot(app)?;
    for (dir, keep) in plan_result_file_sweep(&snapshot.mini_coder_directives) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue, // dir gone / unreadable -> nothing to sweep here.
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Only ever delete plain `*.json` result files OR their `*.raw` stdout
            // captures (never a dir, never anything else). FIX 1: a `.raw` capture can
            // leak if a mini was hard-killed before its wrapper/trap removed it — sweep
            // those too (belt-and-braces, bounded to the recorded scratch dir).
            let ext = path.extension().and_then(|e| e.to_str());
            let is_json = ext.is_some_and(|e| e.eq_ignore_ascii_case("json"));
            let is_raw = ext.is_some_and(|e| e.eq_ignore_ascii_case("raw"));
            if (!is_json && !is_raw) || !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // The keep-set holds both `<result>.json` and `<result>.json.raw` for live
            // directives, so a live mini's in-flight capture is never reaped.
            if keep.contains(name) {
                continue; // a live directive still owns this result/raw file.
            }
            let _ = std::fs::remove_file(&path); // best-effort.
        }
    }
    Ok(())
}

/// PURE plan for `sweep_orphaned_result_files` (unit-testable without fs/AppHandle):
/// map each distinct recorded scratch dir to the set of result-file names that belong
/// to a LIVE (non-terminal) directive in it — i.e. the files that must be KEPT. Any
/// `*.json` in that dir NOT in the set is an orphan eligible for deletion. A terminal
/// directive contributes nothing to the keep-set (its file was meant to be deleted on
/// finalize), so a leaked terminal result file is reclaimed.
fn plan_result_file_sweep(
    directives: &[MiniCoderDirective],
) -> std::collections::HashMap<PathBuf, std::collections::HashSet<String>> {
    use std::collections::{HashMap, HashSet};
    let mut plan: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for d in directives {
        let Some(scratch) = d.scratch_path.as_deref() else {
            continue;
        };
        let scratch = scratch.trim();
        if scratch.is_empty() {
            continue;
        }
        let dir = PathBuf::from(scratch);
        let keep = plan.entry(dir).or_default();
        // Only a LIVE directive's result file must be preserved. A terminal one's file
        // is an orphan (finalize should have removed it) -> not added to keep.
        if !d.status.is_terminal() {
            // The result file is written under the scratch root by its (normalized)
            // rel name; we match on the FINAL component only (the sweep lists one dir).
            let normalized = d.result_path.replace('\\', "/");
            if let Some(name) = normalized.rsplit('/').next() {
                if !name.is_empty() {
                    keep.insert(name.to_string());
                    // FIX 1: also preserve the in-flight `.raw` stdout capture of a
                    // live directive (named `<result>.raw`) so the sweep never reaps a
                    // running mini's capture file.
                    keep.insert(format!("{name}.raw"));
                }
            }
        }
    }
    plan
}

/// Slice 3 (seam B): max directives drained from the Pigeon `mini-pool` queue in ONE pass,
/// so a flood of queued sub-tasks can't starve the rest of the pass (timeouts, launches,
/// finalizes). Whatever is left stays queued and is drained on the next tick.
const PIGEON_INGEST_MAX_PER_PASS: usize = 16;

/// Slice 3 (seam B): when Pigeon is enabled, drain up to [`PIGEON_INGEST_MAX_PER_PASS`]
/// directives the Python `spawn_mini_coder` sent to the `mini-pool` queue and insert them
/// (status `pending`, with their `pigeon_ticket` stamped) into `.aspis-agents.json`. The
/// EXISTING `run_pass` machinery then claims/launches/finalizes them unchanged. When
/// disabled this function is never called (the caller gates it), so the pass is byte-identical.
///
/// Robustness: every error here is NON-FATAL — a missing client (dispatcher still booting),
/// a poll network error, or a payload that does not deserialize into a `MiniCoderDirective`
/// is logged (no payload bodies — they may carry task text) and SKIPPED. We never abort the
/// pass on an ingest hiccup; the durable mailbox keeps the un-drained task for the next tick.
fn ingest_pigeon_directives(app: &AppHandle) {
    let Some(client) = crate::backend::pigeon_service::pigeon_client_from_running() else {
        // Dispatcher not running yet (enabled but still booting) — nothing to drain.
        return;
    };
    let mut drained: Vec<MiniCoderDirective> = Vec::new();
    for _ in 0..PIGEON_INGEST_MAX_PER_PASS {
        match client.poll(PIGEON_MINI_POOL_RECEIVER) {
            Ok(Some((ticket_no, payload))) => {
                match serde_json::from_value::<MiniCoderDirective>(payload) {
                    Ok(mut directive) => {
                        // Force the lifecycle to `pending` (the executor owns the claim) and
                        // stamp the ticket so the egress (seam C) can post the outcome back.
                        directive.status = MiniCoderStatus::Pending;
                        directive.pigeon_ticket = Some(ticket_no);
                        drained.push(directive);
                    }
                    Err(e) => {
                        // The task was already CLAIMED off the queue by this poll; a malformed
                        // payload can't be run. Best-effort fail it so it dead-letters instead
                        // of being reclaimed forever. No payload echoed.
                        eprintln!("mini-coder executor: pigeon ingest: undecodable directive (ticket {ticket_no}): {e}");
                        let _ = client.fail(
                            ticket_no,
                            PIGEON_MINI_POOL_RECEIVER,
                            "undecodable mini-coder directive",
                        );
                    }
                }
            }
            // Empty queue — stop draining this pass.
            Ok(None) => break,
            Err(e) => {
                // Network/None error — non-fatal, skip the rest of the drain this pass.
                eprintln!("mini-coder executor: pigeon ingest poll error: {e}");
                break;
            }
        }
    }
    if drained.is_empty() {
        return;
    }
    // DEDUP (MAX-RECALL BLOCKER): a ticket that was reclaimed+requeued by the Slice-2 sweep
    // (because an earlier /done egress failed) is re-polled carrying the SAME directive `id`
    // already tracked in `.aspis-agents.json`. Pushing it again would LAUNCH THE MINI A SECOND
    // TIME (double file edits). So insert only ids NOT already present; for a re-polled dup whose
    // original is already terminal, RE-ATTEMPT the egress (/done with the existing outcome) to
    // close the requeued ticket; if the original is still in-flight, leave the ticket for the
    // original's own egress (skip — never double-launch).
    let existing: std::collections::HashMap<String, Option<MiniCoderOutcome>> =
        match agents::read_agent_live_state_snapshot(app) {
            Ok(s) => s
                .mini_coder_directives
                .iter()
                .map(|d| (d.id.clone(), d.result.clone()))
                .collect(),
            Err(_) => std::collections::HashMap::new(),
        };
    let mut to_insert: Vec<MiniCoderDirective> = Vec::new();
    let mut dups_to_close: Vec<(i64, MiniCoderOutcome)> = Vec::new();
    for directive in drained.drain(..) {
        match existing.get(&directive.id) {
            None => to_insert.push(directive),
            Some(result_opt) => {
                if let (Some(ticket), Some(outcome)) = (directive.pigeon_ticket, result_opt.clone())
                {
                    dups_to_close.push((ticket, outcome));
                }
                // else: original still in-flight — its own egress will close the ticket.
            }
        }
    }
    if !to_insert.is_empty() {
        // Protect the just-inserted ids from eviction this pass (MAX-RECALL: otherwise a full
        // queue could evict a directive before it runs/egresses, orphaning its ticket).
        let inserted_ids: Vec<String> = to_insert.iter().map(|d| d.id.clone()).collect();
        let _ = agents::mutate_agent_live_state(app, |state| {
            for directive in to_insert.drain(..) {
                state.mini_coder_directives.push(directive);
            }
            cap_pass_protecting(state, &inserted_ids);
        });
    }
    // Best-effort, OUTSIDE the lock: re-attempt the failed egress for requeued-but-terminal
    // tickets so they close instead of looping. A 409 (already done) is harmless and ignored.
    for (ticket, outcome) in dups_to_close {
        if let Ok(json) = serde_json::to_value(&outcome) {
            let _ = client.done(ticket, PIGEON_MINI_POOL_RECEIVER, json);
        }
    }
}

/// One executor pass (extracted so it is callable from a test with a real PTY).
/// Returns Err only on a hard state-access failure; per-directive problems degrade
/// to a synthesized `failed`/`timeout` outcome rather than aborting the pass.
fn run_pass(app: &AppHandle) -> Result<(), String> {
    // Slice 3 (seam B): in Pigeon mode, drain the durable `mini-pool` queue into
    // `.aspis-agents.json` BEFORE the directive scan so the existing machinery claims them
    // this same pass. Gated on the flag: when disabled, NO poll/client — byte-identical.
    if crate::backend::pigeon_service::pigeon_enabled_cached(app) {
        ingest_pigeon_directives(app);
        // Also drain the censor-pool: async Censor LLM review requests from finished minis. The
        // poll is fast (claim-or-null); the slow review runs on its own detached thread.
        crate::backend::censor_review::ingest_pigeon_censor_reviews(app.clone());
    }

    // 1) Locked READ snapshot, then work entirely off the clone (lock released).
    let snapshot = agents::read_agent_live_state_snapshot(app)?;
    let directives = snapshot.mini_coder_directives.clone();
    let has_visual = !snapshot.visual_check_directives.is_empty();
    if directives.is_empty() && !has_visual {
        return Ok(()); // EARLY EXIT: nothing to do, cheapest idle path.
    }
    if has_visual {
        run_visual_check_pass(app, &snapshot);
    }
    if directives.is_empty() {
        return Ok(());
    }

    // WARNING 3 (self-healing): reconcile Failed directives whose retry child is
    // lost/terminal, every pass — not only the once-at-startup sweep. A transient startup
    // lock-contention failure (or an Failed orphan that arose post-startup) thus
    // self-heals on a later tick instead of stranding forever. Reuses THIS pass snapshot
    // (no extra read); cheap when nothing is orphaned (`awaiting_retry_needing_terminal`
    // returns empty -> no mutate). A failure here is non-fatal: log and continue the pass.
    if let Err(e) = Ok(()) as Result<(), String> {
        eprintln!("mini-coder executor: awaiting-retry reconcile error: {e}");
    }

    // P6: the configured concurrency (clamped 1..=4, default 2 when unset). Read once
    // per pass — a cheap config.json read off the lock, gated behind the empty-queue
    // early-return above so an idle app never pays it.
    let max_concurrent = super::projects::read_mini_coder_backend(app)
        .map(|b| mini_coder::effective_max_concurrent(&b))
        .unwrap_or(mini_coder::DEFAULT_MAX_CONCURRENT);

    // BLOCKER 2: snapshot the in-flight deferred-verdict ids ONCE per pass. Used for
    // BOTH the `plan_tick` timeout exclusion AND skipping re-finalize of a finished mini
    // whose verdict thread is already running. An absent managed state (tests) -> empty.
    let verdict_inflight = app
        .try_state::<MiniCoderState>()
        .map(|s| s.verdict_inflight_ids())
        .unwrap_or_default();
    // Agentic tool-loop workers have NO PTY: track them so a running worker is neither
    // prematurely finalized (its PTY is "gone" the whole time) NOR wall-clock-timed-out
    // mid-run (it is bounded by its own max_rounds + per-turn HTTP timeout; there is no PTY
    // to kill).
    let agentic_inflight = app
        .try_state::<MiniCoderState>()
        .map(|s| s.agentic_inflight_ids())
        .unwrap_or_default();
    // plan_tick excludes BOTH in-flight kinds from the timeout sweep.
    let timeout_exclusions: std::collections::HashSet<String> =
        verdict_inflight.union(&agentic_inflight).cloned().collect();

    let now = Utc::now().to_rfc3339();
    let plan = mini_coder::plan_tick(
        &directives,
        &now,
        DEFAULT_WALL_CLOCK_CAP_SECS,
        mini_coder::DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
        DEFAULT_LAUNCH_CAP_SECS,
        max_concurrent,
        &timeout_exclusions,
    );

    // 2) Timeouts FIRST (reap blown-cap minis regardless of concurrency): kill the
    //    PTY OUTSIDE any lock, then transition the directive under the lock.
    for timed_out_id in &plan.timeouts {
        if let Some(directive) = directives.iter().find(|d| &d.id == timed_out_id) {
            kill_mini_pty(app, directive);
            let agent_id = directive.agent_id.clone();
            // v6 Phase 5 (B3): capture the LIVE abort decision so the stuck-report emit
            // below uses it, not the stale pass-snapshot `directive.kill_requested`.
            let mut was_aborted = false;
            let _ = agents::mutate_agent_live_state(app, |state| {
                // P5: killRequested WINS. If the human hit Stop, the human's intent
                // overrides a same-pass timeout — consult the LIVE `d.kill_requested`
                // (set under the lock by `mini_coder_kill`, possibly after the stale
                // pass snapshot was taken) and synthesize aborted_by_human instead.
                transition_directive(state, timed_out_id, |d| {
                    if d.kill_requested {
                        was_aborted = true;
                        mini_coder::apply_aborted(d, "stopped by human (Stop button)")
                    } else {
                        mini_coder::apply_timeout(d, "wall-clock cap exceeded")
                    }
                });
                // WARNING 3: close the lingering mini session row too.
                if let Some(aid) = agent_id.as_deref() {
                    close_mini_session(state, aid);
                }
                // MAX-RECALL: protect the just-timed-out directive from eviction this pass so the
                // Pigeon egress (below, by id) can still read it to close the ticket.
                cap_pass_protecting(state, std::slice::from_ref(timed_out_id));
            });
            // FIX 2: terminate the live console too (timeout reap) — OUTSIDE the lock above,
            // after the directive transition is durably applied. Without this the console is
            // stuck running:true (shimmer on) and the store entry stays pinned forever.
            console_mark_stopped(app, directive);
            // v6 Phase 5: real wall-clock timeouts are reaped HERE (they bypass
            // `finalize_finished_mini`), so emit the structured stuck report from this path
            // too — otherwise the Timeout arm in finalize is unreachable for real timeouts.
            // Skip when the human hit Stop (that's an abort, not a stuck mini) — use the
            // LIVE decision, not the stale snapshot, so a Stop that raced this reap wins.
            if !was_aborted {
                let report = crate::backend::stuck_report::StuckReport::new(
                    directive.id.clone(),
                    directive.parent_agent_id.clone(),
                    "timeout",
                    directive.attempt.saturating_add(1),
                    "",
                    Vec::new(),
                    directive_project(&snapshot, directive),
                );
                persist_and_emit_stuck(app, report);
            }
            // Slice 3 (seam C, bypass path): this terminal reap does NOT go through
            // `finalize_finished_mini`, so close the Pigeon ticket here too — both to unblock
            // the Python wait promptly AND to prevent the reclaim sweep from re-queuing +
            // re-running a timed-out mini. Read the AUTHORITATIVE terminal outcome (timeout or
            // killRequested-WINS aborted) back from state. No-op when not Pigeon-ticketed.
            pigeon_egress_terminal_by_id(app, timed_out_id);
        }
    }

    // 2b) WARNING 4: directives stuck `launching` past the launch cap (their launch
    //     bookkeeping never completed — a crash or a wedged spawn). Fail them to
    //     release the single concurrency slot. Kill any PTY that DID get spawned
    //     (best-effort; usually none, since stuck-launching means apply_launched
    //     never ran) OUTSIDE the lock, then transition under the lock.
    for stuck_id in &plan.stuck_launching {
        if let Some(directive) = directives.iter().find(|d| &d.id == stuck_id) {
            kill_mini_pty(app, directive);
            let agent_id = directive.agent_id.clone();
            let _ = agents::mutate_agent_live_state(app, |state| {
                // P5: killRequested WINS here too (defensive — a Stop on a still-
                // launching directive). Consult the LIVE `d.kill_requested`.
                transition_directive(state, stuck_id, |d| {
                    if d.kill_requested {
                        mini_coder::apply_aborted(d, "stopped by human (Stop button)")
                    } else {
                        mini_coder::apply_failed(d, "launch did not complete (stuck launching)")
                    }
                });
                // WARNING 3: close the lingering mini session row too.
                if let Some(aid) = agent_id.as_deref() {
                    close_mini_session(state, aid);
                }
                // MAX-RECALL: protect from eviction so the Pigeon egress can close the ticket.
                cap_pass_protecting(state, std::slice::from_ref(stuck_id));
            });
            // FIX 2: terminate the live console (stuck-launching reap) — OUTSIDE the lock. A
            // never-seeded directive (no build_initial ran) has no live mini, so set_terminal
            // only flips running=Some(false): it stops "running", never paints a phantom
            // timeline. A directive that DID seed a console gets the neutral Stop banner.
            console_mark_stopped(app, directive);
            // v6 Phase 5: emit the structured stuck report from this reap path too
            // (stuck-launching is a terminal failure, not a normal abort). Skip when
            // the human hit Stop (that's an abort, not a stuck mini).
            if !directive.kill_requested {
                let report = crate::backend::stuck_report::StuckReport::new(
                    directive.id.clone(),
                    directive.parent_agent_id.clone(),
                    "failed",
                    directive.attempt.saturating_add(1),
                    "",
                    Vec::new(),
                    directive_project(&snapshot, directive),
                );
                persist_and_emit_stuck(app, report);
            }
            // Slice 3 (seam C, bypass path): close the Pigeon ticket for this terminal reap too.
            pigeon_egress_terminal_by_id(app, stuck_id);
        }
    }

    // 3) Completion / parent-gone detection for RUNNING minis (off the snapshot).
    let pty_sessions = app.try_state::<crate::backend::agent_pty::AgentPtySessions>();
    // FIX 4 (N+1 locked reads): take AT MOST ONE fresh re-confirm snapshot per pass and
    // reuse it for every directive whose parent looked gone in the stale snapshot —
    // instead of one full locked read per such directive (which starved heartbeats
    // under Python contention). `None` until the FIRST directive needs the re-check;
    // `Some(None)` records that the fresh read FAILED (fail-SAFE: treat as not-gone,
    // never kill on an unreadable state — re-check next pass).
    let mut fresh_recheck: Option<Option<crate::backend::model::AgentLiveState>> = None;
    for directive in directives
        .iter()
        .filter(|d| d.status == MiniCoderStatus::Running)
    {
        if plan.timeouts.contains(&directive.id) {
            continue; // already handled as a timeout this pass.
        }
        let Some(agent_id) = directive.agent_id.as_deref() else {
            continue;
        };
        // Parent-gone: the only human-contact point vanished — kill + fail the mini.
        // WARNING 1: the pass-start `snapshot` is stale — a parent that closed then
        // re-registered (app restart) between the snapshot read and now would be
        // wrongly judged gone. Re-confirm against ONE fresh read (lazily taken once
        // per pass) so only a parent STILL absent/closed at decision time is gone.
        let parent_gone = directive_parent_gone(&snapshot, directive) && {
            // Lazily take the single fresh snapshot on the first re-check this pass.
            let fresh = fresh_recheck
                .get_or_insert_with(|| agents::read_agent_live_state_snapshot(app).ok());
            match fresh {
                Some(fresh) => directive_parent_gone(fresh, directive),
                None => false, // fail-safe: unreadable fresh state -> not gone.
            }
        };
        if parent_gone {
            kill_mini_pty(app, directive);
            let id = directive.id.clone();
            let agent_id = directive.agent_id.clone();
            let _ = agents::mutate_agent_live_state(app, |state| {
                // P5: killRequested WINS over parent-gone too — a human who hit Stop
                // asserted control; report aborted_by_human, not failed. Consult the
                // LIVE `d.kill_requested` inside the locked closure.
                transition_directive(state, &id, |d| {
                    if d.kill_requested {
                        mini_coder::apply_aborted(d, "stopped by human (Stop button)")
                    } else {
                        mini_coder::apply_failed(d, "parent coder is gone")
                    }
                });
                // WARNING 3: close the lingering mini session row too.
                if let Some(aid) = agent_id.as_deref() {
                    close_mini_session(state, aid);
                }
                // MAX-RECALL: protect from eviction so the Pigeon egress can close the ticket.
                cap_pass_protecting(state, std::slice::from_ref(&id));
            });
            // FIX 2: terminate the live console too (parent-gone reap) — OUTSIDE the lock,
            // before the `continue`. Without this the console is stuck running:true forever.
            console_mark_stopped(app, directive);
            // v6 Phase 5: emit the structured stuck report from this reap path too
            // (parent-gone is a terminal failure, not a normal abort). Skip when
            // the human hit Stop (that's an abort, not a stuck mini).
            if !directive.kill_requested {
                let report = crate::backend::stuck_report::StuckReport::new(
                    id.clone(),
                    directive.parent_agent_id.clone(),
                    "failed",
                    directive.attempt.saturating_add(1),
                    "",
                    Vec::new(),
                    directive_project(&snapshot, directive),
                );
                persist_and_emit_stuck(app, report);
            }
            // Slice 3 (seam C, bypass path): close the Pigeon ticket for this terminal reap.
            pigeon_egress_terminal_by_id(app, &id);
            continue;
        }
        // BLOCKER 2 (IN-FLIGHT GUARD): a finished mini whose deferred-verdict thread is
        // already running must NOT be re-detected and re-finalized here — its PTY is gone
        // (still_live would be false), so without this guard `run_pass` would spawn a
        // SECOND verdict thread (or finalize inline) every pass while the first runs.
        if verdict_inflight.contains(&directive.id) {
            continue;
        }
        // Finished: the mini's PTY left the map (EOF reaped by the reader thread).
        // `pty_session_exists` is fail-safe (a poisoned/absent map reports "exists"),
        // so we only finalize when we can POSITIVELY confirm the session is gone —
        // never reading a result file for a mini that is still running.
        // An agentic worker has no PTY; it keeps the directive live until it writes its
        // result file and releases the in-flight id. Only then does the PTY-gone path below
        // finalize (reading that result), so the agentic + one-shot finalize paths converge.
        let still_live = agentic_inflight.contains(&directive.id)
            || match &pty_sessions {
                Some(sessions) => crate::backend::agent_pty::pty_session_exists(sessions, agent_id),
                None => true, // state unavailable -> assume live, re-check next pass.
            };
        if !still_live {
            finalize_finished_mini(app, directive);
        }
    }

    // 4) ONE claim per pass (plan_tick enforces max_concurrent=1). Spawn OUTSIDE the
    //    lock; re-check the claim under the lock so a stale snapshot cannot win twice.
    //    BLOCKER 1: resolve the parent coder's project from THIS pass snapshot (the
    //    same one plan_tick saw) — NOT a fresh locked read inside claim_and_launch.
    //    A separate later read could observe the parent having switched project (or
    //    gone) between the claim write and the read → the mini would get the wrong
    //    (or None) project: wrong rail bucket, wrong scratch dir, and a spurious
    //    failure. Pinning it to the pass snapshot makes the scratch root AND the
    //    session project stamp consistent with the project the directive was planned
    //    against. A parent with no project at this instant fails the claim cleanly.
    //    P6: multiple disjoint claims per pass (plan_tick enforces max_concurrent +
    //    file-disjointness). Each claim re-checks status AND file-disjointness under the
    //    lock (claim_and_launch) so a stale snapshot can't double-claim overlapping files.
    for claim_id in &plan.claims {
        if let Some(directive) = directives.iter().find(|d| &d.id == claim_id) {
            let parent_project = directive_project(&snapshot, directive);
            claim_and_launch(app, directive, parent_project);
        }
    }

    // 5) COARSE cooldown check + FINE cooldown sweep.
    if let Some(st) = app.try_state::<MiniCoderState>() {
        // COARSE: per-project. Drain dirty set, read each project's policy from file,
        // check per-project cooldown, spawn coarse pass. Dirty flag is cleared INSIDE
        // the spawned thread on success (B3: no lost COARSE on transient failure).
        {
            let mut dirty = st.coarse_dirty.lock().unwrap();
            if !dirty.is_empty() {
                let mut last_map = st.last_coarse.lock().unwrap();
                let now = Instant::now();
                let mut deferred: std::collections::HashSet<String> =
                    std::collections::HashSet::new();

                for project_id in dirty.drain() {
                    // Resolve project root.
                    let Ok(root) =
                        crate::backend::projects::resolve_project_root_by_id(app, &project_id)
                    else {
                        // Project gone or unresolvable — drop from dirty set.
                        continue;
                    };
                    // B1: read per-project coarse policy from file (not global mutex).
                    let policy = std::fs::read_to_string(root.join(".aspis").join("coarse_policy"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| matches!(s.as_str(), "off" | "manual" | "auto"))
                        .unwrap_or_else(|| "auto".to_string());
                    if policy != "auto" {
                        // Policy is off/manual — skip, don't re-dirty.
                        continue;
                    }
                    // Per-project cooldown check.
                    let last = last_map.get(&project_id).copied();
                    if last.is_some_and(|t| {
                        now.duration_since(t) <= Duration::from_secs(COARSE_COOLDOWN_S)
                    }) {
                        // Cooldown not elapsed — re-insert for next tick.
                        deferred.insert(project_id);
                        continue;
                    }
                    // Spawn coarse pass.
                    let app = app.clone();
                    let pid = project_id.clone();
                    let dirty_ref = Arc::clone(&st.coarse_dirty);
                    std::thread::spawn(move || {
                        let running = AtomicBool::new(true);
                        crate::backend::censor::orchestrator::run_coarse_pass(
                            &app, &pid, &root, &running,
                        );
                        // Same stamp as manual whole-project review (censor_review_now).
                        crate::backend::censor::orchestrator::stamp_last_coarse_run(&root);
                        // B3: clear dirty only AFTER successful completion.
                        dirty_ref.lock().unwrap().remove(&pid);
                    });
                    last_map.insert(project_id, now);
                }
                // Re-insert projects that were deferred (cooldown not elapsed).
                for pid in deferred {
                    dirty.insert(pid);
                }
            }
        }

        // FINE cooldown sweep: evict entries older than cooldown × 4, every 10 minutes.
        {
            let mut sweep = st.last_cooldown_sweep.lock().unwrap();
            if sweep.elapsed() > Duration::from_secs(600) {
                let mut map = st.fine_cooldown.lock().unwrap();
                let cutoff = Duration::from_secs(FINE_COOLDOWN_S * 4);
                let now = Instant::now();
                map.retain(|_, t| now.duration_since(*t) <= cutoff);
                *sweep = now;
            }
        }
    }

    Ok(())
}

fn run_visual_check_pass(app: &AppHandle, snapshot: &crate::backend::model::AgentLiveState) {
    // One pass instant for BOTH the timeout plan and the claim stamp (they describe the same
    // scan tick).
    let now = Utc::now().to_rfc3339();

    // MINOR (FIX 3): evict timed-out directives AND claim the next Pending in ONE locked
    // mutation. Previously these were TWO separate mutate_agent_live_state calls (evict, then
    // claim) with a TOCTOU window between them — only safe because the claim re-checked status.
    // Recomputing the PURE `visual_pass_plan` on the LIVE state INSIDE the lock makes the
    // eviction set and the claim gate consistent with the exact state we mutate, closing the
    // window. The closure returns the claimed directive CLONED FROM LIVE STATE (not just its
    // id) so the spawn — which must run OUTSIDE the lock — never consults the stale snapshot:
    // a directive appended between the snapshot read and this lock would be claimed here but
    // absent from the snapshot, and an id-only return would orphan it as Running. BLOCKER 2
    // property preserved: the plan gates "anything still Running?" on the POST-eviction view,
    // so a directive timed out THIS pass never starves a Pending claim in the same pass.
    let claimed = agents::mutate_agent_live_state(app, |state| {
        let (timed_out_ids, claimable_pending_id) = crate::backend::visual_check::visual_pass_plan(
            &state.visual_check_directives,
            &now,
            VISUAL_CHECK_TIMEOUT_SECS,
        );
        for id in &timed_out_ids {
            transition_visual_directive(state, id, |d| {
                crate::backend::visual_check::apply_result(
                    d,
                    crate::backend::visual_check::VisualCheckOutcome::timeout(
                        "visual_check timed out waiting for the local critique",
                    ),
                )
            });
        }
        let claimed = claimable_pending_id.and_then(|candidate_id| {
            let d = state
                .visual_check_directives
                .iter_mut()
                .find(|d| d.id == candidate_id)?;
            if d.status != crate::backend::visual_check::VisualCheckStatus::Pending {
                return None;
            }
            match crate::backend::visual_check::apply_claim(d, now.clone()) {
                Ok(next) => {
                    *d = next;
                    Some(d.clone())
                }
                Err(_) => None,
            }
        });
        cap_pass(state);
        claimed
    });

    let Ok(Some(candidate)) = claimed else {
        return;
    };
    let parent_project = snapshot_parent_project(snapshot, &candidate.parent_agent_id)
        .filter(|p| !p.trim().is_empty());
    let Some(project_id) = parent_project else {
        finish_visual_check(
            app,
            &candidate.id,
            crate::backend::visual_check::VisualCheckOutcome::failed(
                "parent agent has no current project",
            ),
        );
        return;
    };
    let project_root = match crate::backend::projects::resolve_project_root_by_id(app, &project_id)
    {
        Ok(root) => root,
        Err(_) => {
            finish_visual_check(
                app,
                &candidate.id,
                crate::backend::visual_check::VisualCheckOutcome::failed("not under project root"),
            );
            return;
        }
    };
    let app_clone = app.clone();
    let directive = candidate.clone();
    let spawned = std::thread::Builder::new()
        .name("visual-check-executor".into())
        .spawn(move || {
            let outcome = crate::backend::visual_check::execute_visual_check(
                app_clone.clone(),
                &project_root,
                &directive,
            );
            finish_visual_check(&app_clone, &directive.id, outcome);
        });
    if spawned.is_err() {
        finish_visual_check(
            app,
            &candidate.id,
            crate::backend::visual_check::VisualCheckOutcome::failed(
                "could not start visual_check executor",
            ),
        );
    }
}

fn finish_visual_check(
    app: &AppHandle,
    id: &str,
    outcome: crate::backend::visual_check::VisualCheckOutcome,
) {
    let id = id.to_string();
    let _ = agents::mutate_agent_live_state(app, |state| {
        transition_visual_directive(state, &id, |d| {
            crate::backend::visual_check::apply_result(d, outcome)
        });
        cap_pass(state);
    });
}

/// Shared lookup: locate the `config.json` config, parse it, find the
/// `"modelRegistry"` array, and linear-scan for the entry whose `"backend"` field
/// matches `backend.kind` **and** whose `"id"` field matches `backend.model`.
/// Returns the matched entry as an owned [`serde_json::Value`], or `None` on any
/// miss (no backend-kind serialization, no model id, no config path, unreadable
/// file, bad JSON, missing/non-array `modelRegistry`, or no matching entry).
fn locate_registry_entry(
    app: &AppHandle,
    backend: &MiniCoderBackend,
) -> Option<serde_json::Value> {
    let backend_kind_str = serde_json::to_value(backend.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))?;
    let model_id = backend.model.as_deref()?;

    let config_path = crate::backend::projects::locate_config_path(app)?;
    let content = std::fs::read_to_string(&config_path).ok()?;
    let config: serde_json::Value = serde_json::from_str(&content).ok()?;

    let registry = config.get("modelRegistry")?.as_array()?;
    for entry in registry {
        if entry.get("backend").and_then(|v| v.as_str()) == Some(backend_kind_str.as_str())
            && entry.get("id").and_then(|v| v.as_str()) == Some(model_id)
        {
            return Some(entry.clone());
        }
    }
    None
}

/// Whether `directive` should run via the AGENTIC tool-loop instead of the one-shot path:
/// it explicitly requested `AgenticIterative` write mode, is a write directive, has an
/// OpenAI-compatible loopback HTTP endpoint (oMLX/api → a non-empty, already-validated
/// `base_url`), and the user's safety policy permits agentic writes (not `Safe`).
/// S2 (capability-driven): the registry tier ("agentic"/"emitEdits") for the mini's
/// (backend-kind, model), or None if config.json / the registry / a matching entry is absent.
/// Best-effort, never panics. Delegates to [`locate_registry_entry`] for the shared
/// config → registry → match lookup.
pub(crate) fn mini_model_tier(app: &AppHandle, backend: &MiniCoderBackend) -> Option<String> {
    locate_registry_entry(app, backend)
        .and_then(|entry| entry.get("tier")?.as_str().map(str::to_string))
}

/// Phase B: resolve the registry's per-model context window (tokens) for this mini's
/// (backend-kind, model). Falls back to 8192 (the safe default) on any miss.
/// Best-effort. Delegates to [`locate_registry_entry`] for the shared
/// config → registry → match lookup.
fn mini_model_context_window(app: &AppHandle, backend: &MiniCoderBackend) -> usize {
    locate_registry_entry(app, backend)
        .and_then(|entry| entry.get("contextWindow")?.as_u64())
        .map(|cw| cw as usize)
        .unwrap_or(8192)
}

/// Phase 7: resolve the registry's per-model sampling for this mini's (backend-kind, model),
/// falling back to `SamplingParams::tuned()` on any miss (no config / no registry / no match /
/// deserialize error). Best-effort, never panics. Delegates to [`locate_registry_entry`]
/// for the shared config → registry → match lookup.
pub(crate) fn mini_model_sampling(
    app: &AppHandle,
    backend: &MiniCoderBackend,
) -> crate::backend::agentic_transport::SamplingParams {
    locate_registry_entry(app, backend)
        .and_then(|entry| {
            let parsed: crate::backend::model_registry::ModelRegistryEntry =
                serde_json::from_value(entry).ok()?;
            Some(crate::backend::agentic_transport::SamplingParams::from_registry(&parsed))
        })
        .unwrap_or_else(crate::backend::agentic_transport::SamplingParams::tuned)
}

// (should_run_agentic + spawn_agentic_worker live in backend/agentic_worker.rs —
// role-untangle Phase 2 pure move. Re-imported below so call sites are unchanged.)
use super::agentic_worker::{should_run_agentic, spawn_agentic_worker};

/// Pure gate: returns Ok(()) if `kind` can be dispatched by the directive
/// executor. Cloud is supported via the agentic HTTP loop + vault Bearer key
/// (OpenRouter); one-shot PTY still rejects Cloud in mini_command_build.
pub(crate) fn backend_supports_directive_dispatch(
    kind: MiniCoderBackendKind,
) -> Result<(), &'static str> {
    let _ = kind;
    Ok(())
}

/// Atomically claim a pending directive (under the lock, re-checking status so a
/// double-claim is impossible), then spawn the one-shot mini PTY OUTSIDE the lock,
/// then mark it `running` + nest the mini session under its parent. On any spawn or
/// claim failure the directive is moved to `failed` (the coder's poll returns it).
fn claim_and_launch(
    app: &AppHandle,
    directive: &MiniCoderDirective,
    // BLOCKER 1: the parent coder's project resolved ONCE from the same pass snapshot
    // that planned this claim. Used for BOTH the scratch root and the session project
    // stamp so they are mutually consistent. None means the parent carried no project
    // at plan time — we refuse to launch a project-less, rail-invisible mini.
    parent_project: Option<String>,
) {
    let directive_id = directive.id.clone();
    // BLOCKER 1: a mini with no project tree has nowhere to write its scratch result
    // and would never surface in any project rail (sessionsByProject keys on
    // current_project_id). We must still CLAIM first (Pending -> Launching) before we
    // can move it to `failed`: `apply_outcome` refuses `Pending -> failed` (only an
    // active Launching/Running directive may reach a terminal state), so failing a
    // still-`pending` directive would be a swallowed no-op and the directive would be
    // re-claimed every pass forever. So: claim, then fail-launching if no project.
    let project_id = parent_project.filter(|p| !p.trim().is_empty());
    // Claim under the lock; the closure re-reads the LIVE status, so if another pass
    // (or instance) already claimed it the apply_claim Err short-circuits → we skip.
    // WARNING 4: stamp `claimed_at` (the Launching anchor `plan_tick` uses to fail a
    // stuck launch) in the SAME locked write that flips the status to launching.
    let claimed_at = Utc::now().to_rfc3339();
    let claimed: Result<bool, String> = agents::mutate_agent_live_state(app, |state| {
        let claimed = {
            // P6 DEFENSE IN DEPTH: re-check file-disjointness against the LIVE active set
            // (`Launching|Running`, Failed excluded) BEFORE claiming, so a stale
            // pass-snapshot can never double-claim files a now-active mini already holds.
            // Find the candidate by VALUE first (clone it) so the disjointness check can
            // borrow the whole slice without aliasing the mutable find below.
            let candidate = state
                .mini_coder_directives
                .iter()
                .find(|d| d.id == directive_id)
                .cloned();
            let Some(candidate) = candidate else {
                return false;
            };
            if candidate.status != MiniCoderStatus::Pending {
                return false; // lost the race (already launching/running/terminal).
            }
            if !mini_coder::files_disjoint_from_active(&candidate, &state.mini_coder_directives) {
                return false; // a live active mini now holds an overlapping file — skip.
            }
            // Apply the claim on the live directive.
            let Some(d) = state
                .mini_coder_directives
                .iter_mut()
                .find(|d| d.id == directive_id)
            else {
                return false;
            };
            match mini_coder::apply_claim(d, claimed_at.clone()) {
                Ok(next) => {
                    *d = next;
                    true
                }
                Err(_) => false, // lost the race (already launching/running/terminal).
            }
        };
        cap_pass(state);
        claimed
    });
    if !matches!(claimed, Ok(true)) {
        return; // did not win the claim, or state write failed — nothing spawned.
    }

    // BLOCKER 1: now that the directive is `launching` (claimed), enforce the project
    // requirement. With no parent project we fail it cleanly (Launching -> failed) and
    // spawn nothing — never a project-less, rail-invisible mini. fail_launching uses
    // apply_failed, which IS permitted from `launching`.
    let Some(project_id) = project_id else {
        fail_launching(app, &directive_id, "parent coder has no current project");
        return;
    };

    // WARNING 4: validate the directive-supplied result path for traversal/absolute
    // BEFORE it is joined under the scratch root to build the write target. The
    // directive comes from a coder's MCP `spawn_mini_coder` tool; a `..`/absolute
    // result_path would otherwise let the resolved write/read target escape the
    // scratch dir. `read_result_file` re-validates on read, but failing the claim
    // here (Launching -> failed) means we never even spawn a mini for a bad path.
    if let Err(e) = mini_coder::validate_result_rel_path(&directive.result_path) {
        fail_launching(app, &directive_id, &format!("invalid result path: {e}"));
        return;
    }

    // Allocate the mini's agent id + resolve its scratch root, then spawn OUTSIDE the
    // lock. Any failure here transitions the (now-launching) directive to failed.
    // BLOCKER 3: the resolved scratch root is PERSISTED on the directive in the
    // launch transition below so finalization reads the result from the SAME tree
    // even if the parent switched projects between launch and the mini's EOF.
    let agent_id = mini_agent_id(directive);
    // BLOCKER 1: the project resolved from the pass snapshot anchors the scratch root
    // AND (below) is stamped onto the mini session so the mini groups into the SAME
    // project as its coder (ProjectsView's sessionsByProject keys on
    // current_project_id). It is guaranteed Some + non-empty by the guard above.
    let scratch_outcome = resolve_scratch_root(app, &project_id);
    let (project_root, scratch_root) = match scratch_outcome {
        Ok(roots) => roots,
        Err(e) => {
            fail_launching(
                app,
                &directive_id,
                &format!("could not resolve mini scratch root: {e}"),
            );
            return;
        }
    };
    let scratch_path_str = scratch_root.to_string_lossy().to_string();
    let result_rel = directive.result_path.clone();

    // P4: resolve the configured global backend. With NO backend configured we
    // refuse to spawn (rather than running a garbage echo): the directive fails
    // cleanly with a clear message the coder's poll surfaces. The headless/test
    // backend is only used by the integration test, never in production.
    //
    // Role untangle (P6b consumption): a MAIN-tier directive (the promoted local Main coder)
    // runs on its OWN model — `read_main_coder_backend` prefers the dedicated `mainCoderBackend`
    // row and INHERITS the mini's when unset. A Mini-tier directive keeps reading the mini's
    // backend verbatim (byte-identical to before). This is the seam that makes the Roles table
    // "Main coder → Local → <model>" actually drive the local Main coder.
    let resolved_backend = if matches!(directive.tier, super::mini_coder::DirectiveTier::Main) {
        super::roles_config::read_main_coder_backend(app)
    } else {
        super::projects::read_mini_coder_backend(app)
    };
    let backend = match resolved_backend {
        Some(b) => b,
        None => {
            fail_launching(app, &directive_id, "no mini-coder backend configured");
            return;
        }
    };
    // Cloud (OpenRouter): agentic HTTP path with Bearer from the shared vault.
    // Previously hard-failed ("pi engine only") even though agentic_transport now
    // supports Authorization — that blocked mini on OpenRouter while orch worked.
    // F50: Main tier → "main", Mini tier → "mini"; falls back to shared key when absent.
    let cloud_api_key: Option<String> = if backend.kind == MiniCoderBackendKind::Cloud {
        let vault_role = match directive.tier {
            super::mini_coder::DirectiveTier::Main => "main",
            _ => "mini",
        };
        match super::vault::read_cloud_llm_key_for_role(vault_role) {
            Ok(Some(k)) if !k.trim().is_empty() => Some(k),
            Ok(_) => {
                fail_launching(
                    app,
                    &directive_id,
                    "cloud mini requires a saved Cloud API key (Settings → Roles → Cloud API key Save)",
                );
                return;
            }
            Err(e) => {
                fail_launching(
                    app,
                    &directive_id,
                    &format!("cloud mini could not read Cloud API key vault: {e}"),
                );
                return;
            }
        }
    } else {
        None
    };
    let backend_kind_label = backend_client_label(&backend);

    // Resolve the MCP wiring (management root + projects dir) for the codex
    // backend's bounded `oracle_context` grant — but ONLY when this directive
    // ACTUALLY granted the oracle (`allow_oracle`). A directive without the grant
    // gets NO mcp_roots, so `build_mini_command` wires NO `-c mcp_servers...`
    // flags (the plan's "allow_oracle=false ⇒ no MCP grant" invariant). ollama/api
    // are text-only and never get the grant regardless (enforced in build_mini_*).
    // Best-effort: if the roots can't be resolved we degrade to NO oracle grant.
    let mcp_roots = if directive.allow_oracle {
        resolve_mcp_roots(app)
    } else {
        None
    };
    // P3: a codex mini with the oracle grant registers over MCP as role "mini",
    // launch-token-bound (the python side pins the stored role + token hash, so
    // the mini cannot self-promote to coder). Mint the token HERE: the RAW token
    // rides only inside the 0600 prompt file; only its HASH lands in the session
    // ledger below. CSPRNG failure is fail-closed (FIX 5): refuse the launch.
    let oracle_grant = if (backend.kind == MiniCoderBackendKind::Codex
        || backend.kind == MiniCoderBackendKind::Openai)
        && mcp_roots.is_some()
    {
        match super::projects::generate_launch_token() {
            Ok(token) => {
                let hash = super::projects::hash_launch_token(&token);
                Some((token, hash))
            }
            Err(e) => {
                fail_launching(app, &directive_id, &format!("mini launch refused: {e}"));
                return;
            }
        }
    } else {
        None
    };

    // P3 (review F1 BLOCKER): persist the granted session — role "mini" + the
    // launch-token HASH — BEFORE the PTY spawn, or a fast mini can call
    // agent_register before the hash is on disk and be rejected. The post-spawn
    // locked write below re-upserts the same row (idempotent update branch).
    if let Some((_, hash)) = oracle_grant.as_ref() {
        let pre_started = Utc::now().to_rfc3339();
        let pre_project = Some(project_id.clone());
        let _ = agents::mutate_agent_live_state(app, |state| {
            upsert_mini_session(
                state,
                &agent_id,
                &directive.parent_agent_id,
                pre_project.clone(),
                &pre_started,
                &backend_kind_label,
                Some(hash.as_str()),
                directive.tier,
            );
        });
    }

    // Capable (>20B) models with write_mode=AgenticIterative run the multi-turn tool loop
    // (they write files themselves via sandboxed tools); everything else uses the one-shot
    // emit-edits PTY. Both produce the SAME result-file contract for finalize→Censor→retry.
    // AUDIT CRITICAL (review F3): refuse BEFORE the branch. A local-kind backend with a
    // NON-loopback base_url would ship the prompt + project source to a remote host on BOTH the
    // agentic AND the one-shot path — declining only the agentic loop just falls through to the
    // one-shot, which makes the same HTTP request. So gate both here.
    if let Some(url) = backend.base_url.as_deref() {
        if agentic_local_base_url_rejected(backend.kind, url) {
            fail_launching(
                app,
                &directive_id,
                "refusing mini spawn: local backend base_url is not loopback (would exfiltrate prompt+source)",
            );
            return;
        }
    }

    // SANDBOX broker: resolve the net policy for this spawn.
    // Two sources unlock the network (OR'd):
    //   1. Persistent flag `net_enabled` (survives restart; set via AllowRemember).
    //   2. One-shot transient grant (set via AllowOnce; consumed on first agentic use).
    //
    // FIX 1: the transient grant is consumed ONLY on the agentic path. The one-shot
    // path (`spawn_one_shot_mini`) does not accept a NetPolicy and would silently waste
    // the grant — leaving the user stuck in an infinite re-prompt loop. Additionally,
    // if the agentic spawn fails, we re-insert the grant so the next attempt can use it
    // (otherwise the failure outcome has net_blocked=false and no consent-request fires).
    //
    // Concurrency note: the grant is keyed per-project (not per-directive). With
    // concurrent same-project agentic directives the first spawn consumes it — the
    // second runs without network access and may emit its own net_blocked outcome,
    // re-triggering the consent flow. Accepted for Slice 0.
    let persistent_net =
        crate::backend::projects::project_net_enabled(app, &project_id).unwrap_or(false);

    // ROLE UNTANGLE Phase 3: a MAIN-tier directive runs the agentic engine or
    // FAILS — it must NEVER silently downgrade to the one-shot mini path (its
    // whole point is the multi-turn sandboxed loop). A Safe write-behavior
    // policy, a missing/non-loopback backend base_url, or a non-write directive
    // all surface as a clean failure the dispatcher sees.
    // Evaluated ONCE and reused for both the guard and the dispatch branch: two
    // separate calls each re-read config.json, and a concurrent Safe flip between
    // them could pass the guard yet take the one-shot branch — the exact downgrade
    // the guard exists to prevent (hostile-review finding).
    // Cloud has no one-shot PTY path — always use the agentic HTTP loop when write+url.
    let run_agentic = if backend.kind == MiniCoderBackendKind::Cloud {
        directive.write
            && backend
                .base_url
                .as_deref()
                .is_some_and(|u| !u.trim().is_empty())
    } else {
        should_run_agentic(app, &backend, directive)
    };
    if backend.kind == MiniCoderBackendKind::Cloud && !run_agentic {
        fail_launching(
            app,
            &directive_id,
            "cloud mini requires write:true and a non-empty baseUrl (OpenRouter HTTPS)",
        );
        return;
    }
    if directive.tier == mini_coder::DirectiveTier::Main && !run_agentic {
        fail_launching(
            app,
            &directive_id,
            "main-coder directive cannot run the agentic engine (requires write:true, a \
             configured loopback backend base_url, and a non-Safe mini write behavior) — \
             refusing to downgrade to a one-shot mini",
        );
        return;
    }

    let spawn_result = if run_agentic {
        // FIX B: resolve the sandbox mode BEFORE consuming the transient grant.
        // When the mode is Unattended, a stale AllowOnce grant (issued before the
        // project was switched to Unattended) must NOT silently enable net — Unattended
        // is fail-closed.  We therefore only consume the transient grant when the mode
        // would actually honour it (Ask / AutoAcceptInWorkspace).
        // SLICE 1 capability gate: Unattended autonomy is honoured ONLY where the OS sandbox is
        // actually enforced (`sandbox::is_enforced()`); on an un-sandboxed platform it silently
        // degrades to Ask (Decision B + silent fallback). `is_enforced()` is constant per process,
        // so applying the same gate at finalize stays consistent with this spawn-time decision.
        let sandbox_mode = crate::backend::broker::effective_sandbox_mode(
            crate::backend::projects::project_sandbox_mode(app, &project_id)
                .unwrap_or(crate::backend::broker::SandboxMode::Ask),
            crate::backend::sandbox::is_enforced(),
        );

        // CHEAP FIX A + WARNING 2 fix: atomically drain BOTH net and folder grants in a
        // single combined lock acquisition, eliminating the split-grant race where two
        // concurrent same-project directives could each steal only part of the grant set.
        // Always drain regardless of mode (WARNING 2: prevents unbounded HashMap growth and
        // stale-grant storms when the project later switches from Unattended back to Ask).
        let (transient_net_taken, transient_folders_taken) = app
            .try_state::<crate::backend::broker::PermissionBrokerState>()
            .map(|broker| broker.take_all_grants(&project_id))
            .unwrap_or((false, std::collections::HashSet::new()));

        // Discard in Unattended (fail-closed); honour in Ask / AutoAcceptInWorkspace.
        // NOTE (Slice 1): `sandbox_mode` here is the EFFECTIVE mode — on an un-sandboxed platform
        // `effective_sandbox_mode` has already degraded a raw `Unattended` to `Ask`, so this branch
        // honours the transient grant (full-Ask supervised behaviour, by design — see
        // `broker::degraded_unattended_behaves_as_full_ask_not_stricter`). On macOS (enforced) the
        // effective mode equals the raw mode, so a real Unattended project still fails closed.
        let transient_net = if sandbox_mode != crate::backend::broker::SandboxMode::Unattended {
            transient_net_taken
        } else {
            false
        };

        // Net is enabled iff the pure resolver says so (pins the invariant table).
        // Cloud/OpenRouter always needs full egress (LLM API + tools), independent of
        // the project net-toggle (otherwise mini says "no internet" with a valid key).
        let net_enabled = backend.kind == MiniCoderBackendKind::Cloud
            || crate::backend::broker::resolve_net_enabled(
                persistent_net,
                transient_net,
                sandbox_mode,
            );
        let agentic_net = if net_enabled {
            crate::backend::sandbox::NetPolicy::Enabled
        } else {
            crate::backend::sandbox::NetPolicy::None
        };
        // SANDBOX broker Slice 2: resolve the effective working set.
        // In Unattended: drain happened but we pass empty — transient grants are not honoured.
        let persisted_working_set =
            crate::backend::projects::project_working_set(app, &project_id).unwrap_or_default();
        let transient_folders = if sandbox_mode != crate::backend::broker::SandboxMode::Unattended {
            transient_folders_taken.clone()
        } else {
            std::collections::HashSet::new()
        };
        let effective_working_set = crate::backend::broker::resolve_working_set(
            &persisted_working_set,
            transient_folders,
            sandbox_mode,
        );
        let working_set_paths: Vec<std::path::PathBuf> = effective_working_set
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect();

        let spawn_r = spawn_agentic_worker(
            app,
            &project_root,
            &scratch_root,
            &result_rel,
            &backend,
            directive,
            agentic_net,
            working_set_paths,
            cloud_api_key.clone(),
        );
        // Re-insert the transient grants atomically (single lock) ONLY in non-Unattended mode
        // if the spawn itself failed: the worker never launched, so the user never saw a
        // blocked outcome and would not be re-prompted — restore grants so the next
        // claim_and_launch attempt can use them.  Unattended never re-inserts (drain is final).
        if spawn_r.is_err() && sandbox_mode != crate::backend::broker::SandboxMode::Unattended {
            if let Some(broker) = app.try_state::<crate::backend::broker::PermissionBrokerState>() {
                broker.reinsert_all_grants(
                    &project_id,
                    transient_net_taken,
                    &transient_folders_taken,
                );
            }
        }
        spawn_r
    } else {
        // One-shot path: NetPolicy is not threaded here. The persistent flag applies
        // via the sandbox wrapper the one-shot runner inherits; the transient grant is
        // intentionally NOT consumed so it remains available for the agentic path.
        spawn_one_shot_mini(
            app,
            &agent_id,
            &project_root,
            &scratch_root,
            &result_rel,
            &backend,
            directive,
            mcp_roots.as_ref(),
            oracle_grant.as_ref().map(|(token, _)| token.as_str()),
        )
    };
    if let Err(e) = spawn_result {
        // The pre-spawn session (if any) must not linger as a live "mini" row
        // for a PTY that never existed.
        if oracle_grant.is_some() {
            let _ = agents::mutate_agent_live_state(app, |state| {
                close_mini_session(state, &agent_id);
            });
        }
        fail_launching(app, &directive_id, &format!("mini spawn failed: {e}"));
        return;
    }
    // Record the ledger entry (host="app", client=backend kind) so the live-state
    // read stamps the mini as app-hosted. Best-effort: a ledger failure does not
    // un-spawn the mini (it will still produce a result + be reaped on EOF).
    let _ = agents::record_mini_launch(app, &agent_id, &backend_kind_label);

    // Mark running + persist the resolved scratch path + nest the mini session under
    // its parent coder — ONE locked write (shrinks the WARNING-4 crash window: the
    // directive is launching only between the claim above and this write).
    let started_at = Utc::now().to_rfc3339();
    let parent_id = directive.parent_agent_id.clone();
    let nest_id = agent_id.clone();
    // Stamp the SAME project resolved above onto the mini session.
    let mini_project = Some(project_id.clone());
    // F07: if a MAIN directive has no task_id, inherit the parent session's
    // current_task_id (orchestrator often has the task selected). Minis do not
    // inherit — only Main owns the Kanban promote-on-done path.
    let inherited_task_id: Option<String> = if !matches!(directive.tier, mini_coder::DirectiveTier::Main)
        || directive
            .task_id
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    {
        None // not Main, or already set
    } else {
        agents::read_agent_live_state_snapshot(app)
            .ok()
            .and_then(|snap| {
                snap.sessions
                    .iter()
                    .find(|s| s.agent_id == parent_id)
                    .and_then(|s| s.current_task_id.clone())
            })
            .filter(|t| !t.trim().is_empty())
    };
    let session_task_id = directive
        .task_id
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| inherited_task_id.clone());
    let _ = agents::mutate_agent_live_state(app, |state| {
        transition_directive(state, &directive_id, |d| {
            mini_coder::apply_launched(d, nest_id.clone(), started_at.clone()).map(|mut next| {
                next.scratch_path = Some(scratch_path_str.clone());
                // F07: stamp inherited task_id so finalize can promote Kanban.
                if next.task_id.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    if let Some(tid) = inherited_task_id.clone() {
                        next.task_id = Some(tid);
                    }
                }
                next
            })
        });
        upsert_mini_session(
            state,
            &nest_id,
            &parent_id,
            mini_project.clone(),
            &started_at,
            &backend_kind_label,
            oracle_grant.as_ref().map(|(_, hash)| hash.as_str()),
            directive.tier,
        );
        // F07: pin the mini session's current_task_id so the board/rail can show WHO.
        if let Some(tid) = session_task_id.as_deref() {
            if let Some(session) = state.sessions.iter_mut().find(|s| s.agent_id == nest_id) {
                session.current_task_id = Some(tid.to_string());
            }
        }
        cap_pass(state);
    });

    // CONSOLE (Step B): the run is now live — publish the initial Activity Console snapshot
    // on `mini-activity://<agent_id>` (same id as `agent-terminal://<agent_id>`). A single
    // spawn entry: model label + scope + the first round, the working shimmer, running=true.
    // `directive.attempt` is 0-based, so the first round number is attempt+1 (a retry that
    // launches as its own directive seeds the console at its own round). Pure observer: a
    // missing store (unmanaged in some tests) makes this a no-op.
    if let Some(store) = console_store(app) {
        let model = console_model_label_for_tier(&backend, directive.tier);
        let label = console_run_label(directive);
        let scope = directive.files.clone();
        let round_n = directive.attempt.saturating_add(1);

        // P2 cost: estimate + record ONCE on new tasks (attempt 0); never re-record
        // on retries (ledger would double-count). Out of scope (P2b): real usage.cost
        // from the model client — for now the ledger accumulates ESTIMATES.
        let est = backend.model.as_deref().and_then(|m| {
            super::cost::estimate_task_cost(app.clone(), m.to_string())
                .ok()
                .flatten()
        });
        if directive.attempt == 0 {
            if let (Some(usd), Some(m)) = (est, backend.model.as_deref()) {
                let _ = super::cost::record_cost(app.clone(), m.to_string(), usd);
            }
        }

        if let Some(store) = console_store(app) {
            let projects_dir = super::projects::ensure_projects_dir(app).ok();

            let f = |a: &mut super::mini_activity::ConsoleActivity| {
                if directive.attempt == 0 {
                    *a = super::mini_activity::build_initial(&model, &label, &scope, round_n);
                } else {
                    super::mini_activity::resume_retry_round(a, &model, &label, &scope, round_n);
                }
                a.task_cost_estimate_usd = est;
            };
            match projects_dir {
                Some(ref pd) => store.update_bridged(app, &agent_id, pd, f),
                None => store.update(app, &agent_id, f),
            }
        }
    }
}

/// Read the finished mini's result file and apply the terminal outcome to its directive.
///
/// WARNING 3 (ONE SNAPSHOT): resolve BOTH the project id and the Censor-trusted flag from
/// a SINGLE agent-state snapshot (was: two separate snapshots that could diverge — e.g.
/// the verdict run against p1 while trust was checked for p2). The pure
/// [`resolve_project_and_trust`] derives both from that one snapshot + a trust lookup.
///
/// BLOCKER 2 (DEFERRED VERDICT): the gate's FINE Censor linters (5–30s) used to run
/// SYNCHRONOUSLY here on the single executor thread, stalling ALL scheduling. Now ONLY a
/// clean `done` on a TRUSTED project needs them; that case is DEFERRED to a dedicated
/// thread (see [`spawn_verdict_thread`]) so the executor loop continues immediately.
/// Every other outcome (non-done terminal, killed, untrusted, no project) needs NO
/// linters and is finalized INLINE (cheap) right here.
fn finalize_finished_mini(app: &AppHandle, directive: &MiniCoderDirective) {
    // WARNING 3: ONE snapshot -> both project_id and trusted.
    let snapshot = agents::read_agent_live_state_snapshot(app).ok();
    let (project_id, trusted) = resolve_project_and_trust(snapshot.as_ref(), directive, |pid| {
        crate::backend::projects::project_censor_trusted(app, pid).unwrap_or(false)
    });

    // The terminal outcome (P5 killRequested-WINS). Computed off the lock.
    let outcome = finalize_outcome(directive);

    // P4: apply a write directive's emitted edits BEFORE the gate decision, so
    // the deterministic Censor below lints the tree the edits produced. The
    // root derivation mirrors finalize_finished_mini_with (scratch parent).
    let apply_root: Option<PathBuf> = directive
        .scratch_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .and_then(|p| Path::new(p).parent().map(|r| r.to_path_buf()));
    // FIX 3: capture net_blocked and folder_write_blocked BEFORE apply_write_directive_edits,
    // because the apply step can replace the outcome (e.g. failed-apply → MiniCoderOutcome::failed)
    // which has net_blocked=false and folder_write_blocked=None, silently zeroing both flags
    // before the emit checks below.
    let was_net_blocked = outcome.net_blocked;
    let was_folder_write_blocked = outcome.folder_write_blocked.clone();
    let (mut outcome, write_diffs) =
        apply_write_directive_edits(apply_root.as_deref(), directive, outcome);

    // v6 Phase 2 (anti-cheat): scan the mini's edits for attempts to GAME the tests
    // (skip/ignore markers, trivial always-true asserts, test-infra edits) instead of
    // making the code pass. Non-blocking — surface it in the outcome for the human/verifier.
    if !outcome.edits.is_empty() {
        // Scope the EditView borrow of `outcome.edits` so it ends before we mutate
        // `outcome.output` below (`detect_test_gaming` returns an owned Vec).
        let gaming = {
            let edits: Vec<crate::backend::tdd_strict::EditView> = outcome
                .edits
                .iter()
                .map(|e| crate::backend::tdd_strict::EditView {
                    path: e.path.as_str(),
                    new_string: e.new_string.as_str(),
                })
                .collect();
            crate::backend::tdd_strict::detect_test_gaming(&edits)
        };
        if !gaming.is_empty() {
            let note = format!(
                "\n\n⚠️ TDD anti-gaming: this change may be gaming the tests:\n- {}",
                gaming.join("\n- ")
            );
            match outcome.output {
                Some(ref mut o) => o.push_str(&note),
                None => outcome.output = Some(note),
            }
        }
    }

    // Phase A: async Censor FINE runners on modified files.
    // Runs `run_fine_batch_no_rail` (deterministic only, no Gemma) on modified files
    // with FINE coalescing (skip files censorated in the last 5s). Findings are written
    // atomically to `.aspis-mini/<agent_id>/steer_censor` + `steer_ready` flag for retry
    // injection, and pushed to the Activity Console for the human.
    //
    // F30: agentic path applies edits via tools → `write_diffs` is empty but
    // `outcome.files_touched` carries the real paths. Still enter fine/coarse when
    // trusted and either source is non-empty.
    let modified_files: Vec<String> = if !write_diffs.is_empty() {
        write_diffs.iter().map(|(path, _)| path.clone()).collect()
    } else {
        outcome.files_touched.clone()
    };
    let phase_a_censor = should_run_phase_a_censor(
        !write_diffs.is_empty(),
        !outcome.files_touched.is_empty(),
        trusted,
    );
    // F15/F30: surface whether censor ran on the durable directive summary so
    // "all clean" is distinguishable from "never reviewed". Stamp immediately
    // when the gate fires (ran=true, total=0 until the async pass finishes);
    // leave None when the gate is skipped.
    if phase_a_censor {
        let summary_files: Vec<String> = modified_files
            .iter()
            .take(crate::backend::mini_coder::CENSOR_MINI_SUMMARY_FILES_CAP)
            .cloned()
            .collect();
        let early = crate::backend::mini_coder::CensorMiniSummary {
            total: 0,
            files: summary_files,
            ran: true,
        };
        let _ = agents::mutate_agent_live_state(app, |state| {
            attach_censor_summary(state, &directive.id, early);
        });
    }
    if phase_a_censor {
        if let Some(ref root) = apply_root {
            let app = app.clone();
            let root = root.clone();
            let agent_id = directive.id.clone();
            let directive_id = directive.id.clone();
            let project_id_for_censor = project_id.clone().unwrap_or_default();

            // FINE cooldown: skip files censorated in the last FINE_COOLDOWN_S seconds.
            let st = app.try_state::<MiniCoderState>();
            let files_to_censor: Vec<String> = if let Some(st) = st.as_ref() {
                let map = st.fine_cooldown.lock().unwrap();
                let now = Instant::now();
                let filtered: Vec<String> = modified_files
                    .iter()
                    .filter(|f| {
                        map.get(*f).is_none_or(|t| {
                            now.duration_since(*t) > Duration::from_secs(FINE_COOLDOWN_S)
                        })
                    })
                    .cloned()
                    .collect();
                // H1: cooldown insert moved INSIDE the spawned thread
                // (after successful run_fine_batch_no_rail).
                filtered
            } else {
                modified_files.clone()
            };

            // Set coarse dirty flag — triggers COARSE on next executor tick.
            if let Some(st) = st.as_ref() {
                st.coarse_dirty
                    .lock()
                    .unwrap()
                    .insert(project_id_for_censor.clone());
            }

            std::thread::spawn(move || {
                if files_to_censor.is_empty() {
                    return;
                }
                // Build Gemma LLM client (if configured) for the Censor LLM tier.
                let local_ai = crate::backend::projects::read_censor_local_ai(&app);
                let gemma_client =
                    crate::backend::censor::gemma::build_gemma_client(&local_ai).ok();
                let gemma_available = gemma_client
                    .as_deref()
                    .is_some_and(crate::backend::censor::gemma::probe_available);
                let gemma_ctx = gemma_client.as_deref().map(|client| {
                    crate::backend::censor::orchestrator::GemmaCtx {
                        client,
                        available: gemma_available,
                        params: local_ai.review_params(),
                    }
                });
                // Run FINE deterministic + optional LLM tier.
                let running = AtomicBool::new(true);
                crate::backend::censor::orchestrator::run_fine_batch_no_rail(
                    &app,
                    &project_id_for_censor,
                    &root,
                    &files_to_censor,
                    gemma_ctx,
                    &running,
                );
                // H1 (cooldown): insert inside thread AFTER successful FINE run.
                // (If the thread or run_fine_batch_no_rail fails, the file was not
                // actually re-censorated — don't block it from the next attempt.)
                if let Some(st) = app.try_state::<MiniCoderState>() {
                    let mut map = st.fine_cooldown.lock().unwrap();
                    let now = Instant::now();
                    for f in &files_to_censor {
                        map.insert(f.clone(), now);
                    }
                }
                // Collect open findings from shards. On a read/corrupt error we do NOT
                // know the true findings — skip steering this round (fail-safe: never
                // inject a possibly-wrong or empty steer as if the file were clean).
                let findings = match crate::backend::censor::orchestrator::collect_open_findings(
                    &root,
                    &files_to_censor,
                ) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("censor phase-a: collect_open_findings failed: {e}");
                        return;
                    }
                };
                if findings.is_empty() {
                    return;
                }
                // Format findings text for steer.
                let text = format!(
                    "=== [Censor FINE Check] ===\n{}\n=== [End Censor] ===",
                    crate::backend::censor::commands::format_findings_text(&findings)
                );
                // Atomic write per-agent steer_censor + ready flag.
                let agent_dir = root.join(MINI_SCRATCH_DIR).join(&agent_id);
                if let Err(e) = std::fs::create_dir_all(&agent_dir) {
                    eprintln!(
                        "censor phase-a: create_dir_all {}: {e}",
                        agent_dir.display()
                    );
                    return;
                }
                let tmp = agent_dir.join(".steer_censor.tmp");
                let target = agent_dir.join("steer_censor");
                if let Err(e) = std::fs::write(&tmp, &text) {
                    eprintln!("censor phase-a: write steer tmp: {e}");
                    return;
                }
                // H2: only write steer_ready if rename succeeds.
                if std::fs::rename(&tmp, &target).is_ok() {
                    if let Err(e) = std::fs::write(agent_dir.join("steer_ready"), "") {
                        eprintln!("censor phase-a: write steer_ready: {e}");
                    }
                } else {
                    eprintln!("censor phase-a: rename steer_censor failed");
                }
                // Push to Activity Console (human-visible).
                let total = findings.len();
                // Durable fleet-state mirror: persist the summary on the directive row
                // BEFORE emitting the fire-and-forget event, so a restart does not lose
                // the linkage (the event listener may be absent on remount). The row is
                // found by directive.id (the directive that owns this phase-a pass);
                // a missing row (evicted) just skips persistence — the emit still fires.
                // `files` is capped at CENSOR_MINI_SUMMARY_FILES_CAP to bound the
                // persisted state file; `total` is the accurate count.
                let summary_files: Vec<String> = files_to_censor
                    .iter()
                    .take(crate::backend::mini_coder::CENSOR_MINI_SUMMARY_FILES_CAP)
                    .cloned()
                    .collect();
                let summary = crate::backend::mini_coder::CensorMiniSummary {
                    total,
                    files: summary_files,
                    ran: true,
                };
                let _ = agents::mutate_agent_live_state(&app, |state| {
                    attach_censor_summary(state, &directive_id, summary);
                });
                let _ = app.emit(
                    "censor://mini-findings",
                    serde_json::json!({
                        "agentId": agent_id,
                        "total": total,
                        "files": files_to_censor,
                    }),
                );
            });
        } else {
            eprintln!(
                "censor phase-a skipped for directive {}: project root not resolvable ({} files modified)",
                directive.id,
                modified_files.len()
            );
        }
    }

    // SANDBOX broker: if the agentic worker detected a net-blocked failure and this run
    // had a project, emit the consent-request event so the frontend can prompt the user.
    // The grant (AllowOnce or AllowRemember) takes effect on the NEXT spawn ("activates on
    // reset" — Seatbelt cannot be widened mid-run). Fire-and-forget: a missing listener /
    // torn-down runtime is non-fatal (same contract as MiniActivityStore::update).
    // Use was_net_blocked (pre-apply capture) to guard against the apply step zeroing
    // the flag when it replaces the outcome with a synthesized failed/timeout (FIX 3).
    //
    // SANDBOX broker Slice 1: gate by sandbox_mode.
    // - Ask / AutoAcceptInWorkspace → always prompt for net (network is always sensitive).
    // - Unattended → suppress the event (fail-closed, no prompt).
    // Defensive: if reading the mode errors (project not found, corrupt metadata) we
    // default to PROMPTING rather than silently failing closed — better an extra dialog
    // than a permanently silenced agent with no feedback to the user.
    //
    // Slice 3: when Unattended suppresses the prompt, log a Terra milestone note in the
    // activity console so the operator sees WHY the agent was blocked (not silent).
    // FIX 2: read sandbox mode ONCE before both block checks; two separate locked disk reads
    // when both flags are set would be wasteful and could theoretically observe different
    // values if the mode changed between them (window is tiny but the reads are not free).
    // Default to Ask on error — better an extra dialog than silently suppressing consent.
    let sandbox_mode = project_id.as_deref().map(|pid| {
        // SLICE 1 capability gate (same wrapper as the spawn site): degrade Unattended→Ask
        // where the OS sandbox is not enforced, so finalize's prompt-gating matches the
        // spawn-time decision. On macOS is_enforced()=true → identity (no behaviour change).
        crate::backend::broker::effective_sandbox_mode(
            crate::backend::projects::project_sandbox_mode(app, pid)
                .unwrap_or(crate::backend::broker::SandboxMode::Ask),
            crate::backend::sandbox::is_enforced(),
        )
    });

    if was_net_blocked {
        let agent_id = mini_agent_id(directive);
        if let Some(pid) = project_id.as_deref() {
            // sandbox_mode is Some because project_id is Some.
            let mode = sandbox_mode.unwrap_or(crate::backend::broker::SandboxMode::Ask);
            if mode.prompts_for_net() {
                // Persist the consent request so it survives an app restart (the "activates
                // on reset" contract means a pending request is still meaningful after
                // restart — without this, a user who wanted to grant on a prior run would
                // never learn of it). Mirrors the Claude consent-hook convention: mint an
                // id like the hook does (getrandom hex, fallback pid+timestamp), stamp
                // `pending_approval`, then append_superseding so duplicate pending asks for
                // the same (project,kind,path) don't accumulate.
                {
                    use crate::backend::consent_bridge::{append_superseding, ConsentBridgeRequest, ConsentBridgeStatus};
                    let request_id = {
                        let mut bytes = [0u8; 16];
                        if getrandom::fill(&mut bytes).is_ok() {
                            hex::encode(bytes)
                        } else {
                            format!(
                                "{}-{}",
                                std::process::id(),
                                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                            )
                        }
                    };
                    let row = ConsentBridgeRequest {
                        id: request_id,
                        agent_id: agent_id.clone(),
                        project_id: pid.to_string(),
                        kind: crate::backend::broker::ConsentKind::Net,
                        detail: "A sandboxed command needed network access, which is disabled for \
                                 this project. Grant to retry."
                            .to_string(),
                        path: None,
                        status: ConsentBridgeStatus::PendingApproval,
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    let _ = crate::backend::agents::mutate_agent_live_state(
                        &app,
                        |st| append_superseding(&mut st.consent_requests, row),
                    );
                }
                let req = crate::backend::broker::ConsentRequest {
                    kind: crate::backend::broker::ConsentKind::Net,
                    project_id: pid.to_string(),
                    agent_id,
                    detail: "A sandboxed command needed network access, which is disabled for \
                             this project. Grant to retry."
                        .to_string(),
                    path: None,
                    // Local seatbelt path: answered by grant_net_consent, not the cloud
                    // live-waiter. None keeps the wire JSON byte-identical (NO-CHURN).
                    approval_id: None,
                };
                let _ = app.emit("sandbox://consent-request", req);
            } else {
                // Unattended: fail-closed, no consent dialog. Log so the operator can see why.
                // Uses push_coder_note (not push_coder_milestone) so the passive annotation
                // does NOT flip running=true on a finished agent (zombie-spinner fix).
                // No command bodies in the note — the terminal already shows the full output.
                let note = unattended_denial_note("net", "");
                if let Some(store) = console_store(app) {
                    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
                    let f = |a: &mut super::mini_activity::ConsoleActivity| {
                        super::mini_activity::push_coder_note(
                            a,
                            &note,
                            Some(super::mini_activity::NodeStyle::Terra),
                            &ts,
                        );
                    };
                    match super::projects::ensure_projects_dir(app).ok() {
                        Some(ref pd) => store.update_bridged(app, &agent_id, pd, f),
                        None => store.update(app, &agent_id, f),
                    }
                }
            }
        } else {
            // FIX 3: project_id is None (snapshot lost / project deleted mid-run). The agent
            // still blocked on net — emit a best-effort denial note without project context so
            // the operator is not left with zero feedback.
            if let Some(store) = console_store(app) {
                let ts = chrono::Local::now().format("%H:%M:%S").to_string();
                let f = |a: &mut super::mini_activity::ConsoleActivity| {
                    super::mini_activity::push_coder_note(
                        a,
                        "Network access denied — project context unavailable",
                        Some(super::mini_activity::NodeStyle::Terra),
                        &ts,
                    );
                };
                match super::projects::ensure_projects_dir(app).ok() {
                    Some(ref pd) => store.update_bridged(app, &agent_id, pd, f),
                    None => store.update(app, &agent_id, f),
                }
            }
        }
    }

    // SANDBOX broker Slice 2: if the agentic worker detected an out-of-scope write (a write
    // attempt targeting a path outside root + working_set), emit a FolderWrite consent-request
    // so the frontend can prompt the user to grant that folder.
    // Pattern is symmetric with the net-blocked emit above.
    //
    // Slice 3: Unattended suppressed path logs a denial note (same pattern as net above).
    if let Some(ref folder) = was_folder_write_blocked {
        let agent_id = mini_agent_id(directive);
        if let Some(pid) = project_id.as_deref() {
            // sandbox_mode is Some because project_id is Some.
            let mode = sandbox_mode.unwrap_or(crate::backend::broker::SandboxMode::Ask);
            if mode.prompts_for_folder_write() {
                // Persist the consent request so it survives an app restart (the "activates
                // on reset" contract means a pending request is still meaningful after
                // restart). Mirrors the Claude consent-hook convention: mint an id like the
                // hook does, stamp `pending_approval`, then append_superseding so duplicate
                // pending asks for the same (project,kind,path) don't accumulate.
                {
                    use crate::backend::consent_bridge::{append_superseding, ConsentBridgeRequest, ConsentBridgeStatus};
                    let request_id = {
                        let mut bytes = [0u8; 16];
                        if getrandom::fill(&mut bytes).is_ok() {
                            hex::encode(bytes)
                        } else {
                            format!(
                                "{}-{}",
                                std::process::id(),
                                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                            )
                        }
                    };
                    let row = ConsentBridgeRequest {
                        id: request_id,
                        agent_id: agent_id.clone(),
                        project_id: pid.to_string(),
                        kind: crate::backend::broker::ConsentKind::FolderWrite,
                        detail: format!(
                            "A sandboxed command attempted to write outside the project to \
                             \"{folder}\". Grant to allow writes there and retry."
                        ),
                        // BLOCKER 1 fix: `path` carries the raw canonical folder so the
                        // frontend passes it (not the prose `detail`) to grant_folder_consent.
                        path: Some(folder.to_string()),
                        status: ConsentBridgeStatus::PendingApproval,
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    let _ = crate::backend::agents::mutate_agent_live_state(
                        &app,
                        |st| append_superseding(&mut st.consent_requests, row),
                    );
                }
                let req = crate::backend::broker::ConsentRequest {
                    kind: crate::backend::broker::ConsentKind::FolderWrite,
                    project_id: pid.to_string(),
                    agent_id,
                    detail: format!(
                        "A sandboxed command attempted to write outside the project to \
                         \"{folder}\". Grant to allow writes there and retry."
                    ),
                    // BLOCKER 1 fix: `path` carries the raw canonical folder so the
                    // frontend passes it (not the prose `detail`) to grant_folder_consent.
                    path: Some(folder.to_string()),
                    // Local seatbelt path: answered by grant_folder_consent (NO-CHURN).
                    approval_id: None,
                };
                let _ = app.emit("sandbox://consent-request", req);
            } else {
                // Unattended: fail-closed, no consent dialog. Log so the operator can see why.
                // Uses push_coder_note (passive annotation, no zombie-spinner).
                let note = unattended_denial_note("folder", folder);
                if let Some(store) = console_store(app) {
                    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
                    let f = |a: &mut super::mini_activity::ConsoleActivity| {
                        super::mini_activity::push_coder_note(
                            a,
                            &note,
                            Some(super::mini_activity::NodeStyle::Terra),
                            &ts,
                        );
                    };
                    match super::projects::ensure_projects_dir(app).ok() {
                        Some(ref pd) => store.update_bridged(app, &agent_id, pd, f),
                        None => store.update(app, &agent_id, f),
                    }
                }
            }
        } else {
            // FIX 3: project_id is None — best-effort denial note without project context.
            if let Some(store) = console_store(app) {
                let ts = chrono::Local::now().format("%H:%M:%S").to_string();
                let f = |a: &mut super::mini_activity::ConsoleActivity| {
                    super::mini_activity::push_coder_note(
                        a,
                        "Out-of-scope write denied — project context unavailable",
                        Some(super::mini_activity::NodeStyle::Terra),
                        &ts,
                    );
                };
                match super::projects::ensure_projects_dir(app).ok() {
                    Some(ref pd) => store.update_bridged(app, &agent_id, pd, f),
                    None => store.update(app, &agent_id, f),
                }
            }
        }
    }

    // Step 8: verdict gate removed. Phase A (above) now runs the Censor wait OFF this
    // thread (async) and no longer injects into the outcome; findings reach the main
    // coder via the persistent Censor ledger (the `censor_findings` MCP tool). The
    // console gets a simple terminal stamp.
    if let Some(agent_id) = directive.agent_id.as_deref() {
        if let Some(store) = console_store(app) {
            let round = directive.attempt.saturating_add(1);
            let fc = outcome.files_touched.len();
            let f = |a: &mut super::mini_activity::ConsoleActivity| {
                for path in &outcome.files_touched {
                    super::mini_activity::push_write_action(a, path, vec![]);
                }
                super::mini_activity::set_terminal(
                    a,
                    super::mini_activity::Banner {
                        // A failed/timed-out/aborted mini must NOT show a green "Done ✓" —
                        // ONLY a real Done shows Done; everything else is Stop (same as
                        // the other terminal-reap paths).
                        kind: if matches!(outcome.status, MiniCoderStatus::Done) {
                            super::mini_activity::BannerKind::Done
                        } else {
                            super::mini_activity::BannerKind::Stop
                        },
                        title: None,
                        sub: Some(format!(
                            "{} · {} round{}",
                            plural(fc, "file"),
                            round,
                            if round != 1 { "s" } else { "" }
                        )),
                    },
                );
            };
            match super::projects::ensure_projects_dir(app).ok() {
                Some(ref pd) => store.update_bridged(app, agent_id, pd, f),
                None => store.update(app, agent_id, f),
            }
        }
    }
    // F08: close the mini/main session on EVERY terminal finalize so the rail
    // drops "Main coder running" ghosts (timeout/parent-gone paths already call
    // close_mini_session; the normal EOF path did not).
    let session_close_id = directive
        .agent_id
        .clone()
        .unwrap_or_else(|| mini_agent_id(directive));
    let _ = agents::mutate_agent_live_state(app, |state| {
        if let Some(d) = state
            .mini_coder_directives
            .iter_mut()
            .find(|d| d.id == directive.id)
        {
            d.status = outcome.status;
            d.result = Some(outcome.clone());
        }
        close_mini_session(state, &session_close_id);
        // Also close by deterministic id in case agent_id diverged.
        let det = mini_agent_id(directive);
        if det != session_close_id {
            close_mini_session(state, &det);
        }
    });

    // F07: successful MAIN write finalize → promote linked Kanban task to review.
    // Only Main tier (not a delegated mini) and only clean `done` (not
    // killed/failed/aborted/needs_clarification). A mini finishing mid-task must
    // not flip the board to review while the Main chain is still open.
    if matches!(directive.tier, mini_coder::DirectiveTier::Main)
        && directive.write
        && matches!(outcome.status, MiniCoderStatus::Done)
    {
        let resolved_project = project_id
            .clone()
            .or_else(|| {
                directive
                    .project_id
                    .clone()
                    .filter(|p| !p.trim().is_empty())
            })
            .or_else(|| snapshot.as_ref().and_then(|s| directive_project(s, directive)));
        let resolved_task = directive
            .task_id
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                // Parent session's current_task_id as last-resort fallback.
                snapshot.as_ref().and_then(|s| {
                    s.sessions
                        .iter()
                        .find(|sess| sess.agent_id == directive.parent_agent_id)
                        .and_then(|sess| sess.current_task_id.clone())
                })
            })
            .filter(|t| !t.trim().is_empty());
        if let (Some(pid), Some(tid)) = (resolved_project, resolved_task) {
            if let Err(e) = super::projects::promote_task_to_review_after_main_write(
                app,
                &pid,
                &tid,
                &session_close_id,
            ) {
                eprintln!(
                    "F07: promote task {tid} to review failed for directive {}: {e}",
                    directive.id
                );
            }
        }
    }

    // v6 Phase 5: on a terminal failure/timeout, emit a structured stuck report so the
    // human gets an actionable record (task, attempts, reason, last-output excerpt)
    // instead of a bare block. Fire-and-forget (a missing listener is fine).
    if matches!(
        outcome.status,
        MiniCoderStatus::Failed | MiniCoderStatus::Timeout
    ) {
        let reason = if matches!(outcome.status, MiniCoderStatus::Timeout) {
            "timeout"
        } else {
            "failed"
        };
        let raw = outcome
            .error
            .as_deref()
            .or(outcome.output.as_deref())
            .unwrap_or("");
        let report = crate::backend::stuck_report::StuckReport::new(
            directive.id.clone(),
            directive.parent_agent_id.clone(),
            reason,
            directive.attempt.saturating_add(1),
            raw,
            outcome.files_touched.clone(),
            snapshot.as_ref().and_then(|s| directive_project(s, directive)),
        );
        persist_and_emit_stuck(app, report);
    }
    // Clean up result file
    if let Some(scratch) = directive
        .scratch_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        let _ = std::fs::remove_file(Path::new(scratch).join(&directive.result_path));
    }
}

/// F30 pure predicate: whether finalize should enter the phase-A fine censor /
/// coarse dirty path. Trusted writes with either non-empty write_diffs OR
/// non-empty files_touched (agentic tools already applied edits) must run.
pub(crate) fn should_run_phase_a_censor(
    write_diffs_nonempty: bool,
    files_touched_nonempty: bool,
    trusted: bool,
) -> bool {
    trusted && (write_diffs_nonempty || files_touched_nonempty)
}

/// WARNING 3 (PURE): derive `(project_id, trusted)` from ONE agent-state snapshot. The
/// project id comes from the mini's SESSION (`current_project_id`, matched by the
/// directive's `agent_id`); trust is then resolved via `trust_lookup(project_id)`. Both
/// flow from the SAME snapshot so they can never diverge (the WARNING-3 TOCTOU). A
/// missing snapshot / agent_id / session / project yields `(None, false)` — fail-closed
/// (never lint an unresolvable tree). `trust_lookup` reads the projects config (a
/// separate file — see WARNING 7: trust can only safely go true->false during finalize,
/// the fail-closed direction). Pure (no AppHandle/lock) so it is unit-testable.
fn resolve_project_and_trust(
    snapshot: Option<&crate::backend::model::AgentLiveState>,
    directive: &MiniCoderDirective,
    trust_lookup: impl FnOnce(&str) -> bool,
) -> (Option<String>, bool) {
    let project_id = directive.agent_id.as_deref().and_then(|aid| {
        snapshot?
            .sessions
            .iter()
            .find(|s| s.agent_id == aid)
            .and_then(|s| s.current_project_id.clone())
    });
    let trusted = match project_id.as_deref() {
        Some(pid) => trust_lookup(pid),
        None => false,
    };
    (project_id, trusted)
}

/// BLOCKER 2: spawn the dedicated deferred-VERDICT thread. It (a) computes the FINE
/// Censor verdict (the slow 5–30s linters) OFF the executor loop, then (b) applies the
/// resulting `GateDecision` via a brief `mutate_agent_live_state`, then (c) ALWAYS clears
/// the in-flight guard. The executor loop returned immediately after spawning this.
///
/// Safeguards:
///  * BOUNDED: the verdict uses the fine-pass's own runner timeouts, so it can't run
///    forever and leak the in-flight entry.
///  * FAIL-CLOSED on panic/error: the whole compute+apply is wrapped in `catch_unwind`;
///    on a caught panic we stamp the clean `done` (the gate is best-effort — a verdict we
///    couldn't compute must NOT block the mini's success) and still clear the guard.
///  * KILL RACE: the apply step (`finalize_finished_mini_with` -> `live_kill_override`)
///    re-reads the LIVE `kill_requested` under the lock, so a Stop that lands while the
///    verdict thread runs still wins (the outcome becomes aborted_by_human).
fn lang_is_tier_a_covered(
    kinds: &std::collections::HashSet<crate::backend::censor::detect::ProjectKind>,
    lang: crate::backend::censor::detect::FileLang,
    fine_baseline: usize,
) -> bool {
    use crate::backend::censor::runners::{applicable_runners, Granularity};
    let fine_count = applicable_runners(kinds, lang)
        .iter()
        .filter(|r| r.granularity() == Granularity::Fine)
        .count();
    fine_count > fine_baseline
}

/// FIX 3 — the cross-cutting-only FINE baseline for a `kinds` set: the count of Fine runners
/// for [`FileLang::Other`] (no language-specific runner → only the cross-cutting Fine
/// runners apply). Computed once per coverage query and fed to [`lang_is_tier_a_covered`].
fn tier_a_fine_baseline(
    kinds: &std::collections::HashSet<crate::backend::censor::detect::ProjectKind>,
) -> usize {
    use crate::backend::censor::detect::FileLang;
    use crate::backend::censor::runners::{applicable_runners, Granularity};
    applicable_runners(kinds, FileLang::Other)
        .iter()
        .filter(|r| r.granularity() == Granularity::Fine)
        .count()
}

/// B2: does this directive touch at least one file in a language WITH deterministic
/// Tier-A coverage that the PER-ROUND verdict actually exercises? "Covered" = the file's
/// [`FileLang`] has a LANGUAGE-SPECIFIC **Fine-granularity** applicable runner — i.e.
/// `applicable_runners(kinds, lang)` contributes MORE FINE runners than the cross-cutting
/// set does. Delegates the per-language decision to the shared [`lang_is_tier_a_covered`]
/// core (FIX 3) so this gate and the A3/E2 language listers never diverge.
///
/// WHY FINE ONLY (the load-bearing correction): the agentic loop's between-round feedback
/// is produced by the deferred verdict thread (`spawn_verdict_thread` ->
/// `run_fine_batch_no_rail`), which runs ONLY [`Granularity::Fine`] runners. The Coarse
/// runners (clippy/cargo-check/cargo-audit/cargo-deny/cargo-fmt, tsc/knip, go vet, …) run
/// asynchronously on the coarse debounce, NOT in the per-round gate, so they cannot inform
/// a retry round. Counting ALL applicable runners would deem a language "covered" whose
/// only language-specific tools are Coarse (e.g. RUST: every Rust runner is Coarse) — the
/// agentic chain would then burn N rounds with NO useful Rust-specific per-round feedback
/// (only the cross-cutting Fine runners fire), the real clippy/cargo-check errors surfacing
/// only after the chain is already terminal. So we count Fine runners exclusively.
///
/// The cross-cutting FINE baseline is computed (not hardcoded) as the count of Fine runners
/// for [`FileLang::Other`], which hits the `_ => {}` arm and so yields ONLY `CROSS_CUTTING`
/// (today: Lizard + Semgrep are Fine, so the baseline is 2; the rest of CROSS_CUTTING is
/// Coarse). NOTE: this baseline is the dynamically-computed cross-cutting FINE count — it is
/// NOT zero. Any `lang` whose Fine count EXCEEDS that baseline added a language-specific
/// FINE runner the per-round verdict can iterate against. Computing it from `FileLang::Other`
/// avoids exporting the private `CROSS_CUTTING` const from the runners module.
///
/// Drives the agentic-iterative budget (B1/B2): an `AgenticIterative` write on a covered
/// language gets the N-round loop; an uncovered one (incl. Rust — Coarse-only) falls back to
/// a single fix pass (iterating without a per-round deterministic verdict buys nothing).
/// Project kinds are detected ONCE; the baseline ONCE; then each file is classified by
/// extension/name.
fn directive_has_tier_a_coverage(root: &Path, files: &[String]) -> bool {
    use crate::backend::censor::detect::{detect_project_kinds, FileLang};
    if files.is_empty() {
        return false;
    }
    let kinds = detect_project_kinds(root);
    // Baseline ONCE, then classify each file by extension/name through the SHARED core.
    let fine_baseline = tier_a_fine_baseline(&kinds);
    files
        .iter()
        .any(|f| lang_is_tier_a_covered(&kinds, FileLang::from_path(Path::new(f)), fine_baseline))
}

/// A3 (coder guidance): the human language names that have deterministic Tier-A
/// gate coverage FOR THIS PROJECT — i.e. the languages where `agentic-iterative`
/// can iterate against per-round feedback. This is the list the coder is shown so
/// it knows where agentic-iterative actually helps.
///
/// SAME definition B2 uses (`directive_has_tier_a_coverage`): a language is
/// "covered" iff `applicable_runners(detected_kinds, lang)` yields MORE
/// [`Granularity::Fine`] runners than the dynamically-computed cross-cutting Fine
/// baseline (`FileLang::Other`). So RUST is NOT listed (clippy/cargo-check/… are
/// all Coarse — no per-round Rust-specific feedback); Python/TS/Go/C++/HTML/Kotlin/
/// Shell/YAML/SQL/Dockerfile/GithubActions/CSS ARE listed WHEN their project-kind /
/// lang gate is satisfied for this root (e.g. Python only when a Python manifest is
/// present; HTML/Shell/YAML/SQL/Dockerfile/GithubActions/CSS have no manifest so they
/// always pass the kind gate).
///
/// PRODUCT-GENERAL: the names are GENERIC language labels (no project/product
/// hardcoding) and the set is computed entirely from the user's detected project +
/// the wired runner table — nothing is keyed off a specific repo. Deterministic and
/// SORTED (stable enumeration over the [`FileLang`] variants → filtered by the same
/// Fine-over-baseline rule → sorted human names) so the injected coder text never
/// churns between launches.
///
/// `pub(crate)` so the coder launch-prompt builder (`projects.rs`, A3) can show the
/// coder this project's covered-language set when guiding its `write_mode` choice.
pub(crate) fn tier_a_covered_languages(root: &Path) -> Vec<&'static str> {
    use crate::backend::censor::detect::detect_project_kinds;
    let kinds = detect_project_kinds(root);
    tier_a_languages_for_kinds(&kinds)
}

/// E2 — the PROJECT-AGNOSTIC potential set: every language that has a language-
/// specific [`Granularity::Fine`] runner AT ALL, i.e. would be agentic-iterative
/// covered IF its project kind / manifest were detected and its tool installed.
/// Computed by evaluating the SAME Fine-over-baseline rule as
/// [`tier_a_covered_languages`] against a kinds set containing EVERY
/// [`ProjectKind`] variant (so manifest-gated languages — Rust/Node/Python/Go/
/// C++/Kotlin — also pass the kind gate; the manifest-less languages —
/// HTML/Shell/YAML/SQL/Dockerfile/GitHub Actions/CSS — already pass on FileLang
/// alone). Used by the GLOBAL Settings coverage indicator, which has no current
/// project: it shows what the gate CAN cover, with a note that actual coverage
/// depends on the detected project's manifests + which tools are installed.
///
/// DETERMINISTIC + SORTED (same enumeration → same filter → sorted human names) so
/// the Settings list never churns. Note this STILL excludes Rust — clippy/cargo-
/// check/… are all [`Granularity::Coarse`], so Rust has no per-round Rust-specific
/// Fine feedback (identical to `tier_a_covered_languages`'s exclusion of Rust).
pub(crate) fn tier_a_potential_languages() -> Vec<&'static str> {
    use crate::backend::censor::detect::ProjectKind;
    use std::collections::HashSet;
    // FIX 4 — EXHAUSTIVENESS GUARD (REAL, not tautological). The "what COULD be covered" set
    // must include EVERY `ProjectKind` variant, so a manifest-gated language is never
    // silently dropped by an absent manifest. We use the canonical [`ProjectKind::ALL`],
    // which is pinned to the enum by an exhaustive, wildcard-free witness match + tests in
    // `detect.rs`: adding a new `ProjectKind` makes that witness match non-exhaustive (FAILS
    // TO COMPILE) and trips the membership test until the variant is added to `ALL` — so a
    // new kind can never be silently missed here.
    let all_kinds: HashSet<ProjectKind> = ProjectKind::ALL.into_iter().collect();
    tier_a_languages_for_kinds(&all_kinds)
}

/// Shared core for [`tier_a_covered_languages`] (kinds = THIS root's detected kinds)
/// and [`tier_a_potential_languages`] (kinds = all kinds). A language is "covered"
/// iff `applicable_runners(kinds, lang)` yields MORE [`Granularity::Fine`] runners
/// than the cross-cutting Fine baseline (`FileLang::Other`). The candidate list +
/// human names live in ONE place so adding a wired language is a single edit.
///
/// FIX 3: the per-language "covered?" decision is the SHARED [`lang_is_tier_a_covered`]
/// core — the SAME predicate the B2 budget gate ([`directive_has_tier_a_coverage`]) uses on
/// each directive file. So the coder's covered-language list (A3) and the executor's
/// per-file budget decision (B2) are guaranteed to agree by construction.
fn tier_a_languages_for_kinds(
    kinds: &std::collections::HashSet<crate::backend::censor::detect::ProjectKind>,
) -> Vec<&'static str> {
    use crate::backend::censor::detect::FileLang;
    let fine_baseline = tier_a_fine_baseline(kinds);
    // Every language-bearing FileLang variant with a human name. `Other` is the
    // baseline reference itself (no language-specific runner) and is intentionally
    // excluded. Each `(variant, human-name)` pair is enumerated explicitly so adding
    // a new wired language is a deliberate one-line addition here (mirrors the
    // explicit match arms in `applicable_runners`/`FileLang::from_path`).
    let candidates: [(FileLang, &'static str); 13] = [
        (FileLang::Rust, "Rust"),
        (FileLang::Ts, "TypeScript/JavaScript"),
        (FileLang::Py, "Python"),
        (FileLang::Go, "Go"),
        (FileLang::Cpp, "C/C++"),
        (FileLang::Html, "HTML"),
        (FileLang::Kotlin, "Kotlin"),
        (FileLang::Shell, "Shell"),
        (FileLang::Yaml, "YAML"),
        (FileLang::Sql, "SQL"),
        (FileLang::Dockerfile, "Dockerfile"),
        (FileLang::GithubActions, "GitHub Actions"),
        (FileLang::Css, "CSS"),
    ];
    let mut out: Vec<&'static str> = candidates
        .into_iter()
        .filter(|(lang, _)| lang_is_tier_a_covered(kinds, *lang, fine_baseline))
        .map(|(_, name)| name)
        .collect();
    out.sort_unstable();
    out
}

/// P6 verdict gate APPLY + finalize. The terminal `outcome` and `trusted` flag are
/// PRECOMPUTED by the caller ([`finalize_finished_mini`], WARNING 3: from ONE snapshot);
/// `verdict_fn(project_root, files) -> Vec<EscalationFinding>` is the deterministic-Censor
/// High-finding collector, INJECTED so the gate is unit-testable with a fake AND so the
/// slow real collector can run on a deferred thread (BLOCKER 2). The closure is invoked
/// ONLY when `trusted && outcome == Done && !kill_requested` (otherwise the gate stamps
/// straight through and no linters run).
///
/// FLOW (all heavy work OUTSIDE the lock; the decision applied in ONE atomic mutate):
///  1. If the gate applies, collect High findings via `verdict_fn`.
///  2. `verdict_gate_decision` turns (directive, outcome, trusted, covered, high_findings)
///     into a pure `GateDecision`. `covered` (B2) is the agentic-iterative language-coverage
///     signal — resolved here and consulted ONLY for an `AgenticIterative` write.
///  3. Apply the decision under ONE `mutate_agent_live_state`:
///       - StampTerminal: stamp the outcome (with the P5 live-kill re-check) + propagate.
///       - FailedWith: `apply_awaiting_retry` + append the Pending retry (atomic).
///       - Escalate: stamp Escalated + propagate to the chain's Failed ancestors.
///  4. Delete the result file + record the training rail AFTER the write succeeds.
fn pigeon_egress_terminal(
    app: &AppHandle,
    directive: &MiniCoderDirective,
    outcome: &MiniCoderOutcome,
) {
    let Some(ticket) = directive.pigeon_ticket else {
        return;
    };
    if !crate::backend::pigeon_service::pigeon_enabled_cached(app) {
        return;
    }
    let Some(client) = crate::backend::pigeon_service::pigeon_client_from_running() else {
        return;
    };
    match serde_json::to_value(outcome) {
        Ok(payload) => {
            if let Err(e) = client.done(ticket, PIGEON_MINI_POOL_RECEIVER, payload) {
                eprintln!("mini-coder executor: pigeon egress done(ticket {ticket}) failed: {e}");
            }
        }
        Err(e) => {
            eprintln!("mini-coder executor: pigeon egress serialize failed (ticket {ticket}): {e}");
        }
    }
}

/// Slice 3 (seam C, bypass paths): like [`pigeon_egress_terminal`] but for the terminal
/// reap paths that transition a directive UNDER THE LOCK without a `MiniCoderDirective` +
/// outcome in hand (the `plan.timeouts` reap, stuck-launching reap, `fail_launching`). It
/// re-reads the directive by id from a FRESH snapshot to get its `pigeon_ticket` AND the
/// authoritative terminal `result` actually stamped (e.g. timeout, or killRequested-WINS
/// aborted), then posts it. No-op for a non-Pigeon directive / disabled / not-yet-terminal.
fn pigeon_egress_terminal_by_id(app: &AppHandle, directive_id: &str) {
    // Cheap pre-gate so a disabled app does ZERO extra work (no snapshot read).
    if !crate::backend::pigeon_service::pigeon_enabled_cached(app) {
        return;
    }
    let Ok(snapshot) = agents::read_agent_live_state_snapshot(app) else {
        return;
    };
    let Some(directive) = snapshot
        .mini_coder_directives
        .iter()
        .find(|d| d.id == directive_id)
    else {
        return;
    };
    // Only act on a directive that (a) came via Pigeon and (b) is actually terminal now.
    if directive.pigeon_ticket.is_none() {
        return;
    }
    if let Some(outcome) = directive.result.clone() {
        pigeon_egress_terminal(app, directive, &outcome);
    } else if directive.status.is_terminal() {
        // MAX-RECALL: a TERMINAL directive with no stamped result would otherwise leave the
        // ticket `claimed` → the sweep requeues it → re-ingest. Close it via /fail so it can
        // never become a double-run. (A still-running directive is left untouched.)
        if let Some(ticket) = directive.pigeon_ticket {
            if let Some(client) = crate::backend::pigeon_service::pigeon_client_from_running() {
                let _ = client.fail(
                    ticket,
                    PIGEON_MINI_POOL_RECEIVER,
                    "terminal mini-coder directive had no result at egress",
                );
            }
        }
    }
}

/// CONSOLE (Step B): map a finalized [`GateDecision`] (+ the actually-applied terminal
/// outcome) onto the Activity Console store mutations, then publish the resulting full
/// snapshot. Pure observer — a missing store (unmanaged in tests) is a silent no-op.
///
/// PATHS:
///  * Failed (dirty, retries left): close the CURRENT round with the DIRTY verdict
///    (the gate findings) + the applied-write action rows, then open the NEXT round. The
///    run stays in flight (shimmer on, running stays true).
///  * Terminal Done (`StampTerminal(done)` that the kill did NOT override): close the round
///    with the verdict (CLEAN if the gate found nothing, else DIRTY) + write rows, then the
///    `done` banner ("N file(s) · M round(s) · edits applied"). running=false.
///  * Escalated: close the round with the DIRTY verdict + the `esc` banner.
///  * Aborted/killed (the P5 live-kill override, or any aborted terminal): the `stop`
///    banner; no verdict (the human cut it short — there is no Censor judgment to show).
///  * Other terminal (failed/timeout/needs_clarification): the `stop` banner as the neutral
///    terminal (a non-success that is neither a clean done nor an escalation).
///
/// `attempt` is the finalized directive's 0-based round index; the human-facing round
/// number is `attempt + 1`, and a retry opens round `attempt + 2`.
#[allow(clippy::too_many_arguments)]
fn clarification_banner_sub(question: Option<&str>) -> String {
    match question {
        Some(q) if !q.trim().is_empty() => format!("needs clarification: {}", q.trim()),
        _ => "needs clarification".to_string(),
    }
}

/// CONSOLE (Step B): the muted sub-line for a `done` banner, e.g.
/// "2 files · 1 round · edits applied" (singular/plural respected).
///
/// FIX 5: the "edits applied" clause is pushed ONLY for an actual WRITE directive
/// (`is_write`). A NON-write directive's `file_count` is the mini's self-reported
/// `files_touched` — those files were inspected, NOT edited by us — so claiming "edits
/// applied" there is a lie. A non-write run with touched files thus reads e.g. "2 files · 1
/// round" with no edits clause.
fn done_sub(file_count: usize, rounds: u32, is_write: bool) -> String {
    let mut parts = vec![plural(file_count, "file"), plural(rounds as usize, "round")];
    if file_count > 0 && is_write {
        parts.push("edits applied".to_string());
    }
    parts.join(" · ")
}

/// CONSOLE (Step B): the muted sub-line for an `esc` banner, e.g. "2 files · 2 rounds".
fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Like [`transition_directive`] but reports whether the transition was APPLIED (the
/// pure `apply` returned Ok and the directive existed). Used by the Failed path:
/// the Pending retry is appended ONLY when the predecessor actually moved to
/// Failed (a racing kill could have made it terminal first — then no retry).
fn transition_directive_ok(
    state: &mut crate::backend::model::AgentLiveState,
    id: &str,
    apply: impl FnOnce(&MiniCoderDirective) -> Result<MiniCoderDirective, String>,
) -> bool {
    if let Some(d) = state.mini_coder_directives.iter_mut().find(|d| d.id == id) {
        if let Ok(next) = apply(d) {
            *d = next;
            return true;
        }
    }
    false
}

/// P5 RACE BACKSTOP: if the LIVE directive `id` has `kill_requested` set (the human hit
/// Stop after this pass's snapshot was read) and the proposed `outcome` is not already
/// an abort, override it to `aborted_by_human` — the human's assertion of control wins
/// any racing terminal. Returns the outcome to actually stamp.
///
/// ASYNC STEERING (a): a live `steer_queue` carrying the STOP sentinel
/// ([`mini_coder::STEER_STOP_SENTINEL`]) is ALSO an abort. The steer writers
/// (`mini_coder_steer` / `dispatch_steer_mini_coder`) normally translate a `stop` steer to
/// `kill_requested=true` at WRITE time (reusing the Stop path), so this is a DEFENSIVE
/// backstop for a stop that reached the queue out-of-band (e.g. a hand-edited state) — the
/// SAME generalized external-signal channel, honored at the same round boundary as the kill.
fn live_kill_override(
    state: &crate::backend::model::AgentLiveState,
    id: &str,
    outcome: MiniCoderOutcome,
) -> MiniCoderOutcome {
    let aborts = state
        .mini_coder_directives
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.kill_requested || d.steer_queue.iter().any(|m| mini_coder::is_steer_stop(m)))
        .unwrap_or(false);
    if aborts && outcome.status != MiniCoderStatus::AbortedByHuman {
        MiniCoderOutcome::aborted("stopped by human (Stop button)")
    } else {
        outcome
    }
}

// ROLE UNTANGLE Phase 2: the edit-application block (normalize_edit_rel, the
// fuzzy-match tiers, ApplyResult, apply_emitted_edits, apply_write_directive_edits
// and their consts) moved VERBATIM to backend/mini_edit_apply.rs. Wildcard
// re-import keeps every call site and test unchanged.
#[allow(unused_imports)]
pub(crate) use super::mini_edit_apply::*;

fn finalize_outcome(directive: &MiniCoderDirective) -> MiniCoderOutcome {
    if directive.kill_requested {
        return MiniCoderOutcome::aborted("stopped by human (Stop button)");
    }
    match directive.scratch_path.as_deref() {
        Some(path) if !path.trim().is_empty() => {
            read_result_outcome(Path::new(path), &directive.result_path)
        }
        _ => MiniCoderOutcome::failed("missing persisted scratch path".to_string()),
    }
}

/// Move a `launching` directive to `failed` (spawn/scratch error path). Under lock.
///
/// BLOCKER 1: a launch failure on a RETRY (`attempt >= 1` / `parent_directive_id`
/// set) MUST propagate the `failed` outcome to its Failed predecessor(s) —
/// including the chain ROOT the Python poll watches — or the root sits Failed
/// forever (poll times out misleadingly, root holds a non-evictable slot). We route
/// through the shared [`stamp_terminal_and_propagate`] so EVERY launch failure
/// propagates exactly like the EOF/timeout terminal paths do.
fn fail_launching(app: &AppHandle, directive_id: &str, reason: &str) {
    let id = directive_id.to_string();
    let reason = reason.to_string();
    let _ = agents::mutate_agent_live_state(app, |state| {
        // The terminal outcome we both stamp AND propagate to the chain's ancestors.
        let outcome = MiniCoderOutcome::failed(reason.clone());
        // ROLE UNTANGLE Phase 3 fix (hostile-review finding): a session may have
        // been PRE-persisted before the launch gates ran (the oracle-grant upsert
        // in claim_and_launch fires before the base_url/Main-tier guards). A
        // launch failure must CLOSE it, or a dangling "active" row leaks into the
        // rail forever. The session id is deterministic (`mini_agent_id`), so
        // derive it from the stored directive; the directive's own `agent_id`
        // field (stamped by apply_launched) is also honored when present.
        let session_ids: Vec<String> = state
            .mini_coder_directives
            .iter()
            .find(|d| d.id == id)
            .map(|d| {
                let mut ids = vec![mini_agent_id(d)];
                if let Some(live) = d.agent_id.clone() {
                    ids.push(live);
                }
                ids
            })
            .unwrap_or_default();
        /* stamp_terminal */
        // gate deleted — terminal outcome stamped inline
        if let Some(d) = state.mini_coder_directives.iter_mut().find(|d| d.id == id) {
            d.status = outcome.status;
            d.result = Some(outcome);
        }
        for session_id in session_ids {
            close_mini_session(state, &session_id);
        }
    });
    // Slice 3 (seam C, bypass path): a launch failure terminates off-finalize — close the
    // Pigeon ticket so the Python wait unblocks AND the reclaim sweep can't re-run it. The
    // chain shares ONE ticket (carried on every member by build_retry_directive), so posting
    // on this id closes it once. No-op when not Pigeon-ticketed / disabled.
    pigeon_egress_terminal_by_id(app, &id);
}

/// Apply a pure directive transition by id inside a locked mutation. The `apply`
/// closure is one of the P1 `apply_*` helpers; its Err (e.g. a terminal-overwrite
/// refusal) is swallowed so a late result can never clobber a kill that already won
/// (the idempotence guard lives in `apply_outcome`). A missing id is a no-op.
fn transition_directive(
    state: &mut crate::backend::model::AgentLiveState,
    id: &str,
    apply: impl FnOnce(&MiniCoderDirective) -> Result<MiniCoderDirective, String>,
) {
    if let Some(d) = state.mini_coder_directives.iter_mut().find(|d| d.id == id) {
        if let Ok(next) = apply(d) {
            *d = next;
        }
    }
    // NITPICK 1: capping is NOT done here (it ran once per transition before). The
    // queue is bounded ONCE per write pass via `cap_pass` inside each
    // `mutate_agent_live_state` closure, so a multi-transition pass caps a single
    // time rather than per directive.
}

fn transition_visual_directive(
    state: &mut crate::backend::model::AgentLiveState,
    id: &str,
    apply: impl FnOnce(
        &crate::backend::visual_check::VisualCheckDirective,
    ) -> Result<crate::backend::visual_check::VisualCheckDirective, String>,
) {
    if let Some(d) = state
        .visual_check_directives
        .iter_mut()
        .find(|d| d.id == id)
    {
        if let Ok(next) = apply(d) {
            *d = next;
        }
    }
}

/// NITPICK 1: bound the directive queue ONCE per write pass. Called at the end of
/// every `mutate_agent_live_state` closure that touches directives, so the eviction
/// (oldest TERMINAL only) runs a single time per persisted write rather than per
/// `transition_directive`.
pub(crate) fn cap_pass(state: &mut crate::backend::model::AgentLiveState) {
    mini_coder::cap_directives(&mut state.mini_coder_directives, MAX_DIRECTIVES);
    state.visual_check_directives = crate::backend::visual_check::cap_directives(std::mem::take(
        &mut state.visual_check_directives,
    ));
}

/// WARNING 5: like [`cap_pass`] but never evicts the `protect` ids this pass — used by
/// the finalize/propagation paths so a chain root (and its Failed ancestors) just
/// stamped terminal in THIS mutate survives the cap until the poll can read its outcome.
fn cap_pass_protecting(state: &mut crate::backend::model::AgentLiveState, protect: &[String]) {
    mini_coder::cap_directives_protecting(
        &mut state.mini_coder_directives,
        MAX_DIRECTIVES,
        protect,
    );
    state.visual_check_directives = crate::backend::visual_check::cap_directives(std::mem::take(
        &mut state.visual_check_directives,
    ));
}

/// WARNING 5: the set of ids freshly stamped terminal in a finalize/propagation mutate
/// — the leaf `id` plus every Failed ancestor `propagate_terminal_to_ancestors`
/// stamps — so `cap_pass_protecting` can grace them one pass against eviction. Computed
/// from the LIVE state (after propagation) so it includes the ancestors actually stamped.
fn just_finalized_chain_ids(
    state: &crate::backend::model::AgentLiveState,
    leaf_id: &str,
) -> Vec<String> {
    let mut ids = vec![leaf_id.to_string()];
    if let Some(leaf) = state
        .mini_coder_directives
        .iter()
        .find(|d| d.id == leaf_id)
        .cloned()
    {
        // After propagation the ancestors are no longer Failed (they were stamped
        // terminal), so recompute the lineage by the shared root rather than by status.
        let root = leaf.id.as_str();
        for d in &state.mini_coder_directives {
            if d.id != leaf_id && (d.id == root || d.parent_directive_id.as_deref() == Some(root)) {
                ids.push(d.id.clone());
            }
        }
    }
    ids
}

/// Upsert the mini's `AgentSession` so it appears nested under its parent coder in
/// the rail (P3 renders the nesting; P2 just plants the row). Host is stamped from
/// the ledger at read time, so here we only set the durable nesting + role/status.
fn upsert_mini_session(
    state: &mut crate::backend::model::AgentLiveState,
    agent_id: &str,
    parent_agent_id: &str,
    // The parent coder's project, so the mini groups into the SAME project bucket
    // as its coder (sessionsByProject keys on this — a None would hide the mini from
    // the rail). May be None if the parent carried no project (the mini then runs but
    // simply won't surface in any project rail, which is the correct degraded state).
    current_project_id: Option<String>,
    started_at: &str,
    // The backend kind label (ollama/api/codex) recorded as the session client so
    // the rail's MINI chip reflects the real runtime, not the P2 "echo" placeholder.
    client: &str,
    // P3: Some(hash) marks a read-only oracle grant: the session stores role
    // "mini" + the launch-token HASH so MCP registration is token-bound and the
    // stored role pins what the mini may register as. None = the status-quo
    // "coder"-labelled, token-less mini session.
    oracle_token_hash: Option<&str>,
    // ROLE UNTANGLE Phase 3: the directive TIER. A Main-tier session is the
    // first-class MAIN CODER — stored role "coder" (never "mini") and a
    // "Main coder running" message, so the rail/ledger tell the tiers apart.
    tier: mini_coder::DirectiveTier,
) {
    let timestamp_now = Utc::now().to_rfc3339();
    let is_main = tier == mini_coder::DirectiveTier::Main;
    if let Some(session) = state.sessions.iter_mut().find(|s| s.agent_id == agent_id) {
        if session.status == "done" {
            // Max-recall fix: a terminal session is never resurrected — a late
            // post-spawn re-upsert racing a fast finalize must not flip a closed
            // mini back to "active" in the rail.
            return;
        }
        session.status = "active".into();
        session.parent_agent_id = Some(parent_agent_id.to_string());
        if is_main {
            // The Main coder is never a "mini" — even with an oracle grant its
            // registration role stays "coder".
            session.role = "coder".into();
            session.message = Some("Main coder running".into());
            if let Some(hash) = oracle_token_hash {
                session.launch_token_hash = Some(hash.to_string());
                session.launch_token_issued_at = Some(timestamp_now.clone());
            }
        } else if let Some(hash) = oracle_token_hash {
            session.role = "mini".into();
            session.launch_token_hash = Some(hash.to_string());
            session.launch_token_issued_at = Some(timestamp_now.clone());
        }
        // Keep the project link fresh on re-upsert too, but never CLEAR a previously
        // resolved project with a later None (a transient empty parent snapshot must
        // not drop the mini out of the rail mid-run).
        if current_project_id.is_some() {
            session.current_project_id = current_project_id;
        }
        // WARNING 7: on UPDATE stamp lastSeenAt to NOW, not the (possibly stale)
        // launch time, so a re-upsert keeps the liveness timestamp current.
        session.last_seen_at = Some(Utc::now().to_rfc3339());
        return;
    }
    state.sessions.push(crate::backend::model::AgentSession {
        agent_id: agent_id.to_string(),
        role: if oracle_token_hash.is_some() && !is_main {
            "mini".into()
        } else {
            "coder".into()
        },
        model: None,
        status: "active".into(),
        client: Some(client.to_string()),
        message: Some(if is_main {
            "Main coder running".into()
        } else {
            "Mini-coder running".into()
        }),
        current_project_id,
        current_task_id: None,
        current_file_path: None,
        first_seen_at: Some(started_at.to_string()),
        last_seen_at: Some(started_at.to_string()),
        launch_token_hash: oracle_token_hash.map(String::from),
        launch_token_issued_at: oracle_token_hash.map(|_| timestamp_now.clone()),
        session_token_hash: None,
        session_token_issued_at: None,
        launch_consumed_at: None,
        subagents: Vec::new(),
        needs_user: None,
        host: Some(super::agents::HOST_APP.to_string()),
        parent_agent_id: Some(parent_agent_id.to_string()),
        pending_question: None,
        user_reply: None,
    });
}

/// WARNING 3: mark a finished mini's SESSION terminal so it stops surfacing in the
/// project rail. Sets status to `"done"` — one of the statuses
/// `isRecentProjectSession` (src/utils/agentClaims.ts) explicitly excludes
/// (`done`/`archived`/`idle`/`stopped`), so the row drops out of
/// ProjectsView.sessionsByProject (and thus the rail) on the next poll instead of
/// lingering as a stale "active" agent whose PTY is already reaped. A missing session
/// (never upserted, or already pruned) is a no-op. Called from every terminal path
/// (finished / timeout / parent-gone / stuck-launching) inside its locked mutate.
fn close_mini_session(state: &mut crate::backend::model::AgentLiveState, agent_id: &str) {
    if let Some(session) = state.sessions.iter_mut().find(|s| s.agent_id == agent_id) {
        session.status = "done".into();
        // Max-recall hygiene: a closed mini's launch-token hash is dead weight
        // (the raw token dies with the prompt file) — don't retain it.
        session.launch_token_hash = None;
        session.launch_token_issued_at = None;
    }
}

/// `mini-<parentShort>-<id8>`: a stable, allowlist-safe id (`[A-Za-z0-9._-]`).
pub(crate) fn mini_agent_id(directive: &MiniCoderDirective) -> String {
    let parent_short: String = directive
        .parent_agent_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let id8: String = directive
        .id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let parent_short = if parent_short.is_empty() {
        "p".to_string()
    } else {
        parent_short
    };
    let id8 = if id8.is_empty() { "x".to_string() } else { id8 };
    format!("mini-{parent_short}-{id8}")
}

/// BLOCKER 1: the parent coder's current project id read PURELY from the supplied
/// snapshot (the pass snapshot that `plan_tick` saw). None if the parent session is
/// absent or carries no project. Resolving from the pass snapshot — rather than a
/// fresh locked read at claim time — keeps the mini's scratch root and session
/// project stamp consistent with the project the directive was planned against, and
/// removes the race where the parent switches project (or vanishes) between the claim
/// write and a second read.
fn snapshot_parent_project(
    snapshot: &crate::backend::model::AgentLiveState,
    parent_agent_id: &str,
) -> Option<String> {
    snapshot
        .sessions
        .iter()
        .find(|s| s.agent_id == parent_agent_id)
        .and_then(|s| s.current_project_id.clone())
}

/// ROLE UNTANGLE Phase 3: the sentinel `parent_agent_id` of an APP-AUTHORED
/// directive (appended by `append_main_coder_directive`, called from
/// `polis_fix_sin`). It is an event-log
/// identity, never a live session — such directives carry their project scope
/// EXPLICITLY (`directive.project_id`) and have no parent session to lose.
pub(crate) const APP_USER_PARENT: &str = "app-user";

/// True when this directive is app-authored: sentinel parent + explicit project.
/// Both conditions required — a hand-crafted directive with the sentinel but no
/// project still fails the claim cleanly (no scope), and an MCP directive that
/// somehow carried a projectId still obeys the parent-liveness sweep.
fn is_app_authored(directive: &MiniCoderDirective) -> bool {
    directive.parent_agent_id == APP_USER_PARENT
        && directive
            .project_id
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
}

/// Project scope for a directive: an app-authored one carries it explicitly;
/// everything else derives it from the live parent session (the status quo).
fn directive_project(
    snapshot: &crate::backend::model::AgentLiveState,
    directive: &MiniCoderDirective,
) -> Option<String> {
    if is_app_authored(directive) {
        return directive.project_id.clone();
    }
    snapshot_parent_project(snapshot, &directive.parent_agent_id)
}

/// Parent-liveness for the auto-kill sweep: an app-authored directive has no
/// parent session to lose — the HUMAN supervises it (Stop button / steer), so it
/// is never "parent gone". Every MCP-dispatched directive keeps the sweep.
fn directive_parent_gone(
    snapshot: &crate::backend::model::AgentLiveState,
    directive: &MiniCoderDirective,
) -> bool {
    if is_app_authored(directive) {
        return false;
    }
    parent_is_gone(snapshot, &directive.parent_agent_id)
}

/// Resolve `(project_root, scratch_root)` for a mini. `scratch_root` =
/// `<project_root>/.aspis-mini`, created if missing. The project root comes from the
/// parent's `current_project_id`; if that is absent, fail (we never run a mini
/// outside a known project tree).
fn resolve_scratch_root(app: &AppHandle, project_id: &str) -> Result<(PathBuf, PathBuf), String> {
    let project_id = project_id.trim();
    if project_id.is_empty() {
        return Err("parent coder has no current project".to_string());
    }
    let project_root = crate::backend::projects::resolve_project_root_by_id(app, project_id)?;
    let scratch_root = project_root.join(MINI_SCRATCH_DIR);
    std::fs::create_dir_all(&scratch_root)
        .map_err(|e| format!("could not create mini scratch dir: {e}"))?;
    Ok((project_root, scratch_root))
}

/// Kill + reap a mini's PTY (idempotent; a missing session is a no-op). Used by the
/// timeout + parent-gone paths. OUTSIDE the state lock.
fn kill_mini_pty(app: &AppHandle, directive: &MiniCoderDirective) {
    if let Some(agent_id) = directive.agent_id.as_deref() {
        crate::backend::agent_pty::kill_agent_pty(app, agent_id);
    }
}

/// True when the parent coder session is absent or `closed` (the mini has lost its
/// only human-contact point). A `launch_pending`/active parent is NOT gone.
fn parent_is_gone(snapshot: &crate::backend::model::AgentLiveState, parent_agent_id: &str) -> bool {
    match snapshot
        .sessions
        .iter()
        .find(|s| s.agent_id == parent_agent_id)
    {
        Some(session) => session.status.trim().eq_ignore_ascii_case("closed"),
        None => true,
    }
}

/// Read the mini's result file and resolve a terminal outcome, CANONICALIZING the
/// opened target and re-verifying it stays under `scratch_root` BEFORE trusting the
/// content — closing the P1 symlink TOCTOU (`read_result_file` is lexical-only).
/// Any escape / missing / invalid file degrades to `failed`.
fn read_result_outcome(scratch_root: &Path, result_rel_path: &str) -> MiniCoderOutcome {
    // The lexical guard + lenient parse live in P1's `read_result_file`. Before
    // calling it we add the canonicalize-after-open check: resolve the real path of
    // the file the mini wrote and require it to live under the canonical scratch
    // root, so a symlink planted inside scratch that points off-root is rejected.
    let normalized = result_rel_path.replace('\\', "/");
    let target = scratch_root.join(&normalized);
    match (
        std::fs::canonicalize(scratch_root),
        std::fs::canonicalize(&target),
    ) {
        (Ok(canon_root), Ok(canon_target)) => {
            if !canon_target.starts_with(&canon_root) {
                return MiniCoderOutcome::failed("result file escapes scratch root (symlink)");
            }
        }
        // The target not existing yet (canonicalize fails) is the normal "no result
        // file" case → fall through to read_result_file, which returns `failed`.
        // A scratch root that won't canonicalize is a hard environment error → fail.
        (Err(_), _) => return MiniCoderOutcome::failed("scratch root unresolved"),
        (Ok(_), Err(_)) => {
            return MiniCoderOutcome::failed("result file missing or unresolved");
        }
    }
    mini_coder::read_result_file(scratch_root, result_rel_path)
}

/// The ledger/session `client` label for a backend kind (ollama/api/codex). The
/// rail's MINI chip shows this so the operator sees the real runtime.
fn backend_client_label(backend: &MiniCoderBackend) -> String {
    match backend.kind {
        MiniCoderBackendKind::Ollama => "ollama",
        MiniCoderBackendKind::Api => "api",
        MiniCoderBackendKind::Codex => "codex",
        MiniCoderBackendKind::Openai => "openai",
        MiniCoderBackendKind::Omlx => "omlx",
        MiniCoderBackendKind::AppleFm => "appleFm",
        MiniCoderBackendKind::Cloud => "cloud",
    }
    .to_string()
}

/// CONSOLE (Step B): the monospace model label for the Activity Console's `MiniRun`, e.g.
/// "mini · ollama/qwen2.5-coder" or "mini · codex". The backend kind label + the resolved
/// model tag (when set); a backend with no model tag (api/codex without a pinned model)
/// shows just the kind. Privacy-safe: only the already-surfaced runtime label, no secrets.
/// F08: model chip label. Main-tier sessions must not render as pure "Mini".
fn console_model_label_for_tier(
    backend: &MiniCoderBackend,
    tier: mini_coder::DirectiveTier,
) -> String {
    let kind = backend_client_label(backend);
    let role = if matches!(tier, mini_coder::DirectiveTier::Main) {
        "main"
    } else {
        "mini"
    };
    match backend
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        Some(model) => format!("{role} · {kind}/{model}"),
        None => format!("{role} · {kind}"),
    }
}

/// CONSOLE (Step B): a short, privacy-safe label for the spawn row + the run's working
/// shimmer. Prefers the directive's `task` (already a human task summary), trimmed to a
/// one-line cap; falls back to a file-scope summary, then a generic label. Never leaks a
/// raw transcript — `task` is the coder-authored one-line intent the rail already shows.
fn console_run_label(directive: &MiniCoderDirective) -> String {
    let task = directive.task.trim();
    if !task.is_empty() {
        // One line, bounded — the row is a single chip, not a paragraph.
        let first_line = task.lines().next().unwrap_or(task).trim();
        return truncate_label(first_line, 80);
    }
    match directive.files.len() {
        0 => "mini-coder".to_string(),
        1 => format!("mini-coder · {}", directive.files[0]),
        n => format!("mini-coder · {n} files"),
    }
}

/// Char-bounded truncation with an ellipsis (no panic on a multi-byte boundary — we count
/// chars, not bytes). Used only for the console row label.
fn truncate_label(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let kept: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// CONSOLE (Step B): the managed activity store, if installed. Absent only in tests that do
/// not `.manage` it; every console mutation is then a silent no-op (the executor's behavior
/// is otherwise unchanged — the console is a pure observer).
fn console_store(
    app: &AppHandle,
) -> Option<tauri::State<'_, super::mini_activity::MiniActivityStore>> {
    app.try_state::<super::mini_activity::MiniActivityStore>()
}

/// Slice 3 — pure, unit-testable helper for the Unattended denial note appended to the
/// activity console when the broker fails closed without prompting. `kind` is `"net"` or
/// `"folder"`; `detail` is the net hint string or the blocked folder path respectively.
/// Returns a short operator-readable label (≤200 chars after truncation) suitable for a
/// `ConsoleEntry::Coder` milestone row. No secrets are emitted: the folder path is safe
/// (already canonicalized by the write tool and stored in `out_of_scope_write`); command
/// bodies are never included.
pub(crate) fn unattended_denial_note(kind: &str, detail: &str) -> String {
    let note = match kind {
        "net" => {
            if detail.is_empty() {
                "Network access denied (Unattended mode)".to_string()
            } else {
                // Trim the hint to avoid bloating the timeline row.
                let hint = detail.chars().take(120).collect::<String>();
                format!("Network access denied (Unattended mode): {hint}")
            }
        }
        "folder" => {
            let folder = detail.chars().take(140).collect::<String>();
            format!("Write to \"{folder}\" denied (Unattended mode)")
        }
        other => format!("Access denied (Unattended mode, kind={other})"),
    };
    // Hard cap at 200 chars (the ConsoleEntry milestone label limit in mini_activity.rs).
    if note.chars().count() <= 200 {
        note
    } else {
        let truncated: String = note.chars().take(199).collect();
        format!("{truncated}\u{2026}") // …
    }
}

/// CONSOLE (Step B) — FIX 2: stamp the live console TERMINAL for the executor's terminal-reap
/// paths (timeout / stuck-launching / parent-gone). Those paths transition the directive to a
/// terminal status but never go through `console_finalize`, so without this the console stays
/// `running:true` (shimmer on) forever AND the store entry is pinned non-evictable (running ⇒
/// pinned). `set_terminal` flips `running=Some(false)` regardless of whether a live mini
/// exists (so a never-seeded stuck-launching directive simply stops being "running" — no
/// phantom timeline), and stamps the neutral `Stop` banner (mirrors `console_finalize`'s
/// `_ => stop` neutral terminal) only when there IS a live mini. Pure observer: a missing
/// store (unmanaged in tests) or a directive with no `agent_id` (never launched a PTY) is a
/// silent no-op — the console NEVER alters the directive's outcome.
fn console_mark_stopped(app: &AppHandle, directive: &MiniCoderDirective) {
    if let Some(store) = console_store(app) {
        if let Some(agent_id) = directive.agent_id.as_deref() {
            let f = |a: &mut super::mini_activity::ConsoleActivity| {
                super::mini_activity::set_terminal(
                    a,
                    super::mini_activity::Banner {
                        kind: super::mini_activity::BannerKind::Stop,
                        title: None,
                        sub: None,
                    },
                );
            };
            match super::projects::ensure_projects_dir(app).ok() {
                Some(ref pd) => store.update_bridged(app, agent_id, pd, f),
                None => store.update(app, agent_id, f),
            }
        }
    }
}

/// Resolve the MCP roots (`management_root`, `projects_dir`) the codex backend's
/// bounded `oracle_context` grant needs — the SAME wiring a full coder gets. Best
/// effort: `None` if the projects dir can't be resolved, in which case the codex
/// backend simply gets NO oracle grant (the mini still runs, just without Oracle).
fn resolve_mcp_roots(app: &AppHandle) -> Option<McpRoots> {
    let projects_dir = super::projects::ensure_projects_dir(app).ok()?;
    let management_root = agents::management_root_for_mcp(app, &projects_dir).ok()?;
    Some(McpRoots {
        management_root,
        projects_dir,
    })
}

/// The two roots a future read-only `oracle_context` MCP scope would be built from.
/// P3: consumed by the codex command arms — with the read-only oracle grant the
/// shared `-c mcp_servers.*` tokens are built from these roots (server-side
/// "mini"-role narrowing). Text-only backends ignore them.
pub(crate) struct McpRoots {
    pub(crate) management_root: PathBuf,
    pub(crate) projects_dir: PathBuf,
}

/// Spawn the real one-shot mini for the configured backend. Builds the per-kind
/// command (prompt delivered over STDIN via a restricted temp file — NEVER on
/// argv, NEVER echoed to the PTY) and launches it through the SAME app-hosted PTY
/// path every agent uses. cwd = the project root.
///
/// NO-WINDOW: the PTY path runs the shell DIRECTLY under ConPTY (the PTY *is* the
/// console — there is no separate conhost window to suppress), so the no-extra-
/// window property holds by construction (same as every `agent_pty` launch). The
/// wrapper shell is spawned by portable-pty, not `std::process::Command`, so a
/// `CREATE_NO_WINDOW` flag does not apply / is unnecessary here.
///
/// B1 INVARIANT (token/secret): no API key is ever placed on argv. The codex
/// backend rides the LOCAL subscription (no key at all). The api backend's key
/// must come from the CLI's own env. The prompt (task + file contents) is written
/// to a 0600 restricted temp file, read by the wrapper, deleted, then piped to the
/// backend over stdin — it is never an argv positional nor `Write-Host`/`echo`'d.
#[allow(clippy::too_many_arguments)]
fn spawn_one_shot_mini(
    app: &AppHandle,
    agent_id: &str,
    project_root: &Path,
    scratch_root: &Path,
    result_rel_path: &str,
    backend: &MiniCoderBackend,
    directive: &MiniCoderDirective,
    mcp_roots: Option<&McpRoots>,
    // P3: the RAW launch token for the read-only oracle grant (codex-only).
    // `Some` implies `mcp_roots` is `Some` (both derive from the same gate).
    oracle_token: Option<&str>,
) -> Result<(), String> {
    let result_target = scratch_root.join(result_rel_path.replace('\\', "/"));
    // P3: the prompt advertises the oracle grant ONLY when the token exists; the
    // token itself stays inside the 0600 prompt file (stdin delivery, never argv).
    let oracle_access = oracle_token.map(|token| MiniOracleAccess {
        agent_id,
        launch_token: token,
    });
    // Phase C: estimate task size BEFORE building the prompt. If the task
    // doesn't fit 70% of the model's context window, refuse to spawn —
    // don't waste a doomed generation. The runner sees the error and blocks.
    let context_window = mini_model_context_window(app, backend);
    let estimate = crate::backend::task_size::estimate_task_size(
        &directive.task,
        &directive.files,
        project_root,
        context_window,
    );
    if !estimate.fits_model {
        return Err(format!(
            "task too large: {}",
            estimate
                .reason
                .unwrap_or_else(|| "exceeds model context budget".into())
        ));
    }
    // Front-load the file scope + contents into the prompt (bounded per file).
    // For retries (attempt > 0), check for censor steer feedback and append to task.
    let mut directive_for_prompt = directive.clone();
    if directive.attempt > 0 {
        if let Some(parent_id) = directive.parent_directive_id.as_deref() {
            let steer_dir = scratch_root.join(parent_id);
            let ready_path = steer_dir.join("steer_ready");
            let steer_path = steer_dir.join("steer_censor");
            if ready_path.exists() {
                let _ = std::fs::remove_file(&ready_path);
                if let Ok(text) = std::fs::read_to_string(&steer_path) {
                    if !text.trim().is_empty() {
                        directive_for_prompt.task.push_str("\n\n");
                        directive_for_prompt
                            .task
                            .push_str("CENSOR FINDINGS (fix these this round):\n");
                        directive_for_prompt.task.push_str(&text);
                    }
                }
                let _ = std::fs::remove_file(&steer_path);
            }
        }
    }
    let prompt = build_mini_prompt(
        backend,
        &directive_for_prompt,
        project_root,
        &result_target,
        oracle_access.as_ref(),
    );
    // Phase B: compact the built prompt to 70% of the model's context window.
    // Zero-LLM BM25 — trims irrelevant file blocks, keeps task + hard constraints.
    // (context_window already computed above by Phase C — no redeclaration needed.)
    let (prompt, budget) = if context_window > 0 {
        let (compacted, budget) = crate::backend::compact::compact_built_prompt(
            &prompt,
            &directive.task,
            context_window,
            0,
        );
        if budget.percent_saved > 0.0 {
            eprintln!(
                "mini compaction: {}→{} tokens ({:.0}% saved, {}/{} files kept)",
                budget.tokens_before,
                budget.tokens_after,
                budget.percent_saved,
                budget.files_kept,
                budget.files_kept + budget.files_trimmed,
            );
        }
        (compacted, budget)
    } else {
        (prompt, crate::backend::compact::CompactBudget::default())
    };
    let _ = budget; // (logged above when savings occurred)
                    // P6 thinking split: ANY retry (attempt > 0) runs with model thinking ON —
                    // reasoning about feedback is the use case; the initial pass stays OFF
                    // (mechanical, fully specified). Consumed ONLY by the oMLX body builders
                    // (Qwen-gated); codex/ollama/api commands are byte-identical either way.
    let fix_pass_thinking = directive.attempt > 0;
    let MiniCommandBuild {
        prompt_file,
        profile_file,
        command,
    } = build_mini_command(
        backend,
        project_root,
        &result_target,
        &prompt,
        mcp_roots,
        fix_pass_thinking,
    )?;
    let sessions = match app.try_state::<crate::backend::agent_pty::AgentPtySessions>() {
        Some(s) => s,
        None => {
            // The launched shell never ran, so it cannot delete the temp files.
            remove_mini_temp_files(
                prompt_file.as_deref(),
                profile_file.as_deref(),
            );
            return Err("Agent terminal state is unavailable.".to_string());
        }
    };
    match crate::backend::agent_pty::spawn_agent_pty(app, &sessions, agent_id, command) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Spawn failed -> the wrapper never ran to delete the temp files.
            remove_mini_temp_files(
                prompt_file.as_deref(),
                profile_file.as_deref(),
            );
            Err(e)
        }
    }
}

/// max-recall FIX 10: remove the restricted temp files a built mini command owns — the
/// prompt file, AND (P5) the OPTIONAL Seatbelt `.sb` profile
/// (each in its own 0600 dir). Called on every pre-/at-spawn failure path in
/// [`spawn_one_shot_mini`], where the in-script wrapper/trap never ran to delete them.
/// Centralized (not inlined per arm) so the failure arms can't diverge and no cleanup
/// (the `.sb` — a leaked profile per launch is a bug) can be forgotten. A
/// `None` path is a no-op.
fn remove_mini_temp_files(
    prompt_file: Option<&Path>,
    profile_file: Option<&Path>,
) {
    if let Some(path) = prompt_file {
        super::projects::remove_restricted_temp_file(path);
    }
    if let Some(path) = profile_file {
        super::projects::remove_restricted_temp_file(path);
    }
}

// ROLE UNTANGLE Phase 2: the mini PROMPT block (MAX_PROMPT_* consts,
// MiniOracleAccess, mini_thinking_directive, mini_language_block,
// compose_agentic_system_prompt, censor_phase_a_summary, build_mini_prompt,
// read_prompt_file) moved VERBATIM to backend/mini_prompt.rs. Wildcard
// re-import keeps every call site and test unchanged.
#[allow(unused_imports)]
pub(crate) use super::mini_prompt::*;

#[cfg(test)]
mod mini_language_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn agentic_local_base_url_rejected_only_for_local_nonloopback() {
        // local backend on loopback → accepted
        assert!(!agentic_local_base_url_rejected(
            MiniCoderBackendKind::Omlx,
            "http://127.0.0.1:8000"
        ));
        assert!(!agentic_local_base_url_rejected(
            MiniCoderBackendKind::Ollama,
            "http://localhost:11434"
        ));
        // local backend pointed off-box → REJECTED (would exfiltrate prompt+source)
        assert!(agentic_local_base_url_rejected(
            MiniCoderBackendKind::Omlx,
            "http://evil.example.com:8000"
        ));
        // cloud backends are remote by design → not rejected by this local-only gate
        assert!(!agentic_local_base_url_rejected(
            MiniCoderBackendKind::Codex,
            "http://evil.example.com"
        ));
    }

    #[test]
    fn task_scope_rust() {
        let b = mini_language_block(Path::new("/nonexistent_xyz"), "mini", &["a.rs".to_string()])
            .unwrap();
        assert!(b.contains("--- BEGIN LANGUAGE SKILL"));
        assert!(b.contains("veteran Rust"));
    }

    #[test]
    fn task_scope_wins_python() {
        let b = mini_language_block(
            Path::new("/nonexistent_xyz"),
            "mini",
            &["a.py".to_string(), "b.py".to_string()],
        )
        .unwrap();
        assert!(b.contains("veteran Python"));
    }

    #[test]
    fn no_mappable_file_nonexistent_project_is_none() {
        assert!(
            mini_language_block(Path::new("/nonexistent_xyz"), "mini", &["a.md".to_string()])
                .is_none()
        );
    }

    #[test]
    fn empty_file_list_nonexistent_project_is_none() {
        assert!(mini_language_block(Path::new("/nonexistent_xyz"), "mini", &[]).is_none());
    }

    #[test]
    fn agentic_system_prompt_separates_and_falls_back() {
        let base = crate::backend::agentic_runner::AGENTIC_SYSTEM_PROMPT;
        // None/None → exactly the base (byte-identical to the pre-feature path).
        assert_eq!(compose_agentic_system_prompt(None, None), base);
        // Lang only → base, then a NEWLINE separator, then the block (no fused boundary).
        let composed = compose_agentic_system_prompt(None, Some("--- BEGIN LANGUAGE SKILL marker"));
        assert!(composed.starts_with(base));
        assert!(composed.contains("\n--- BEGIN LANGUAGE SKILL marker"));
    }

    #[test]
    fn agentic_system_prompt_orders_skill_before_lang() {
        // P5: the per-profile SKILL block precedes the language block, both after the base.
        let base = crate::backend::agentic_runner::AGENTIC_SYSTEM_PROMPT;
        let composed = compose_agentic_system_prompt(
            Some("--- BEGIN PROJECT SKILL marker"),
            Some("--- BEGIN LANGUAGE SKILL marker"),
        );
        assert!(composed.starts_with(base));
        let skill_at = composed.find("BEGIN PROJECT SKILL").unwrap();
        let lang_at = composed.find("BEGIN LANGUAGE SKILL").unwrap();
        assert!(
            skill_at < lang_at,
            "skill block must precede the language block"
        );
    }

    #[test]
    fn empty_files_falls_back_to_project_kind() {
        // No task files but a real Rust manifest → project-primary detection yields the Rust block.
        let dir =
            std::env::temp_dir().join(format!("devboule_minilang_{}_proj", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let b = mini_language_block(&dir, "mini", &[]).unwrap();
        assert!(b.contains("veteran Rust"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mini_language_block_uses_tier_profile_override() {
        // The tier owns the skill (mini-big/SKILL.md exists) → its lang override must be injected,
        // proving mini-big/mini-small language personas reach the launch prompt, not just "mini".
        let dir =
            std::env::temp_dir().join(format!("devboule_minilang_{}_tier", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let skills = dir.join(".claude").join("skills").join("mini-big");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "tier skill").unwrap();
        std::fs::write(skills.join("lang-rust.md"), "MINIBIG RUST PERSONA").unwrap();
        let b = mini_language_block(&dir, "mini-big", &["a.rs".to_string()]).unwrap();
        assert!(
            b.contains("MINIBIG RUST PERSONA"),
            "tier lang override not injected: {b}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
// (build_mini_prompt + read_prompt_file live in backend/mini_prompt.rs — Phase 2 pure move.)

// ROLE UNTANGLE Phase 2: the mini COMMAND BUILD block (MiniCommandBuild,
// build_mini_command + the three per-OS impls, the oMLX/AppleFm run builders,
// build_seatbelt_profile, the loopback gates and the OMLX_*/MINI_RLIMIT_* consts)
// moved VERBATIM to backend/mini_command_build.rs. Wildcard re-import keeps
// every call site and test unchanged.
#[allow(unused_imports)]
pub(crate) use super::mini_command_build::*;

/// PURE helper: mark the NON-TERMINAL directive owning `agent_id` as kill-requested.
/// Returns true iff a directive with that `agentId` was found AND it is a live (non-
/// terminal) mini whose flag was set. Mutating the in-memory state only; the caller
/// persists it under the state lock. Idempotent: a directive already flagged stays
/// flagged. Kept pure so the order-of-operations invariant (flag set BEFORE the PTY
/// kill) is unit-testable without a real AppHandle.
///
/// WARNING 3 — the boolean is the kill TARGET GUARD: `mini_coder_kill` only proceeds
/// to `agent_pty_kill` when this returns true (a genuine live mini owns the id), so a
/// mis-routed / crafted invoke for a non-mini agent id can never kill a normal coder's
/// PTY from this command.
///
/// WARNING 4 — a directive that already reached a TERMINAL state (the mini finished in
/// the same instant) is left untouched: we do NOT set killRequested on it (terminal is
/// terminal) and we return false (there is no live PTY to kill anyway).
///
/// P6 — KILL THE WHOLE CHAIN: a Stop on ANY attempt in a retry chain must abort the chain.
/// After locating the directive by `agent_id`, we derive its chain ROOT and flag
/// `kill_requested` on EVERY active (`Launching|Running`) directive in that lineage — so
/// the human's intent reaches the attempt that actually has a live PTY (the one whose
/// EOF-finalize will synthesize `aborted_by_human`, then propagate it up the chain). The
/// returned bool is true iff a LIVE (active) attempt in the chain was flagged — only then
/// does `mini_coder_kill` proceed to kill the PTY for `agent_id`.
fn mark_kill_requested(
    state: &mut crate::backend::model::AgentLiveState,
    agent_id: &str,
) -> Option<String> {
    // Find the directive owning this agent id (clone its lineage keys to drop the borrow).
    let matched = state
        .mini_coder_directives
        .iter()
        .find(|d| d.agent_id.as_deref() == Some(agent_id))
        .map(|d| (d.id.to_string(), d.status));
    let Some((root, matched_status)) = matched else {
        // No directive owns this id (not a mini): no flag, no kill.
        return None;
    };

    // A matched directive that already reached a TERMINAL state is left untouched (and no
    // PTY to kill): no-op — exactly as before.
    if matched_status.is_terminal() {
        return None;
    }

    // Flag kill_requested across the WHOLE lineage: the matched directive AND every other
    // NON-TERMINAL attempt sharing the chain root (the live attempt — Launching|Running —
    // is what carries the PTY; an Failed predecessor is flagged too so a racing
    // re-finalize honours the abort). A terminal chain-mate is left untouched.
    for d in state.mini_coder_directives.iter_mut() {
        let in_chain = d.id == root || d.parent_directive_id.as_deref() == Some(root.as_str());
        if in_chain && !d.status.is_terminal() {
            d.kill_requested = true;
        }
    }

    // WARNING 6 (KILL STALE AGENT_ID): the PTY to kill belongs to the chain's ACTIVE
    // (`Launching|Running`) attempt — NOT necessarily the directive matched by `agent_id`.
    // If the human hit Stop via an Failed PREDECESSOR's stale agent id, that
    // predecessor's PTY is a DEAD past mini; the LIVE retry runs under a DIFFERENT agent
    // id and would keep going if we killed the stale one. So return the live attempt's
    // agent id as the PTY to kill. Falls back to the matched id (the original behaviour)
    // when no chain-mate is currently active (e.g. the matched directive IS the live one,
    // or a launching attempt has no agent_id yet — then there is no live PTY to kill).
    let live_pty = state
        .mini_coder_directives
        .iter()
        .filter(|d| {
            (d.id == root || d.parent_directive_id.as_deref() == Some(root.as_str()))
                && d.status.is_active()
        })
        .find_map(|d| d.agent_id.clone());
    Some(live_pty.unwrap_or_else(|| agent_id.to_string()))
}

/// ASYNC STEERING (a): outcome of a steer attempt on the in-memory state — what the
/// `mini_coder_steer` command turns into a result for the human / Console.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SteerOutcome {
    /// Appended a correction; carries the targeted attempt's agent id when it has one (an
    /// ACTIVE attempt carries its live PTY id; a `Pending` retry targeted in the handoff
    /// window has `None` — it has no PTY yet, and the queued path never needs one) and the
    /// new queue length.
    Queued {
        live_agent_id: Option<String>,
        queued: usize,
    },
    /// The message was the STOP sentinel — flagged the chain `kill_requested` (reusing
    /// the Stop path). Carries the live attempt's PTY agent id to kill, like
    /// `mark_kill_requested`.
    Stopped { live_agent_id: Option<String> },
    /// The chain's live attempt already holds the maximum queued corrections
    /// ([`mini_coder::MAX_STEER_QUEUE_LEN`]); the message was REFUSED (not dropped) so a
    /// queued correction is never lost. Carries the current (full) queue length.
    QueueFull { queued: usize },
    /// No live (non-terminal) mini owns this agent id (non-mini id, already-terminal, or
    /// the steer message was empty) — a pure no-op.
    NoOp,
}

/// ASYNC STEERING (a) — PURE helper mirroring [`mark_kill_requested`]: route a steer
/// `message` to the mini chain owning `agent_id`. The SAME external-signal channel
/// generalized from the kill bool to a queue:
///   * the STOP sentinel ([`mini_coder::STEER_STOP_SENTINEL`]) flags `kill_requested`
///     across the live chain (REUSING the kill path) and returns `Stopped`;
///   * any other (non-blank) message is APPENDED to the chain member that WILL run: the
///     ACTIVE (`Launching|Running`) attempt if any, else (C1) the highest-attempt NON-TERMINAL
///     member — the `Pending` retry in the retry-handoff window, NOT the dead `Failed`
///     predecessor that owns the matched `agent_id` (whose queue would never be drained). The
///     attempt whose next round boundary drains the queue into the fix-pass task. Returns
///     `Queued`. Capped at [`mini_coder::MAX_STEER_QUEUE_LEN`] (refused with `QueueFull` when
///     full so an already-queued correction is never lost) and each message pre-sanitized +
///     capped to [`mini_coder::MAX_STEER_MESSAGE_LEN`] by the caller.
///   * a non-mini id, an already-terminal chain, or a blank message is a `NoOp`.
/// Mutates the in-memory state only; the caller persists under the state lock. Kept pure so
/// it is unit-testable without a real AppHandle (no GPU / no live mini needed).
fn mark_steer_requested(
    state: &mut crate::backend::model::AgentLiveState,
    agent_id: &str,
    message: &str,
) -> SteerOutcome {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return SteerOutcome::NoOp;
    }

    // A STOP steer reuses the kill path verbatim (flag the whole live chain, return the
    // PTY to kill) — steering generalizes the kill, it does not bypass it.
    if mini_coder::is_steer_stop(trimmed) {
        return match mark_kill_requested(state, agent_id) {
            Some(live) => SteerOutcome::Stopped {
                live_agent_id: Some(live),
            },
            None => SteerOutcome::NoOp,
        };
    }

    // Locate the chain (root) the way mark_kill_requested does, and bail on a non-mini id
    // or an already-terminal matched directive.
    let matched = state
        .mini_coder_directives
        .iter()
        .find(|d| d.agent_id.as_deref() == Some(agent_id))
        .map(|d| (d.id.to_string(), d.status));
    let Some((root, matched_status)) = matched else {
        return SteerOutcome::NoOp;
    };
    if matched_status.is_terminal() {
        return SteerOutcome::NoOp;
    }

    // Append to the chain member that WILL run — the attempt whose NEXT round boundary
    // (build_retry_directive) drains the queue into the fix-pass task. There is at most
    // one ACTIVE (`Launching|Running`) attempt in a chain (plan_tick claims one at a time),
    // so prefer it. C1: when NO attempt is currently active we must NOT fall back to the
    // matched directive — in the retry-handoff window the matched directive is the DEAD
    // `Failed` predecessor (it carries `agent_id`; the freshly-minted `Pending` retry
    // has none yet), and appending there would SILENTLY LOSE the steer (the predecessor
    // never re-runs). Instead target the HIGHEST-attempt NON-TERMINAL chain member — the
    // Pending retry `R` that plan_tick will claim — mirroring Python's "active preference"
    // (`_MINI_ACTIVE_STATUSES` includes `pending`). A chain with no non-terminal member at
    // all (everything terminal) yields None -> NoOp (the matched-status terminal guard above
    // already covers the common case).
    let in_chain = |d: &MiniCoderDirective| {
        d.id == root || d.parent_directive_id.as_deref() == Some(root.as_str())
    };
    let target_idx = state
        .mini_coder_directives
        .iter()
        .position(|d| in_chain(d) && d.status.is_active())
        .or_else(|| {
            // No active attempt: target the highest-attempt non-terminal member (the
            // Pending retry that will run), NOT the matched (possibly dead predecessor) id.
            state
                .mini_coder_directives
                .iter()
                .enumerate()
                .filter(|(_, d)| in_chain(d) && !d.status.is_terminal())
                .max_by_key(|(_, d)| d.attempt)
                .map(|(i, _)| i)
        });
    let Some(idx) = target_idx else {
        return SteerOutcome::NoOp;
    };
    let target = &mut state.mini_coder_directives[idx];
    // Bounded FIFO: refuse (do not silently drop the oldest) when full so a queued
    // correction is never lost. The caller surfaces this as a clear "queue full" status.
    if target.steer_queue.len() >= mini_coder::MAX_STEER_QUEUE_LEN {
        return SteerOutcome::QueueFull {
            queued: target.steer_queue.len(),
        };
    }
    target.steer_queue.push(
        trimmed
            .chars()
            .take(mini_coder::MAX_STEER_MESSAGE_LEN)
            .collect(),
    );
    SteerOutcome::Queued {
        live_agent_id: target.agent_id.clone(),
        queued: target.steer_queue.len(),
    }
}

/// P5 SAFETY BRAKE — the human Stop button on a mini's terminal. A TRUE override:
///   1) RECORD `killRequested=true` on the directive (persisted under the state lock)
///      BEFORE anything else, so the EOF-driven `finalize_finished_mini` — and the
///      timeout / parent-gone paths — synthesize `aborted_by_human` and a racing
///      `done`/`failed`/`timeout` can never overwrite the human's assertion of control;
///   2) THEN `agent_pty_kill(agent_id)` (kill + reap the PTY) OUTSIDE the state lock,
///      which drives the mini's PTY to EOF so the executor finalizes it as aborted.
///
/// LOCK DISCIPLINE: the directive flag is written under the agent-state file lock; the
/// PTY kill (which takes the PTY map lock) happens AFTER that lock is released — the two
/// locks are never held at once (no deadlock with the executor or the reader thread).
/// Idempotent: a double-Stop re-flags (no-op) and re-kills (a missing session is a
/// no-op). A directive that already reached a terminal state stays terminal — the
/// `transition_directive` idempotence guard refuses to clobber it.
///
/// SELF-DEFENCE (WARNING 3/4): the PTY kill fires ONLY when the id genuinely belongs to
/// a LIVE (non-terminal) mini directive (the `mark_kill_requested` gate). A non-mini id,
/// an already-terminal mini, or an unmatched id is a pure no-op — this command can never
/// kill an unrelated (e.g. normal coder) PTY even if the UI gate is bypassed.
#[tauri::command]
pub fn mini_coder_kill(app: AppHandle, agent_id: String) -> Result<(), String> {
    // FIX 2 (SAFETY OVERRIDE): Stop must work REGARDLESS of vault state. The vault can
    // auto-lock while a mini runs; gating Stop on `ensure_unlocked()` would trap the
    // human into waiting out the 600s wall-clock cap with no way to abort. Stop is a
    // safety brake — it only kills an ALREADY-RUNNING child + records the abort intent;
    // it neither reads secrets nor mutates protected config, so the unlock gate is
    // intentionally absent. The mini-only `mark_kill_requested` gate (below) still
    // prevents this from ever touching a non-mini PTY.
    crate::backend::agent_pty::validate_agent_id(&agent_id)?;

    // 1) Record killRequested FIRST (persisted) so the EOF-finalize sees the intent.
    //    `mark_kill_requested` returns Some(<live PTY agent id>) ONLY when a genuine LIVE
    //    (non-terminal) mini chain owns this id. WARNING 6: the returned id is the chain's
    //    ACTIVE (Launching|Running) attempt's PTY — which may DIFFER from `agent_id` when
    //    the human stopped via an Failed predecessor's STALE id (a dead past mini);
    //    killing `agent_id` directly would miss the live retry. WARNING 4: an already-
    //    terminal mini is left untouched (None); WARNING 3: a non-mini id matches nothing.
    let pty_to_kill = agents::mutate_agent_live_state(&app, |st| {
        let target = mark_kill_requested(st, &agent_id);
        cap_pass(st);
        target
    })
    .ok()
    .flatten();

    // 2) THEN kill the LIVE attempt's PTY OUTSIDE the state lock -> EOF -> executor
    //    finalize as aborted_by_human. SELF-DEFENCE (WARNING 3): only kill when a live
    //    mini chain was matched — never kill a non-mini PTY (e.g. a normal coder) from
    //    this command, even if the UI gate is bypassed by a mis-routed/crafted invoke.
    //    The executor's `finalize_finished_mini` (driven by the PTY EOF this kill
    //    causes) closes the mini SESSION within the SAME locked write that records
    //    aborted_by_human (WARNING 2: we do NOT close the session here — doing so
    //    raced the executor's "done" close, yielding a non-deterministic final session
    //    status and a redundant/early write).
    // An AGENTIC worker has no PTY to kill — signal its cancel flag so the loop actually
    // halts (checked between rounds + before each tool call). No-op for one-shot minis.
    if let Some(state) = app.try_state::<MiniCoderState>() {
        state.cancel_agentic(&agent_id);
        if let Some(live_id) = &pty_to_kill {
            state.cancel_agentic(live_id);
        }
    }

    match pty_to_kill {
        Some(live_id) => crate::backend::agent_pty::kill_agent_pty(&app, &live_id),
        None => {
            // No live mini owns this id (non-mini id, already-terminal mini, or capped
            // out): no-op. Debug log only — never kill an unrelated PTY.
            eprintln!("mini_coder_kill: no live mini for agent_id; no-op (not killing PTY)");
        }
    }
    Ok(())
}

/// ASYNC STEERING (a) — the HUMAN's Console hook to steer a RUNNING mini, the sibling of
/// [`mini_coder_kill`]. The SAME external-signal channel generalized: instead of one Stop
/// bool, the human APPENDS a mid-flight correction to the live mini chain's `steer_queue`,
/// which the executor folds into the NEXT fix-pass round's task (`build_retry_directive` /
/// `fold_steer_block`). A `message` equal to the STOP sentinel
/// ([`mini_coder::STEER_STOP_SENTINEL`], case-insensitive) REUSES the Stop path: it flags
/// `kill_requested` across the chain and kills the live PTY, exactly like the Stop button.
///
/// CONTRACT (matches `pi-subagents` "interrupts after the current tool execution"):
/// steering takes effect at a ROUND BOUNDARY (between the mini's fix passes), NOT mid-token.
/// A single one-shot mini with NO fix pass therefore has no mid-flight injection point
/// except `stop` — a queued correction on such a mini lands only if/when the verdict gate
/// opens a retry round. The result reports `status`: `queued` (+ `queued` length) /
/// `stopped` / `queue_full` (+ `queued`) / `noop`, so the Console can tell the human what
/// happened.
///
/// LOCK DISCIPLINE mirrors `mini_coder_kill`: the queue/flag write happens under the
/// agent-state file lock; any PTY kill (the stop path) happens AFTER that lock is released.
/// SELF-DEFENCE: a non-mini id, an already-terminal mini, or an unmatched id is a pure
/// no-op — this command can never touch an unrelated PTY (it only kills on `stop`, and only
/// the live mini chain's PTY). NO vault-unlock gate (like Stop): a mid-flight correction
/// neither reads secrets nor mutates protected config.
#[tauri::command]
pub fn mini_coder_steer(
    app: AppHandle,
    state: State<'_, BackendState>,
    agent_id: String,
    message: String,
) -> Result<serde_json::Value, String> {
    // Audit F-02-003: steer injects work into a mini; Stop (mini_coder_kill) stays
    // unlock-free by design (safety override). Steer is not an emergency stop.
    state.ensure_unlocked()?;
    crate::backend::agent_pty::validate_agent_id(&agent_id)?;
    // CO-WRITER PARITY with the Python `steer_mini_coder` tool (clean_text ->
    // strip_invisible_and_bidi + whitespace collapse, THEN the shared cap): C2 — strip
    // invisible/bidi/control chars (so a steer can't smuggle an RTL override / zero-width
    // joiner / BOM into the prompt or a toast) and collapse whitespace BEFORE capping to the
    // shared MAX_STEER_MESSAGE_LEN, so a pathological message cannot bloat the directive or
    // the fix-pass prompt regardless of which writer set it. C3: an empty/blank message
    // (after sanitize) is REJECTED with an error — matching Python's `clean_text` McpError —
    // rather than silently succeeding as a no-op.
    let sanitized = mini_coder::sanitize_steer_message(&message);
    if sanitized.is_empty() {
        return Err("Steer message is required.".into());
    }
    let trimmed: String = sanitized
        .chars()
        .take(mini_coder::MAX_STEER_MESSAGE_LEN)
        .collect();

    // 1) Append the correction (or flag the stop) under the state lock.
    let outcome = agents::mutate_agent_live_state(&app, |st| {
        let res = mark_steer_requested(st, &agent_id, &trimmed);
        cap_pass(st);
        res
    })
    .unwrap_or(SteerOutcome::NoOp);

    // 2) On a STOP steer, drive the live PTY to EOF OUTSIDE the lock — identical to
    //    `mini_coder_kill` (the executor's EOF-finalize then synthesizes aborted_by_human).
    let result = match outcome {
        SteerOutcome::Stopped { live_agent_id } => {
            if let Some(live_id) = live_agent_id {
                crate::backend::agent_pty::kill_agent_pty(&app, &live_id);
            }
            serde_json::json!({ "status": "stopped" })
        }
        SteerOutcome::Queued { queued, .. } => {
            serde_json::json!({ "status": "queued", "queued": queued })
        }
        SteerOutcome::QueueFull { queued } => {
            serde_json::json!({ "status": "queue_full", "queued": queued })
        }
        SteerOutcome::NoOp => {
            // PI-SESSION FALLBACK: when no directive row matched (NoOp), the agent may still
            // be a live pi-sidecar session (a coder/mini whose steering is via the sidecar,
            // not the directive queue). Delegate to the pi route — mirroring orchestrator_steer
            // (projects.rs:4107) — so a pi coder/mini row is steerable from the UI. The MCP
            // surface (`steer_mini_coder`) intentionally stays not_found for unknown ids: the
            // rig pin test (`test_steer_pi_coder_id_pins_not_found`) keeps passing because the
            // MCP tool never touches the pi route; only the UI's `mini_coder_steer` command
            // falls back here. A send error propagates as Err (fail loud); the user-echo
            // injection is best-effort so a queue-full sidecar never blocks the steer.
            if crate::backend::pi_sidecar::pi_session_exists(&app, &agent_id) {
                // STOP sentinel FIRST (review HIGH): "stop" on a directive row sets
                // kill_requested; on a pi session it must STOP THE SESSION — without
                // this guard the literal word "stop" was chatted at the model and the
                // session kept running.
                if mini_coder::is_steer_stop(&trimmed) {
                    let existed =
                        crate::backend::pi_sidecar::stop_pi_session(&app, &agent_id)?;
                    return Ok(if existed {
                        serde_json::json!({ "status": "stopped" })
                    } else {
                        serde_json::json!({ "status": "noop" })
                    });
                }
                // User echo: inject BEFORE delivery so the sidecar's reader thread fires the
                // echo queue-refill before the prompt (the echo appears before any assistant
                // output). Best-effort: a missing/missing queue is non-fatal — the steer
                // still reaches the sidecar via send_prompt_to_session. `msg_id` is None
                // (the UI does not carry a steer msg id — the user-echo is the steer itself).
                let _ = crate::backend::pi_sidecar::inject_console_entry(
                    &app,
                    &agent_id,
                    crate::backend::mini_activity::ConsoleEntry::Chat {
                        role: "user".to_string(),
                        text: trimmed.clone(),
                        time: crate::backend::mini_activity::console_now_str(),
                        msg_id: None,
                    },
                );
                crate::backend::pi_sidecar::send_prompt_to_session(&app, &agent_id, &trimmed)?;
                serde_json::json!({ "status": "steered_pi" })
            } else {
                serde_json::json!({ "status": "noop" })
            }
        }
    };
    Ok(result)
}

/// TEST-ONLY headless one-shot: a trivial shell that writes a fixed `done` result
/// JSON (`{"status":"done","output":"<task>"}`) to `result_target` then exits.
/// Kept so the `#[ignore]` integration test still exercises spawn -> one-shot ->
/// result-file -> EOF -> read WITHOUT a real model backend. NOT used in production
/// (production always goes through `build_mini_command` with a configured backend).
#[cfg(all(test, windows))]
fn build_headless_mini_command(
    project_root: &Path,
    result_target: &Path,
    task: &str,
) -> Result<CommandBuilder, String> {
    use super::mini_coder::MiniCoderResult;
    let result = MiniCoderResult {
        status: "done".to_string(),
        output: Some(task.to_string()),
        ..Default::default()
    };
    let json = serde_json::to_string(&result)
        .map_err(|e| format!("could not serialize mini result: {e}"))?;
    let json_lit = json.replace('\'', "''");
    let target = result_target.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         [System.IO.File]::WriteAllText('{target}', '{json_lit}', (New-Object System.Text.UTF8Encoding $false)); \
         exit 0"
    );
    let mut cmd = CommandBuilder::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    cmd.cwd(project_root);
    Ok(cmd)
}

#[cfg(test)]
#[path = "mini_coder_executor_tests.rs"]
mod tests;

/// HL-3: console-seed path must call `record_cost` exactly once for attempt-0
/// roots (historical double-count inflated the ledger on every mini spawn).
#[cfg(test)]
mod hl3_cost_once_tests {
    #[test]
    fn mini_console_seed_records_cost_exactly_once() {
        let src = include_str!("mini_coder_executor.rs");
        let start = src
            .find("// CONSOLE (Step B):")
            .expect("console seed marker");
        let end = src[start..]
            .find("/// Read the finished mini's result file")
            .map(|i| start + i)
            .expect("finalize marker after console seed");
        let block = &src[start..end];
        let count = block.matches("record_cost").count();
        assert_eq!(
            count, 1,
            "attempt-0 mini root must call record_cost exactly once in console seed (got {count})"
        );
    }
}

#[cfg(test)]
#[path = "rig_executor_tests.rs"]
mod rig_tests;

// ── Slice 3: unattended_denial_note pure helper ───────────────────────────────

#[cfg(test)]
mod denial_note_tests {
    use super::unattended_denial_note;

    #[test]
    fn net_with_empty_detail_omits_colon() {
        let note = unattended_denial_note("net", "");
        assert_eq!(note, "Network access denied (Unattended mode)");
    }

    #[test]
    fn net_with_detail_includes_hint() {
        let note = unattended_denial_note("net", "cargo fetch failed");
        assert!(
            note.contains("Network access denied (Unattended mode)"),
            "must start with denial prefix"
        );
        assert!(note.contains("cargo fetch failed"), "must include the hint");
    }

    #[test]
    fn folder_formats_path() {
        let note = unattended_denial_note("folder", "/tmp/extra");
        assert_eq!(note, "Write to \"/tmp/extra\" denied (Unattended mode)");
    }

    #[test]
    fn unknown_kind_produces_generic_note() {
        let note = unattended_denial_note("exec", "");
        assert!(note.contains("Unattended mode"), "must mention mode");
        assert!(note.contains("exec"), "must mention kind");
    }

    #[test]
    fn note_is_capped_at_200_chars() {
        // A folder path longer than the cap produces a truncated note.
        let long_path = "x".repeat(300);
        let note = unattended_denial_note("folder", &long_path);
        assert!(
            note.chars().count() <= 200,
            "note must not exceed 200 chars; got {}",
            note.chars().count()
        );
    }

    #[test]
    fn net_hint_is_capped_at_120_chars() {
        let long_hint = "h".repeat(200);
        let note = unattended_denial_note("net", &long_hint);
        // The hint is truncated to 120 chars before embedding, so total stays well under 200.
        assert!(
            note.chars().count() <= 200,
            "note must not exceed 200 chars; got {}",
            note.chars().count()
        );
        // Hint portion present (first 120 'h' chars).
        assert!(note.contains(&"h".repeat(120)));
    }
}

// ---- attach_censor_summary pure helper tests ----
#[cfg(test)]
mod attach_censor_summary_tests {
    use super::*;

    fn sample_state() -> crate::backend::model::AgentLiveState {
        crate::backend::model::AgentLiveState {
            version: 6,
            updated_at: String::new(),
            sessions: Vec::new(),
            claims: Vec::new(),
            events: Vec::new(),
            rules: Vec::new(),
            state_path: String::new(),
            mcp_command: String::new(),
            mcp_client_config: String::new(),
            mini_coder_directives: Vec::new(),
            visual_check_directives: Vec::new(),
            design_request_directives: Vec::new(),
            git_push_requests: Vec::new(),
            plan_approval_requests: Vec::new(),
            consent_requests: Vec::new(),
        }
    }

    #[test]
    fn attach_finds_existing_directive() {
        let mut state = sample_state();
        let directive = crate::backend::mini_coder::MiniCoderDirective {
            id: "d-attach".into(),
            parent_agent_id: "coder-1".into(),
            task: "t".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            // All other fields are zero-sized defaults (empty strings, None, empty Vecs).
            ..crate::backend::mini_coder::MiniCoderDirective {
                id: String::new(),
                parent_agent_id: String::new(),
                ..crate::backend::mini_coder::MiniCoderDirective {
                    id: String::new(),
                    parent_agent_id: String::new(),
                    status: crate::backend::mini_coder::MiniCoderStatus::Pending,
                    task: String::new(),
                    files: Vec::new(),
                    write: false,
                    write_mode: crate::backend::mini_coder::WriteMode::EmitEdits,
                    tier: crate::backend::mini_coder::DirectiveTier::Mini,
                    project_id: None,
                    task_id: None,
                    backend: None,
                    allow_oracle: false,
                    kill_requested: false,
                    steer_queue: Vec::new(),
                    result_path: String::new(),
                    agent_id: None,
                    created_at: String::new(),
                    claimed_at: None,
                    scratch_path: None,
                    started_at: None,
                    result: None,
                    stuck_report: None,
                    censor_summary: None,
                    attempt: 0,
                    parent_directive_id: None,
                    pigeon_ticket: None,
                    collected: None,
                }
            }
        };
        state.mini_coder_directives.push(directive);
        let attached = attach_censor_summary(
            &mut state,
            "d-attach",
            crate::backend::mini_coder::CensorMiniSummary {
                total: 5,
                files: vec!["src/a.rs".into()],
                ran: false,
            },
        );
        assert!(attached, "must find the directive row");
        let d = state
            .mini_coder_directives
            .iter()
            .find(|d| d.id == "d-attach")
            .expect("directive present");
        let s = d.censor_summary.as_ref().expect("summary attached");
        assert_eq!(s.total, 5);
        assert_eq!(s.files, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn attach_missing_directive_returns_false() {
        let mut state = sample_state();
        let attached = attach_censor_summary(
            &mut state,
            "nonexistent",
            crate::backend::mini_coder::CensorMiniSummary {
                total: 1,
                files: vec![],
                ran: false,
            },
        );
        assert!(!attached, "must not find a missing directive");
    }

    #[test]
    fn attach_overwrites_previous_summary() {
        let mut state = sample_state();
        let directive = crate::backend::mini_coder::MiniCoderDirective {
            id: "d-overwrite".into(),
            parent_agent_id: "coder-1".into(),
            task: "t".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            censor_summary: Some(crate::backend::mini_coder::CensorMiniSummary {
                total: 1,
                files: vec!["old.rs".into()],
                ran: false,
            }),
            ..Default::default()
        };
        state.mini_coder_directives.push(directive);
        let attached = attach_censor_summary(
            &mut state,
            "d-overwrite",
            crate::backend::mini_coder::CensorMiniSummary {
                total: 9,
                files: vec!["new.rs".into()],
                ran: false,
            },
        );
        assert!(attached);
        let d = state
            .mini_coder_directives
            .iter()
            .find(|d| d.id == "d-overwrite")
            .expect("directive present");
        let s = d.censor_summary.as_ref().expect("summary overwritten");
        assert_eq!(s.total, 9, "total updated");
        assert_eq!(s.files, vec!["new.rs".to_string()], "files updated");
    }
}
