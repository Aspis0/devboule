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
use std::time::Duration;

use chrono::Utc;
use portable_pty::CommandBuilder;
use tauri::{AppHandle, Manager};

use super::agents;
use super::mini_coder::{
    self, MiniCoderBackend, MiniCoderBackendKind, MiniCoderDirective, MiniCoderOutcome,
    MiniCoderStatus, DEFAULT_LAUNCH_CAP_SECS, DEFAULT_WALL_CLOCK_CAP_SECS, MAX_DIRECTIVES,
};
#[cfg(windows)]
use super::projects::ps_single_quote;

/// How often the executor wakes to scan the directive queue. A coder's
/// `spawn_mini_coder` blocks on a ~0.75s MCP poll, so a 1.5s executor cadence keeps
/// the end-to-end claim→spawn latency comfortably inside the coder's poll while
/// keeping the idle cost (one locked read of a usually-empty queue) negligible.
const SCAN_INTERVAL: Duration = Duration::from_millis(1500);

/// Scratch dir name (under the project root) where minis write their result files.
/// A sibling of `.aspis-censor`; `read_result_file` confines reads to it.
const MINI_SCRATCH_DIR: &str = ".aspis-mini";
const VISUAL_CHECK_TIMEOUT_SECS: i64 = 120;

/// Env var carrying the PATH to the OPTIONAL oMLX bearer-token file (oMLX-P2). The
/// token itself NEVER touches argv/PTY/logs: it lives in a 0600 restricted file whose
/// path rides in this env var; the launch script reads the file and sends
/// `Authorization: Bearer <token>`. Unset ⇒ no key configured ⇒ no header. Used by
/// both the Windows (`$env:OMLX_KEY_FILE`) and macOS (`$OMLX_KEY_FILE`) arms; kept
/// uncfg'd so the platform-agnostic macOS-script test can reference it on Windows.
const OMLX_KEY_FILE_ENV: &str = "OMLX_KEY_FILE";

/// Env var carrying the oMLX HTTP request timeout (seconds) to the launch script
/// (macOS python `urlopen`). Non-secret. Derived from `DEFAULT_WALL_CLOCK_CAP_SECS`
/// (the executor's PTY wall-clock kill) MINUS `OMLX_HTTP_TIMEOUT_MARGIN_SECS`, so a
/// stalled oMLX request fails fast on the HTTP layer JUST BEFORE the PTY is killed
/// (a clean `failed` fallback instead of waiting the full cap). Kept uncfg'd so the
/// platform-agnostic macOS-script test can reference it on the Windows dev host.
const OMLX_TIMEOUT_ENV: &str = "OMLX_TIMEOUT";

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
fn omlx_http_timeout_secs() -> i64 {
    (DEFAULT_WALL_CLOCK_CAP_SECS - OMLX_HTTP_TIMEOUT_MARGIN_SECS).max(1)
}

/// Managed singleton state for the mini-coder executor. Holds the shared stop flag
/// and the loop's join handle so app-exit can signal + reap it. `None` thread means
/// not yet installed (or already reaped).
pub struct MiniCoderState {
    running: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// BLOCKER 2 (EXECUTOR-LOOP STALL): process-wide set of directive ids whose
    /// deferred Censor-VERDICT thread is currently running. The FINE linters (5–30s) ran
    /// SYNCHRONOUSLY inside `finalize_finished_mini` on the single executor thread,
    /// blocking ALL scheduling (timeouts, new claims, sibling finalizes) for their whole
    /// duration. Now a clean `done` on a TRUSTED project defers the verdict to a
    /// dedicated thread; the executor loop continues immediately. This set is the
    /// IN-FLIGHT GUARD so `run_pass` does NOT re-detect + re-spawn the same finished mini
    /// (its PTY is already gone), AND the TIMEOUT EXCLUSION so `plan_tick` does not
    /// wall-cap-timeout a directive that is Running-but-awaiting-its-verdict-thread. The
    /// thread always clears its id (success, error, or panic — fail-closed).
    verdict_inflight: Arc<Mutex<std::collections::HashSet<String>>>,
}

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

    // P6 (crash recovery): an `AwaitingRetry` directive whose forward-linked retry
    // directive is ABSENT from the queue (evicted, or never appended after a crash
    // between the awaiting-retry stamp and the retry append) is stuck in limbo — no
    // retry will ever propagate a verdict back to it. Fail it (`failed("retry lost")`)
    // and propagate to its lineage so the ROOT id's poll unblocks. Best-effort.
    if let Err(e) = sweep_orphaned_awaiting_retry(&app) {
        eprintln!("mini-coder executor: startup awaiting-retry-sweep error: {e}");
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

/// P6 (crash recovery) + BLOCKER 1: stamp every `AwaitingRetry` directive that its retry
/// chain can no longer reach via normal finalize propagation, then propagate that terminal
/// up the chain so the Python poll on the ROOT id unblocks. Two cases (pure
/// `awaiting_retry_needing_terminal`):
///   * ABSENT retry child (evicted / never appended after a crash) -> `failed("retry
///     lost")`.
///   * PRESENT + TERMINAL retry child while the predecessor is still AwaitingRetry (a
///     MISSED propagation — e.g. a retry that failed at LAUNCH before the BLOCKER-1 fix
///     routed `fail_launching` through propagation, or a crash mid-propagation) ->
///     re-propagate the CHILD's own terminal outcome.
///
/// AwaitingRetry is neither active nor terminal, so the `apply_*` active-only guard can't
/// stamp it — we write `status`/`result` DIRECTLY, then propagate to the chain's other
/// AwaitingRetry ancestors. Returns Err only on a hard state-access failure.
fn sweep_orphaned_awaiting_retry(app: &AppHandle) -> Result<(), String> {
    let snapshot = agents::read_agent_live_state_snapshot(app)?;
    reconcile_awaiting_retry_orphans(app, &snapshot.mini_coder_directives)
}

/// WARNING 3 (self-healing): the actual AwaitingRetry-orphan reconcile, taking the
/// directives slice the CALLER already holds (the startup sweep reads its own snapshot;
/// `run_pass` reuses its pass snapshot — no extra locked read). Idempotent by the
/// snapshot-then-recheck-under-lock pattern: an action is only stamped if the directive is
/// STILL AwaitingRetry under the lock, so a concurrent finalize can't double-stamp.
///
/// Folding this into the steady-state pass (in addition to the once-at-startup call) makes
/// a TRANSIENT startup lock-contention failure self-heal on a later tick, instead of
/// permanently stranding the orphaned AwaitingRetry directive.
fn reconcile_awaiting_retry_orphans(
    app: &AppHandle,
    directives: &[MiniCoderDirective],
) -> Result<(), String> {
    let actions = mini_coder::awaiting_retry_needing_terminal(directives);
    if actions.is_empty() {
        return Ok(());
    }
    agents::mutate_agent_live_state(app, |state| {
        let mut protect: Vec<String> = Vec::new();
        for (id, action) in &actions {
            // Re-check it is STILL AwaitingRetry under the lock (a racing finalize could
            // have moved it), then direct-stamp the outcome + propagate.
            let still_awaiting = state
                .mini_coder_directives
                .iter()
                .any(|d| d.id == *id && d.status == MiniCoderStatus::AwaitingRetry);
            if !still_awaiting {
                continue;
            }
            // Resolve the terminal outcome to stamp: a synthesized `failed` for a lost
            // child, or the child's OWN terminal result for the missed-propagation case
            // (fall back to a synthesized failed if the child's result is somehow unset).
            let outcome = match action {
                mini_coder::RetrySweepAction::FailLost => {
                    MiniCoderOutcome::failed("retry lost (retry directive absent after restart)")
                }
                mini_coder::RetrySweepAction::PropagateChildTerminal { child_id } => state
                    .mini_coder_directives
                    .iter()
                    .find(|d| d.id == *child_id)
                    .and_then(|c| c.result.clone())
                    .unwrap_or_else(|| {
                        MiniCoderOutcome::failed("retry chain terminated (outcome unrecorded)")
                    }),
            };
            if let Some(d) = state.mini_coder_directives.iter_mut().find(|d| d.id == *id) {
                d.status = outcome.status;
                d.result = Some(outcome.clone());
            }
            propagate_terminal_to_ancestors(state, id, &outcome);
            // WARNING 5: protect the chain just stamped terminal this pass from eviction.
            protect.extend(just_finalized_chain_ids(state, id));
        }
        cap_pass_protecting(state, &protect);
    })
}

/// WARNING 5 (result-file leak recovery): delete orphaned `*.json` files in the known
/// `.aspis-mini/` scratch dir(s) that no LIVE (non-terminal) directive still needs.
///
/// A mini whose terminal state-write permanently fails leaves its result file behind
/// (`finalize_finished_mini` only removes it on a successful write). Over many such
/// failures the scratch dir grows unbounded. On startup we reclaim them: for each
/// distinct scratch dir recorded on a current directive, delete every `*.json` whose
/// name is NOT the `result_path` of a still-live directive in that same dir.
///
/// Bounded + best-effort by construction: we only touch the precise scratch dirs the
/// executor itself recorded (never the whole project tree), we only delete `*.json`,
/// and every fs error is swallowed (a failure never aborts startup). Returns Err only
/// on a hard state-READ failure.
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

/// One executor pass (extracted so it is callable from a test with a real PTY).
/// Returns Err only on a hard state-access failure; per-directive problems degrade
/// to a synthesized `failed`/`timeout` outcome rather than aborting the pass.
fn run_pass(app: &AppHandle) -> Result<(), String> {
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

    // WARNING 3 (self-healing): reconcile AwaitingRetry directives whose retry child is
    // lost/terminal, every pass — not only the once-at-startup sweep. A transient startup
    // lock-contention failure (or an AwaitingRetry orphan that arose post-startup) thus
    // self-heals on a later tick instead of stranding forever. Reuses THIS pass snapshot
    // (no extra read); cheap when nothing is orphaned (`awaiting_retry_needing_terminal`
    // returns empty -> no mutate). A failure here is non-fatal: log and continue the pass.
    if let Err(e) = reconcile_awaiting_retry_orphans(app, &directives) {
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

    let now = Utc::now().to_rfc3339();
    let plan = mini_coder::plan_tick_excluding(
        &directives,
        &now,
        DEFAULT_WALL_CLOCK_CAP_SECS,
        mini_coder::DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
        DEFAULT_LAUNCH_CAP_SECS,
        max_concurrent,
        &verdict_inflight,
    );

    // 2) Timeouts FIRST (reap blown-cap minis regardless of concurrency): kill the
    //    PTY OUTSIDE any lock, then transition the directive under the lock.
    for timed_out_id in &plan.timeouts {
        if let Some(directive) = directives.iter().find(|d| &d.id == timed_out_id) {
            kill_mini_pty(app, directive);
            let agent_id = directive.agent_id.clone();
            let _ = agents::mutate_agent_live_state(app, |state| {
                // P5: killRequested WINS. If the human hit Stop, the human's intent
                // overrides a same-pass timeout — consult the LIVE `d.kill_requested`
                // (set under the lock by `mini_coder_kill`, possibly after the stale
                // pass snapshot was taken) and synthesize aborted_by_human instead.
                transition_directive(state, timed_out_id, |d| {
                    if d.kill_requested {
                        mini_coder::apply_aborted(d, "stopped by human (Stop button)")
                    } else {
                        mini_coder::apply_timeout(d, "wall-clock cap exceeded")
                    }
                });
                // WARNING 3: close the lingering mini session row too.
                if let Some(aid) = agent_id.as_deref() {
                    close_mini_session(state, aid);
                }
                cap_pass(state);
            });
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
                cap_pass(state);
            });
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
        let parent_gone = parent_is_gone(&snapshot, &directive.parent_agent_id) && {
            // Lazily take the single fresh snapshot on the first re-check this pass.
            let fresh = fresh_recheck
                .get_or_insert_with(|| agents::read_agent_live_state_snapshot(app).ok());
            match fresh {
                Some(fresh) => parent_is_gone(fresh, &directive.parent_agent_id),
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
                cap_pass(state);
            });
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
        let still_live = match &pty_sessions {
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
            let parent_project = snapshot_parent_project(&snapshot, &directive.parent_agent_id);
            claim_and_launch(app, directive, parent_project);
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
    let project_root = match crate::backend::projects::resolve_project_root_by_id(app, &project_id) {
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
            let outcome =
                crate::backend::visual_check::execute_visual_check(app_clone.clone(), &project_root, &directive);
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
            // (`Launching|Running`, AwaitingRetry excluded) BEFORE claiming, so a stale
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
    let backend = match super::projects::read_mini_coder_backend(app) {
        Some(b) => b,
        None => {
            fail_launching(app, &directive_id, "no mini-coder backend configured");
            return;
        }
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

    if let Err(e) = spawn_one_shot_mini(
        app,
        &agent_id,
        &project_root,
        &scratch_root,
        &result_rel,
        &backend,
        directive,
        mcp_roots.as_ref(),
    ) {
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
    let _ = agents::mutate_agent_live_state(app, |state| {
        transition_directive(state, &directive_id, |d| {
            mini_coder::apply_launched(d, nest_id.clone(), started_at.clone()).map(|mut next| {
                next.scratch_path = Some(scratch_path_str.clone());
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
        );
        cap_pass(state);
    });
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

    // The gate (linters) is needed ONLY for a clean, un-killed `done` on a TRUSTED tree.
    let needs_gate =
        outcome.status == MiniCoderStatus::Done && !directive.kill_requested && trusted;

    if needs_gate {
        // BLOCKER 2: DEFER. Claim the in-flight guard so `run_pass` neither re-finalizes
        // nor wall-cap-times-out this directive while the verdict thread runs. If the
        // claim fails (a thread is already running for this id — possible after a poisoned
        // lock or a racing pass) do NOTHING: the existing thread will finalize it.
        if let Some(state) = app.try_state::<MiniCoderState>() {
            if state.claim_verdict(&directive.id) {
                spawn_verdict_thread(app.clone(), directive.clone(), project_id, outcome);
                return;
            }
            // Could not claim (already in flight) — leave it to the running thread.
            return;
        }
        // No managed state (tests): fall through to inline finalize with the gate, using
        // the real collector — preserves the old single-thread test behavior. WARNING 6:
        // no managed stop flag here -> a fresh always-run flag (this is a one-shot inline
        // collect with no shutdown to honor).
        let pid = project_id.clone();
        let stop = AtomicBool::new(true);
        finalize_finished_mini_with(app, directive, outcome, trusted, |root, files| {
            real_censor_verdict(app, pid.as_deref(), root, files, &stop)
        });
        return;
    }

    // INLINE (no linters): untrusted / non-done / killed / no-project. The gate is a
    // no-op here (high_findings stays empty), so the verdict closure is never called.
    finalize_finished_mini_with(app, directive, outcome, trusted, |_root, _files| Vec::new());
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
fn spawn_verdict_thread(
    app: AppHandle,
    directive: MiniCoderDirective,
    project_id: Option<String>,
    outcome: MiniCoderOutcome,
) {
    // Clones retained for the spawn-failure fallback path (the closure MOVES the originals).
    let fb_app = app.clone();
    let fb_directive = directive.clone();
    let fb_project_id = project_id.clone();
    let fb_outcome = outcome.clone();

    // BLOCKER 2 (RAII) + WARNING 6: resolve the in-flight-set handle AND the executor's
    // real stop flag from the managed state ONCE, before the spawn. The guard releases the
    // claimed id from inside the thread on every exit path; the stop flag is threaded into
    // the linter run so a shutdown bails it. The id was already CLAIMED by the caller
    // (`finalize_finished_mini`), so the state is guaranteed present here.
    let inflight = app.try_state::<MiniCoderState>().map(|s| s.verdict_inflight_handle());
    let stop = app
        .try_state::<MiniCoderState>()
        .map(|s| s.running_flag())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));

    let stop_for_thread = Arc::clone(&stop);
    let spawned = std::thread::Builder::new()
        .name("mini-coder-verdict".into())
        .spawn(move || {
            let id = directive.id.clone();
            let pid = project_id.clone();
            let stop = stop_for_thread;
            run_verdict_thread_body(
                inflight,
                id,
                // (a)+(b) compute the verdict (slow) + apply the decision. TRUSTED (the
                // caller only defers a trusted done). WARNING 6: the linter honors `stop`.
                || {
                    finalize_finished_mini_with(&app, &directive, outcome.clone(), true, |root, files| {
                        real_censor_verdict(&app, pid.as_deref(), root, files, &stop)
                    });
                },
                // FAIL-CLOSED: a panic in the verdict/apply must not block the mini's
                // success. Re-finalize as the clean `done` with NO findings (trusted=true
                // + empty findings -> StampTerminal(done)) so the chain unblocks. The
                // verdict closure is never called here.
                || {
                    finalize_finished_mini_with(&app, &directive, outcome.clone(), true, |_r, _f| {
                        Vec::new()
                    });
                },
            );
        });
    if let Err(e) = spawned {
        // Could not spawn the thread: clear the guard we claimed and finalize INLINE so
        // the directive never strands (degraded: one inline stall, never a stuck mini).
        eprintln!("mini-coder: verdict thread spawn failed ({e}); finalizing inline");
        if let Some(state) = fb_app.try_state::<MiniCoderState>() {
            state.release_verdict(&fb_directive.id);
        }
        finalize_finished_mini_with(&fb_app, &fb_directive, fb_outcome, true, |root, files| {
            real_censor_verdict(&fb_app, fb_project_id.as_deref(), root, files, &stop)
        });
    }
}

/// BLOCKER 2: the verdict thread body, extracted so the RAII release + the
/// double-catch_unwind fail-closed path are unit-testable WITHOUT an `AppHandle` (inject
/// fakes for `work` / `fail_closed`). Invariants:
///  * the [`VerdictInflightGuard`] is created FIRST and dropped LAST — so `id` is released
///    on EVERY exit path: normal return, a `work` panic, AND a `fail_closed` panic (Drop
///    runs during unwind).
///  * `work` runs under `catch_unwind`; on a caught panic `fail_closed` runs under its OWN
///    `catch_unwind`, so a DOUBLE panic (work AND fail-closed) cannot escape the thread.
fn run_verdict_thread_body(
    inflight: Option<Arc<Mutex<std::collections::HashSet<String>>>>,
    id: String,
    work: impl FnOnce(),
    fail_closed: impl FnOnce(),
) {
    // RAII guard FIRST: its Drop releases the id no matter how we leave this function. When
    // there is no managed state (tests that don't install it) the guard is absent and the
    // caller is responsible — but every production path has it (the id was just claimed).
    let _guard = inflight.map(|set| VerdictInflightGuard { set, id });

    let work_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
    if work_res.is_err() {
        eprintln!("mini-coder verdict thread: panicked; fail-closing to done");
        // BLOCKER 2: wrap the fail-closed finalize in its OWN catch_unwind so a panic
        // here (a double panic) cannot unwind past the guard's Drop or abort the process.
        let fc_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(fail_closed));
        if fc_res.is_err() {
            eprintln!("mini-coder verdict thread: fail-closed finalize ALSO panicked; id still released by guard");
        }
    }
    // `_guard` drops here (or during unwind above) -> the in-flight id is released.
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
///  2. `verdict_gate_decision` turns (directive, outcome, trusted, high_findings) into a
///     pure `GateDecision`.
///  3. Apply the decision under ONE `mutate_agent_live_state`:
///       - StampTerminal: stamp the outcome (with the P5 live-kill re-check) + propagate.
///       - AwaitingRetryWith: `apply_awaiting_retry` + append the Pending retry (atomic).
///       - Escalate: stamp Escalated + propagate to the chain's AwaitingRetry ancestors.
///  4. Delete the result file + record the training rail AFTER the write succeeds.
fn finalize_finished_mini_with(
    app: &AppHandle,
    directive: &MiniCoderDirective,
    outcome: MiniCoderOutcome,
    trusted: bool,
    verdict_fn: impl Fn(&Path, &[String]) -> Vec<mini_coder::EscalationFinding>,
) {
    // 1) VERDICT GATE — only on a clean self-reported `done` that the human did NOT kill,
    //    on a TRUSTED tree (resolved by the caller). For everything else we skip straight
    //    to stamping the outcome (gate is a no-op there — verdict_fn is never called).
    let project_root: Option<PathBuf> = directive
        .scratch_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .and_then(|p| Path::new(p).parent().map(|r| r.to_path_buf()));
    let mut high_findings: Vec<mini_coder::EscalationFinding> = Vec::new();
    if trusted && outcome.status == MiniCoderStatus::Done && !directive.kill_requested {
        if let Some(root) = project_root.as_deref() {
            // Deterministic-only Censor pass (Gemma disabled inside `verdict_fn`).
            high_findings = verdict_fn(root, &outcome.files_touched);
        }
    }

    // 3) Pure decision.
    let now = Utc::now().to_rfc3339();
    let retry_id = format!("{}-r{}", mini_coder::chain_root_id(directive), directive.attempt + 1);
    let retry_result_path = format!("{retry_id}.json");
    let decision = mini_coder::verdict_gate_decision(
        directive,
        &outcome,
        trusted,
        high_findings,
        &retry_id,
        &retry_result_path,
        &now,
    );

    // The result file (if any) to clean up after the outcome is durably recorded.
    let target = directive
        .scratch_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(|p| Path::new(p).join(directive.result_path.replace('\\', "/")));
    let id = directive.id.clone();
    let agent_id = directive.agent_id.clone();

    // 4) Apply the decision atomically. `applied_outcome` is what we record on the
    //    training rail (the terminal outcome actually stamped; None for AwaitingRetry,
    //    which is NOT terminal and produces no training record this finalize).
    let mut applied_outcome: Option<MiniCoderOutcome> = None;
    let applied = agents::mutate_agent_live_state(app, |state| {
        match &decision {
            mini_coder::GateDecision::AwaitingRetryWith { retry } => {
                // ONE atomic mutate: move the predecessor to AwaitingRetry (stamping the
                // forward link) AND append the Pending retry. Never half-applied.
                let stamped = transition_directive_ok(state, &id, |d| {
                    mini_coder::apply_awaiting_retry(d, retry.id.clone())
                });
                if stamped {
                    // Only append the retry if the predecessor actually transitioned (a
                    // racing kill could have made it terminal first — then no retry).
                    state.mini_coder_directives.push((**retry).clone());
                }
                // The predecessor's PTY is gone — close its session row.
                if let Some(aid) = agent_id.as_deref() {
                    close_mini_session(state, aid);
                }
                cap_pass(state);
            }
            mini_coder::GateDecision::Escalate(escalated) => {
                // P5 live-kill re-check still wins (a human Stop during the gate).
                let final_outcome = live_kill_override(state, &id, escalated.clone());
                // BLOCKER 1: stamp + propagate + protect-cap via the SHARED helper (same
                // path as StampTerminal / fail_launching) so the chain root unblocks.
                let fo = final_outcome.clone();
                stamp_terminal_and_propagate(state, &id, &final_outcome, agent_id.as_deref(), |d| {
                    if fo.status == MiniCoderStatus::Escalated {
                        // The intended Running -> Escalated transition (the kill did NOT
                        // win): use the dedicated pure helper, reconstructing its args
                        // from the decision's outcome payload.
                        let escalation = fo.escalation.clone().unwrap_or_default();
                        mini_coder::apply_escalated(d, fo.files_touched.clone(), escalation)
                    } else {
                        // The kill won (aborted_by_human) — stamp the abort instead.
                        mini_coder::apply_result(d, fo.clone())
                    }
                });
                applied_outcome = Some(final_outcome);
            }
            mini_coder::GateDecision::StampTerminal(o) => {
                // P5 RACE BACKSTOP — killRequested WINS even when the flag landed AFTER
                // this pass's snapshot was read (re-check the LIVE d.kill_requested).
                let final_outcome = live_kill_override(state, &id, o.clone());
                // BLOCKER 1: stamp + propagate (unblocks the chain's AwaitingRetry
                // predecessors — the poll watches the ROOT id) + protect-cap + close the
                // session, all via the SHARED helper. A standalone directive has no
                // ancestors (propagation is a no-op there).
                let fo = final_outcome.clone();
                stamp_terminal_and_propagate(state, &id, &final_outcome, agent_id.as_deref(), |d| {
                    mini_coder::apply_result(d, fo.clone())
                });
                applied_outcome = Some(final_outcome);
            }
        }
    });
    // Delete the result file only after the WRITE succeeded, so a crash between read
    // and persist re-reads the same file next pass (idempotent). `applied.is_ok()`
    // means the locked state WRITE succeeded — NOT that the directive transition was
    // applied: a refused transition (already terminal — e.g. a kill won) is a silent
    // no-op inside `transition_directive`, yet the surrounding write still succeeds, so
    // the file is deleted after any successful write (the now-terminal directive will
    // never read it again).
    if applied.is_ok() {
        // The predecessor's result file (this attempt's) is consumed regardless of the
        // decision — a retry writes its OWN result_path, so deleting the predecessor's is
        // always correct. Best-effort.
        if let Some(target) = &target {
            let _ = std::fs::remove_file(target);
        }
        // TRAINING RAIL: record the TERMINAL directive result AFTER the state write
        // succeeded and the agent-state lock is fully released. An AwaitingRetry decision
        // produced NO terminal outcome (`applied_outcome == None`) — nothing to record
        // until the chain's leaf actually terminates. Derive the project root from the
        // persisted scratch path (`<project_root>/.aspis-mini`).
        // LOCK-ORDERING CONTRACT: the agent-state lock is NOT held here — this call
        // comes after `mutate_agent_live_state` returned. training_export's JSONL
        // per-path mutex is therefore the only lock acquired in this section.
        if let Some(terminal) = applied_outcome.as_ref() {
            if let Some(scratch) = directive
                .scratch_path
                .as_deref()
                .filter(|p| !p.trim().is_empty())
            {
                if let Some(project_root) = Path::new(scratch).parent() {
                    super::training_export::record_directive_result(
                        project_root,
                        directive,
                        terminal,
                    );
                }
            }
        }
    }
}

/// Like [`transition_directive`] but reports whether the transition was APPLIED (the
/// pure `apply` returned Ok and the directive existed). Used by the AwaitingRetry path:
/// the Pending retry is appended ONLY when the predecessor actually moved to
/// AwaitingRetry (a racing kill could have made it terminal first — then no retry).
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
fn live_kill_override(
    state: &crate::backend::model::AgentLiveState,
    id: &str,
    outcome: MiniCoderOutcome,
) -> MiniCoderOutcome {
    let killed = state
        .mini_coder_directives
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.kill_requested)
        .unwrap_or(false);
    if killed && outcome.status != MiniCoderStatus::AbortedByHuman {
        MiniCoderOutcome::aborted("stopped by human (Stop button)")
    } else {
        outcome
    }
}

/// P6 PROPAGATION: stamp the SAME terminal `outcome` onto every `AwaitingRetry`
/// predecessor in the leaf `leaf_id`'s retry lineage (so the Python poll watching the
/// chain ROOT id unblocks). Computed on the LIVE state inside the locked closure. An
/// AwaitingRetry directive is neither active nor terminal, so the `apply_*` active-only
/// guard can't stamp it — we write `status`/`result` directly here (the executor owns
/// this impure stamp; the pure transitions stay clean). A standalone directive (not in a
/// chain) has no AwaitingRetry ancestors → no-op.
fn propagate_terminal_to_ancestors(
    state: &mut crate::backend::model::AgentLiveState,
    leaf_id: &str,
    outcome: &MiniCoderOutcome,
) {
    // Find the leaf to compute its lineage (clone the small directive to drop the borrow).
    let Some(leaf) = state
        .mini_coder_directives
        .iter()
        .find(|d| d.id == leaf_id)
        .cloned()
    else {
        return;
    };
    let ancestor_ids = mini_coder::awaiting_retry_ancestors(&state.mini_coder_directives, &leaf);
    for aid in ancestor_ids {
        if let Some(d) = state.mini_coder_directives.iter_mut().find(|d| d.id == aid) {
            // Direct stamp: AwaitingRetry -> the leaf's terminal status + outcome.
            d.status = outcome.status;
            d.result = Some(outcome.clone());
        }
    }
}

/// BLOCKER 1: the SHARED "stamp a terminal outcome on `id` + propagate it to the chain's
/// AwaitingRetry ancestors + grace the just-finalized chain against the cap" sequence,
/// used by EVERY terminal path that must unblock a retry chain's root poll:
/// `finalize_finished_mini_with`'s StampTerminal/Escalate arms AND `fail_launching`.
///
/// The old `fail_launching` only stamped the directive and did NOT propagate — so when a
/// RETRY failed at launch (a project/scratch/spawn error on `attempt >= 1`), its
/// AwaitingRetry predecessor(s), including the ROOT the Python poll watches, were never
/// stamped: the root sat AwaitingRetry forever (the poll eventually timed out with a
/// misleading `timeout`, and the root permanently held a non-evictable directive slot).
///
/// Must be called INSIDE a `mutate_agent_live_state` closure (it mutates `state`). The
/// `apply` closure is the pure terminal transition (e.g. `apply_failed`/`apply_result`);
/// `outcome` is the SAME terminal outcome that closure stamps — used for propagation.
/// `agent_id` (if any) closes the mini's session row. WARNING 5: the cap is run with the
/// just-finalized chain protected so a full queue can't evict the freshly-stamped root
/// before the poll reads it.
fn stamp_terminal_and_propagate(
    state: &mut crate::backend::model::AgentLiveState,
    id: &str,
    outcome: &MiniCoderOutcome,
    agent_id: Option<&str>,
    apply: impl FnOnce(&MiniCoderDirective) -> Result<MiniCoderDirective, String>,
) {
    transition_directive(state, id, apply);
    propagate_terminal_to_ancestors(state, id, outcome);
    if let Some(aid) = agent_id {
        close_mini_session(state, aid);
    }
    let protect = just_finalized_chain_ids(state, id);
    cap_pass_protecting(state, &protect);
}

/// P6: the REAL deterministic-Censor verdict collector injected into
/// [`finalize_finished_mini_with`] in production. Runs the deterministic FINE runners on
/// the touched files with Gemma DISABLED (the deliberate trade-off: a CPU Gemma pass
/// could take 60s+ and stall the single executor loop; Gemma findings still reach the
/// ledger via the live watcher), then reads back OPEN + High-severity findings from the
/// freshly-written shards and projects them to `EscalationFinding`.
///
/// ENTRY POINT NOTE: the pure deterministic collector `orchestrator::fine_batch_collect`
/// is PRIVATE and returns only changed-file names, so we drive the PUBLIC
/// `orchestrator::run_fine_batch(app, project_id, root, files, None /*gemma off*/, &running)`
/// — passing `gemma = None` gives the deterministic-only pass we need. `run_fine_batch`
/// ALSO emits a `findings-updated` Tauri event and records the training rail; both are
/// benign side effects here (no model call, the watcher would emit the same shards). We
/// then read back High findings from the shards via `ledger::read_shard`.
///
/// `root` is the PROJECT root (the parent of the `.aspis-mini` scratch dir). Best-effort:
/// any failure (missing project id, shard read error) yields an empty High set → the mini
/// is treated as CLEAN (we never block a mini on our own collector failure).
fn real_censor_verdict(
    app: &AppHandle,
    project_id: Option<&str>,
    root: &Path,
    files: &[String],
    // WARNING 6: the executor's REAL running/stop flag (threaded from `MiniCoderState`).
    // The fine linters honor it, so a shutdown signal bails the in-flight linter run
    // instead of leaving zombie linter subprocesses. A pre-cleared flag returns fast.
    stop: &AtomicBool,
) -> Vec<mini_coder::EscalationFinding> {
    use crate::backend::censor::schema::{Disposition, Severity};
    let Some(project_id) = project_id else {
        return Vec::new();
    };
    if files.is_empty() {
        return Vec::new();
    }
    // Normalize the touched paths to the shard rel-path form (forward slashes), drop any
    // that fail the ledger path guard (never feed a `..`/absolute into a shard read).
    let rel_files: Vec<String> = files
        .iter()
        .map(|f| f.replace('\\', "/"))
        .filter(|f| crate::backend::censor::ledger::validate_rel_path(f).is_ok())
        .collect();
    if rel_files.is_empty() {
        return Vec::new();
    }
    // WARNING 6: short-circuit before launching ANY linter if shutdown was already
    // signalled (the flag is the executor's real stop flag) — never start work we'd
    // immediately abandon.
    if !stop.load(Ordering::SeqCst) {
        return Vec::new();
    }
    // WARNING 4: use the no-rail variant so the gate does NOT record a training pair —
    // the live Censor watcher records the SAME file change, and recording on both would
    // emit duplicate `censor_verdict` lines into pairs.jsonl. The gate needs only the
    // shards (read back below) for its escalation decision. WARNING 6: pass the REAL
    // stop flag so an app exit bails the in-flight linter run.
    crate::backend::censor::orchestrator::run_fine_batch_no_rail(
        app,
        project_id,
        root,
        &rel_files,
        None,
        stop,
    );

    // Read back OPEN + High findings for exactly the touched files.
    let mut out: Vec<mini_coder::EscalationFinding> = Vec::new();
    for rel in &rel_files {
        let Ok(Some(shard)) = crate::backend::censor::ledger::read_shard(root, rel) else {
            continue;
        };
        for f in shard.findings {
            if f.disposition == Disposition::Open && f.severity == Severity::High {
                out.push(mini_coder::EscalationFinding {
                    file: f.file,
                    severity: "high".into(),
                    source: f.source,
                    title: f.title,
                    line: f.line,
                });
            }
        }
    }
    // MAJOR 3: the synchronous visual advisory was removed from the verdict path. It blocked
    // the VerdictInflightGuard 0.5–30s per HTML-touching task and hijacked the user's design-
    // preview window. The sanctioned path is the `visual_check` MCP tool the mini-coder agent
    // is instructed to call from its own loop (async, not on the blocking verdict thread).
    out
}

/// PURE decision: the terminal `MiniCoderOutcome` a finished mini's directive should
/// receive. Split out of `finalize_finished_mini` so the killRequested-WINS race is
/// directly unit-testable (no AppHandle / lock needed).
///
/// P5 — killRequested WINS: if the human hit Stop (`mini_coder_kill` set
/// `kill_requested` the instant BEFORE the PTY kill), the outcome is
/// `aborted_by_human` REGARDLESS of whether the mini dropped a `done`/
/// `needs_clarification` result file in the same instant. We do NOT read the result
/// file at all in that case — a racing `done` must never overwrite the human's abort.
///
/// Otherwise (the normal EOF path): read the result from the PERSISTED scratch root
/// (BLOCKER 3) and resolve to done/needs_clarification, or `failed` when the file is
/// missing/invalid. A missing/empty persisted scratch path is itself a `failed`.
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
/// set) MUST propagate the `failed` outcome to its AwaitingRetry predecessor(s) —
/// including the chain ROOT the Python poll watches — or the root sits AwaitingRetry
/// forever (poll times out misleadingly, root holds a non-evictable slot). We route
/// through the shared [`stamp_terminal_and_propagate`] so EVERY launch failure
/// propagates exactly like the EOF/timeout terminal paths do.
fn fail_launching(app: &AppHandle, directive_id: &str, reason: &str) {
    let id = directive_id.to_string();
    let reason = reason.to_string();
    let _ = agents::mutate_agent_live_state(app, |state| {
        // The terminal outcome we both stamp AND propagate to the chain's ancestors.
        let outcome = MiniCoderOutcome::failed(reason.clone());
        // A launch failure usually carries no agent_id (apply_launched, which stamps it,
        // never ran), but read the LIVE value so a session is closed if one was upserted.
        let agent_id = state
            .mini_coder_directives
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| d.agent_id.clone());
        stamp_terminal_and_propagate(state, &id, &outcome, agent_id.as_deref(), |d| {
            mini_coder::apply_failed(d, reason.clone())
        });
    });
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
    if let Some(d) = state.visual_check_directives.iter_mut().find(|d| d.id == id) {
        if let Ok(next) = apply(d) {
            *d = next;
        }
    }
}

/// NITPICK 1: bound the directive queue ONCE per write pass. Called at the end of
/// every `mutate_agent_live_state` closure that touches directives, so the eviction
/// (oldest TERMINAL only) runs a single time per persisted write rather than per
/// `transition_directive`.
fn cap_pass(state: &mut crate::backend::model::AgentLiveState) {
    mini_coder::cap_directives(&mut state.mini_coder_directives, MAX_DIRECTIVES);
    state.visual_check_directives =
        crate::backend::visual_check::cap_directives(std::mem::take(&mut state.visual_check_directives));
}

/// WARNING 5: like [`cap_pass`] but never evicts the `protect` ids this pass — used by
/// the finalize/propagation paths so a chain root (and its AwaitingRetry ancestors) just
/// stamped terminal in THIS mutate survives the cap until the poll can read its outcome.
fn cap_pass_protecting(state: &mut crate::backend::model::AgentLiveState, protect: &[String]) {
    mini_coder::cap_directives_protecting(&mut state.mini_coder_directives, MAX_DIRECTIVES, protect);
    state.visual_check_directives =
        crate::backend::visual_check::cap_directives(std::mem::take(&mut state.visual_check_directives));
}

/// WARNING 5: the set of ids freshly stamped terminal in a finalize/propagation mutate
/// — the leaf `id` plus every AwaitingRetry ancestor `propagate_terminal_to_ancestors`
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
        // After propagation the ancestors are no longer AwaitingRetry (they were stamped
        // terminal), so recompute the lineage by the shared root rather than by status.
        let root = mini_coder::chain_root_id(&leaf);
        for d in &state.mini_coder_directives {
            if d.id != leaf_id
                && (d.id == root || d.parent_directive_id.as_deref() == Some(root))
            {
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
) {
    if let Some(session) = state.sessions.iter_mut().find(|s| s.agent_id == agent_id) {
        session.status = "active".into();
        session.parent_agent_id = Some(parent_agent_id.to_string());
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
        role: "coder".into(),
        model: None,
        status: "active".into(),
        client: Some(client.to_string()),
        message: Some("Mini-coder running".into()),
        current_project_id,
        current_task_id: None,
        current_file_path: None,
        first_seen_at: Some(started_at.to_string()),
        last_seen_at: Some(started_at.to_string()),
        launch_token_hash: None,
        launch_token_issued_at: None,
        session_token_hash: None,
        session_token_issued_at: None,
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
    }
}

/// `mini-<parentShort>-<id8>`: a stable, allowlist-safe id (`[A-Za-z0-9._-]`).
fn mini_agent_id(directive: &MiniCoderDirective) -> String {
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
        MiniCoderBackendKind::Omlx => "omlx",
        MiniCoderBackendKind::AppleFm => "appleFm",
    }
    .to_string()
}

/// Resolve the MCP roots (`management_root`, `projects_dir`) the codex backend's
/// bounded `oracle_context` grant needs — the SAME wiring a full coder gets. Best
/// effort: `None` if the projects dir can't be resolved, in which case the codex
/// backend simply gets NO oracle grant (the mini still runs, just without Oracle).
fn resolve_mcp_roots(app: &AppHandle) -> Option<McpRoots> {
    let projects_dir = super::projects::ensure_projects_dir(app).ok()?;
    let management_root = agents::management_root_for_mcp(app, &projects_dir);
    Some(McpRoots {
        management_root,
        projects_dir,
    })
}

/// The two roots a future read-only `oracle_context` MCP scope would be built from.
/// MINOR 9: the fields are currently UNREAD — `build_mini_command` drops `mcp_roots`
/// so a mini gets NO MCP grant today. Kept (plumbed) for the future narrow scope.
// TODO: read these when wiring a READ-ONLY oracle_context-only MCP scope.
#[allow(dead_code)]
struct McpRoots {
    management_root: PathBuf,
    projects_dir: PathBuf,
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
) -> Result<(), String> {
    let result_target = scratch_root.join(result_rel_path.replace('\\', "/"));
    // Front-load the file scope + contents into the prompt (bounded per file).
    let prompt = build_mini_prompt(backend, directive, project_root, &result_target);
    let MiniCommandBuild {
        prompt_file,
        key_file,
        command,
    } = build_mini_command(backend, project_root, &result_target, &prompt, mcp_roots)?;
    let sessions = match app.try_state::<crate::backend::agent_pty::AgentPtySessions>() {
        Some(s) => s,
        None => {
            // The launched shell never ran, so it cannot delete the temp files.
            remove_mini_temp_files(prompt_file.as_deref(), key_file.as_deref());
            return Err("Agent terminal state is unavailable.".to_string());
        }
    };
    match crate::backend::agent_pty::spawn_agent_pty(app, &sessions, agent_id, command) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Spawn failed -> the wrapper never ran to delete the temp files.
            remove_mini_temp_files(prompt_file.as_deref(), key_file.as_deref());
            Err(e)
        }
    }
}

/// max-recall FIX 10: remove BOTH restricted temp files a built mini command owns — the
/// prompt file AND the OPTIONAL oMLX key file (each in its own 0600 dir). Called on every
/// pre-/at-spawn failure path in [`spawn_one_shot_mini`], where the in-script
/// wrapper/trap never ran to delete them. Centralized (not inlined per arm) so the two
/// failure arms can't diverge and the key-file cleanup can't be forgotten when the future
/// key config field lands. A `None` path is a no-op.
fn remove_mini_temp_files(prompt_file: Option<&Path>, key_file: Option<&Path>) {
    if let Some(path) = prompt_file {
        super::projects::remove_restricted_temp_file(path);
    }
    if let Some(path) = key_file {
        super::projects::remove_restricted_temp_file(path);
    }
}

/// Hard cap on the bytes of each named file we front-load into the mini prompt.
/// Generous for a single source file the coder names; a runaway file is truncated
/// so the prompt (and the one PowerShell/sh `-Command` argv that carries the
/// SCRIPT — not the prompt) stays bounded.
const MAX_PROMPT_FILE_BYTES: usize = 32 * 1024;
/// Max number of named files front-loaded with full contents (extras are listed
/// by path only) so a directive naming hundreds of files can't blow up the prompt.
const MAX_PROMPT_FILES: usize = 20;

/// Build the fixed instruction prompt the mini runs. Front-loads the file scope
/// (paths + bounded contents read from the project root), an anti-destructive
/// constraints block, the EXACT result schema, and — for codex with `allow_oracle`
/// — a bounded `oracle_context` grant. The mini is told to either WRITE the result
/// JSON to `<resultPath>` then exit (codex, which can write files) or OUTPUT ONLY a
/// single JSON object (ollama/api, whose stdout the wrapper captures into the file).
///
/// PURE w.r.t. spawning: it only READS the named files (bounded). Contains NO
/// secret. The task + file contents are NOT secrets, but are still delivered over
/// stdin (never argv) by `build_mini_command`.
fn build_mini_prompt(
    backend: &MiniCoderBackend,
    directive: &MiniCoderDirective,
    project_root: &Path,
    result_target: &Path,
) -> String {
    let backend_can_write_file = matches!(backend.kind, MiniCoderBackendKind::Codex);
    // MINOR 9: a mini gets NO MCP/oracle grant (the full aspis-management server is
    // mutation-capable; a one-shot helper must not reach it). It works PURELY from
    // the front-loaded file scope below. `directive.allow_oracle` stays plumbed for a
    // future read-only oracle scope but does NOT advertise any tool today.
    let _ = directive.allow_oracle; // INERT — see build_mini_command's MINOR 9 doc.
    let result_path_display = result_target.to_string_lossy();

    let mut prompt = String::new();
    prompt.push_str(
        "You are a one-shot mini-coder helper invoked by a senior coder agent. \
Do EXACTLY the task below, on ONLY the listed files, then finish. You run once \
and exit; you cannot ask follow-up questions interactively.\n\n",
    );
    prompt.push_str("TASK:\n");
    prompt.push_str(directive.task.trim());
    prompt.push_str("\n\n");

    // Explicit file scope, with bounded contents front-loaded.
    prompt.push_str("FILE SCOPE (operate on ONLY these files):\n");
    if directive.files.is_empty() {
        prompt.push_str("(no files named — do not touch any file; if the task needs a file, report needs_clarification)\n");
    } else {
        for (idx, rel) in directive.files.iter().enumerate() {
            prompt.push_str("- ");
            prompt.push_str(rel);
            prompt.push('\n');
            if idx < MAX_PROMPT_FILES {
                if let Some(contents) = read_prompt_file(project_root, rel) {
                    prompt.push_str("```\n");
                    prompt.push_str(&contents);
                    if !contents.ends_with('\n') {
                        prompt.push('\n');
                    }
                    prompt.push_str("```\n");
                }
            }
        }
        if directive.files.len() > MAX_PROMPT_FILES {
            prompt.push_str(
                "(remaining files listed by path only; read them yourself if needed and allowed)\n",
            );
        }
    }
    prompt.push('\n');

    // Anti-destructive constraints (PROMPT-ONLY — not an OS sandbox).
    prompt.push_str(
        "HARD CONSTRAINTS (safety — you MUST obey):\n\
- NEVER run destructive commands: no `rm -rf`, no force-push, no broad/recursive deletes.\n\
- NEVER delete, move, or create files outside the FILE SCOPE above.\n\
- NEVER make network writes, installs, or external calls.\n\
- Do ONLY the single task; do not refactor, reformat, or touch unrelated code.\n\
- If you create or change a self-contained .html artifact, include it in filesTouched so the parent coder can run visual_check for visual feedback.\n\
- If the task is ambiguous or unsafe, do NOT guess: report needs_clarification.\n\n",
    );

    // MINOR 9: the mini has NO tools/MCP. Tell it to work from the front-loaded
    // context only (the file contents above) — do not promise an oracle it cannot use.
    prompt.push_str(
        "CONTEXT: You have NO external tools. Work ONLY from the file contents \
front-loaded above; do not attempt to call tools, browse, or fetch more context.\n\n",
    );

    // Result contract.
    prompt.push_str("RESULT (your FINAL action):\n");
    prompt.push_str(
        "Report your result as a SINGLE JSON object with this schema:\n\
{\"status\":\"done\"|\"needs_clarification\", \"output\":\"short summary\", \
\"filesTouched\":[\"path\",...], \"question\":\"...only if needs_clarification...\", \
\"partial\":\"...optional...\"}\n",
    );
    if backend_can_write_file {
        prompt.push_str("WRITE this JSON object to the file at:\n");
        prompt.push_str(&result_path_display);
        prompt.push_str("\nthen exit. Write NOTHING else to that file.\n");
    } else {
        prompt.push_str(
            "OUTPUT this JSON object to stdout and NOTHING ELSE (no prose, no code fences, \
no logs). Output exactly one JSON object, then stop.\n",
        );
    }
    prompt
}

/// Read a named file's contents for the prompt, confined to the project root,
/// bounded to `MAX_PROMPT_FILE_BYTES`. Returns `None` on any error / path escape /
/// non-UTF-8 (the mini still gets the path; it can read the file itself if allowed).
fn read_prompt_file(project_root: &Path, rel: &str) -> Option<String> {
    let normalized = rel.replace('\\', "/");
    // Reject traversal/absolute before joining.
    let path = Path::new(&normalized);
    for component in path.components() {
        match component {
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
            _ => {}
        }
    }
    let target = project_root.join(&normalized);
    if !target.starts_with(project_root) {
        return None;
    }
    // WARNING 3: the lexical `starts_with` guard above is symlink-blind — a `files`
    // entry inside the project root that resolves to a SYMLINK pointing outside the
    // root would otherwise front-load an arbitrary file into the prompt. Canonicalize
    // BOTH the root and the target and require the real target to stay under the real
    // root before reading (mirrors `read_result_outcome`'s canonicalize-after-open).
    // A target that won't canonicalize (missing / broken link) -> None (skip it).
    let (canon_root, canon_target) = match (
        std::fs::canonicalize(project_root),
        std::fs::canonicalize(&target),
    ) {
        (Ok(root), Ok(tgt)) => (root, tgt),
        _ => return None,
    };
    if !canon_target.starts_with(&canon_root) {
        return None;
    }
    // Read from the canonicalized path so the bytes come from the verified target.
    let bytes = std::fs::read(&canon_target).ok()?;
    let truncated = if bytes.len() > MAX_PROMPT_FILE_BYTES {
        &bytes[..MAX_PROMPT_FILE_BYTES]
    } else {
        &bytes[..]
    };
    String::from_utf8(truncated.to_vec()).ok()
}

/// Build the per-kind one-shot command. Returns a [`MiniCommandBuild`] carrying the
/// `CommandBuilder` AND the restricted temp-file paths the caller must clean up if the
/// SPAWN itself fails (the in-script wrapper/trap never ran to delete them):
///   - `prompt_file`: the 0600 temp file holding the prompt (delivered over STDIN, never
///     argv); the wrapper reads it, DELETES it, then pipes it to the backend.
///   - `key_file`: the OPTIONAL 0600 temp file holding the oMLX bearer token (its PATH
///     rides in an env var, never argv/logs). max-recall FIX 10: it is returned here too
///     (it lives in its OWN restricted dir, distinct from the prompt dir) so EVERY
///     spawn-failure path in `spawn_one_shot_mini` removes BOTH files. Previously only the
///     prompt file left this fn, so a pre-spawn failure leaked the 0600 token file. `None`
///     today (no key config field yet — `omlx_api_key` returns None) but wired leak-free.
///
/// Per kind:
///   - codex: `codex exec` (prompt over stdin, `-m <model>` if set). The mini
///     WRITES its result JSON to `resultPath` itself. MINOR 9: a mini gets NO MCP
///     grant (see below) — it works from the front-loaded prompt context only.
///   - ollama: `ollama run <model>` (prompt over stdin; text-only, no tools). The
///     wrapper captures the model's stdout and normalizes it into `resultPath`.
///   - api: the configured CLI `command` (prompt over stdin). Same stdout->file
///     wrapper as ollama. The API key comes from the CLI's own ENV, never argv.
///
/// MINOR 9 (security scope): we DELIBERATELY do NOT wire any MCP grant to a mini.
/// The previous codex `-c mcp_servers.aspis-management...` grant pointed at the FULL
/// `aspis-management` MCP server, which exposes mutation tools (project edits,
/// `spawn_mini_coder`, `censor_dispose`) — far more than the read-only
/// `oracle_context` a one-shot helper should ever have. Until a READ-ONLY,
/// oracle_context-ONLY MCP scope exists, the mini works purely from the front-loaded
/// prompt context. `mcp_roots` stays in the signature (plumbed) but is INERT.
// TODO: wire a READ-ONLY oracle_context-only MCP scope before enabling the grant.
/// The result of [`build_mini_command`]: the launch command plus the restricted temp
/// files the SPAWN caller must clean up on a spawn failure (the in-script cleanup never
/// ran). Both paths are `Option` because the prompt file is always present once built but
/// the key file is only present for an oMLX backend with a configured token (none today).
struct MiniCommandBuild {
    prompt_file: Option<PathBuf>,
    key_file: Option<PathBuf>,
    command: CommandBuilder,
}

fn build_mini_command(
    backend: &MiniCoderBackend,
    project_root: &Path,
    result_target: &Path,
    prompt: &str,
    mcp_roots: Option<&McpRoots>,
) -> Result<MiniCommandBuild, String> {
    // The prompt goes to a restricted temp file (0600). It is NOT a secret, but
    // keeping it off argv matches the agent-launch contract and avoids argv-length
    // / quoting issues with large multi-file prompts.
    let prompt_file = super::projects::write_restricted_prompt_file(prompt)?;
    // oMLX-P2: OPTIONAL bearer token. There is NO config field for it yet (no P2/P3 UI),
    // so `omlx_api_key` returns None today — but the full mechanism is wired so a future
    // field just works: the token (when present) goes to a 0600 RESTRICTED file whose
    // PATH rides in an env var (never argv/PTY/logs); the launch script reads it and
    // sends `Authorization: Bearer <token>`; the Windows `finally` / macOS trap remove
    // the file on every exit path. Absent ⇒ no file ⇒ no env ⇒ no header.
    let key_file = match omlx_api_key(backend) {
        Some(token) => match super::projects::write_restricted_prompt_file(&token) {
            Ok(path) => Some(path),
            Err(e) => {
                super::projects::remove_restricted_temp_file(&prompt_file);
                return Err(e);
            }
        },
        None => None,
    };
    // MINOR 9: the mini gets NO MCP grant. `mcp_roots` is intentionally dropped here
    // (None passed down) so build_mini_command_impl wires NO `-c mcp_servers...`
    // flags for ANY kind. Keep the binding referenced so the plumbing stays live for
    // the future read-only scope without an unused-arg warning.
    let _ = mcp_roots; // INERT — see the MINOR 9 doc comment above + the TODO.
    let cmd = build_mini_command_impl(
        backend,
        project_root,
        result_target,
        &prompt_file,
        key_file.as_deref(),
        None,
    );
    match cmd {
        Ok(command) => Ok(MiniCommandBuild {
            prompt_file: Some(prompt_file),
            key_file,
            command,
        }),
        Err(e) => {
            super::projects::remove_restricted_temp_file(&prompt_file);
            if let Some(key_file) = key_file.as_deref() {
                super::projects::remove_restricted_temp_file(key_file);
            }
            Err(e)
        }
    }
}

/// oMLX-P2: resolve the OPTIONAL oMLX bearer token for a backend. Returns `None`
/// today — there is no config field/UI for an oMLX key yet (the local server is
/// usually unauthenticated). The mechanism around it (restricted file + env + script
/// header + cleanup) is fully wired, so adding a config field later only needs to make
/// this function return the configured token. The token (if any) is NEVER logged or
/// placed on argv. `backend` is accepted now so the future field read needs no
/// signature change.
fn omlx_api_key(backend: &MiniCoderBackend) -> Option<String> {
    let _ = backend; // no key field yet — see the doc comment.
    None
}

#[cfg(windows)]
fn build_mini_command_impl(
    backend: &MiniCoderBackend,
    project_root: &Path,
    result_target: &Path,
    prompt_file: &Path,
    key_file: Option<&Path>,
    mcp_roots: Option<&McpRoots>,
) -> Result<CommandBuilder, String> {
    let prompt_path = ps_single_quote(&prompt_file.to_string_lossy());
    let result_path = ps_single_quote(&result_target.to_string_lossy());
    // WARNING 7: a sibling temp file for the backend's RAW stdout, so we never hold
    // the (potentially huge) output in a PowerShell string. It lives next to the
    // result file inside the same scratch dir and is removed in the `finally`. The
    // `.raw` suffix is on the directive's result path so it stays under the scratch
    // root (the result path was traversal-validated by claim_and_launch).
    let raw_path = ps_single_quote(&format!("{}.raw", result_target.to_string_lossy()));

    // FIX 1 (source-content leak): define the prompt file / its restricted parent dir
    // / the raw capture path BEFORE the try, then read the prompt INSIDE the try so a
    // failing `Get-Content` (ErrorActionPreference=Stop) can no longer skip cleanup.
    // The `finally` ALWAYS removes the source-bearing prompt dir AND the `.raw`
    // capture, on success OR on any error in the body. NEVER `Write-Host $prompt`
    // (B1: no prompt on the PTY stream).
    let preamble = format!(
        "$ErrorActionPreference='Stop'\n\
$promptFile = {prompt_path}\n\
$promptDir = [System.IO.Path]::GetDirectoryName($promptFile)\n\
$rawFile = {raw_path}\n\
try {{\n\
$prompt = Get-Content -Raw -LiteralPath $promptFile\n"
    );

    let body = match backend.kind {
        MiniCoderBackendKind::Codex => {
            // codex exec: prompt piped over stdin (read from `-`), -m if set. The mini
            // WRITES the result file itself. MINOR 9: minis get NO MCP grant (the
            // `mcp_roots` arg is intentionally unused — see build_mini_command).
            let _ = mcp_roots; // MINOR 9: inert until a read-only oracle scope exists.
            let mut args: Vec<String> = vec!["exec".to_string()];
            if let Some(model) = backend.model.as_deref() {
                if !model.trim().is_empty() {
                    args.push("-m".to_string());
                    args.push(model.trim().to_string());
                }
            }
            let arg_list = args
                .iter()
                .map(|a| ps_single_quote(a))
                .collect::<Vec<_>>()
                .join(", ");
            // `$prompt | & codex @codexArgs`: prompt on STDIN, never argv.
            format!("$codexArgs = @({arg_list})\n$prompt | & codex @codexArgs\n")
        }
        MiniCoderBackendKind::Ollama => {
            let model = backend
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .ok_or_else(|| "ollama backend requires a model tag".to_string())?;
            let model_q = ps_single_quote(model);
            // `$prompt | & ollama run <model>`: prompt on STDIN. `& ollama run` uses
            // the call operator because the executable + args are OUR fixed tokens
            // (no operator-supplied command line to tokenize). Capture stdout into the
            // result file via the shared wrapper.
            let run = format!("$prompt | & ollama run {model_q}");
            windows_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::Api => {
            let command = backend
                .command
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .ok_or_else(|| "api backend requires a command line".to_string())?;
            // BLOCKER 1 / WARNING 5: the `command` is a TRUSTED, operator-configured
            // shell command LINE — the same trust model as a `customAgentClients`
            // command (see projects::build_windows_agent_script's custom branch). We
            // therefore interpolate it VERBATIM as a pipeline target WITHOUT the `&`
            // call operator, so PowerShell tokenizes the whole line itself
            // (`mycli chat --json` runs `mycli` with args `chat --json`). Using
            // `& {command}` would treat the entire multi-word string as a single
            // executable NAME and fail. The prompt is piped over stdin; the API key
            // comes from the CLI's OWN env — never injected by us, never on argv.
            let run = format!("$prompt | {command}");
            windows_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::Omlx => {
            // oMLX-P2: the one-shot script POSTs an OpenAI chat-completion to the
            // loopback oMLX server ITSELF (Invoke-RestMethod), emits the model's text
            // on stdout, and the EXISTING wrapper extracts the MiniCoderResult JSON —
            // exactly as for ollama/api (Option A: keep the PTY). model + base_url are
            // REQUIRED (validated in oMLX-P1; re-checked here so a hand-edited config
            // fails cleanly instead of building a bad request).
            let model = backend
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .ok_or_else(|| "omlx backend requires a model".to_string())?;
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .ok_or_else(|| "omlx backend requires a base URL".to_string())?;
            // OPTIONAL bearer token (oMLX is usually unauthenticated). There is no
            // config field for it yet (no UI in P2/P3), so `omlx_api_key` returns None
            // today — but the whole key-on-a-restricted-file mechanism below is wired so
            // a future field just works (key absent ⇒ no Authorization header). The key
            // is NEVER on argv: it rides a 0600 file whose PATH is passed via env, read
            // inside the script, and removed in the `finally`.
            let key_env = key_file.map(|_| OMLX_KEY_FILE_ENV);
            let run = build_omlx_run_windows(base_url, model, key_env);
            windows_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::AppleFm => {
            return Err("Apple on-device requires macOS 27+.".to_string());
        }
    };

    // FIX 1: close the try opened in the preamble and ALWAYS run cleanup in the
    // `finally` — the source-bearing prompt dir AND the `.raw` capture are removed on
    // success OR any error (so a failed Get-Content / backend can no longer leak the
    // restricted prompt file on disk). SilentlyContinue: an already-removed file is fine.
    //
    // F5: codex does NOT use `windows_stdout_to_result_wrapper`, so it never writes the
    // `.raw` file; guard the removal with `Test-Path` so the cleanup targets a file that
    // actually exists (the wrapper backends still get their raw capture removed).
    //
    // F4: the key-dir cleanup is emitted ONLY for a keyed spawn. Non-keyed (and
    // non-oMLX) scripts no longer carry the `if ($env:OMLX_KEY_FILE) { … }` collateral.
    // The keyed path still removes the token's restricted parent dir on EVERY exit; the
    // PATH rides in `$env:OMLX_KEY_FILE` (set on the CommandBuilder below, never argv),
    // and the script never echoes it.
    let key_cleanup = if key_file.is_some() {
        format!(
            "  if ($env:{OMLX_KEY_FILE_ENV}) {{ Remove-Item -LiteralPath ([System.IO.Path]::GetDirectoryName($env:{OMLX_KEY_FILE_ENV})) -Recurse -Force -ErrorAction SilentlyContinue }}\n"
        )
    } else {
        String::new()
    };
    let finally = format!(
        "}}\n\
finally {{\n\
  Remove-Item -LiteralPath $promptDir -Recurse -Force -ErrorAction SilentlyContinue\n\
  if (Test-Path -LiteralPath $rawFile) {{ Remove-Item -LiteralPath $rawFile -Force -ErrorAction SilentlyContinue }}\n\
{key_cleanup}\
}}\n"
    );
    let script = format!("{preamble}{body}{finally}exit 0");
    let mut cmd = CommandBuilder::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    cmd.cwd(project_root);
    // oMLX-P2: pass the OPTIONAL key file PATH via env (never argv/PTY). The script
    // reads the token from this file and sends `Authorization: Bearer <token>`; if
    // unset (no key configured) it omits the header entirely.
    if let Some(key_file) = key_file {
        cmd.env(OMLX_KEY_FILE_ENV, key_file.as_os_str());
    }
    Ok(cmd)
}

/// oMLX-P2 (Windows): build the `$run` pipeline that POSTs an OpenAI chat-completion
/// to the loopback oMLX server and writes the model's answer to stdout, which the
/// EXISTING `windows_stdout_to_result_wrapper` then extracts into a `MiniCoderResult`.
///
/// INJECTION-SAFETY (critical): the prompt is the `$prompt` PowerShell variable read
/// from the restricted file in the preamble; it is passed as a VALUE into a hashtable
/// and JSON-encoded by `ConvertTo-Json`. It is NEVER string-concatenated into the JSON
/// body, so no prompt content can break out of the JSON string and forge fields.
///
/// `model` and `base_url` are OUR tokens (validated in oMLX-P1: model is a bare tag,
/// base_url is a normalized loopback origin), embedded single-quoted via
/// `ps_single_quote`. `base_url` already has any trailing slash stripped (P1), so
/// `<base>/chat/completions` never double-slashes.
///
/// FAILURE = SILENCE: the whole request is wrapped in `try { … } catch { }` so ANY
/// connection/HTTP/parse error writes NOTHING to stdout. The wrapper then finds no
/// valid JSON and writes the clean `{"status":"failed",...}` fallback — a non-2xx
/// response (Invoke-RestMethod throws) yields the SAME clean fallback, never partial
/// garbage in the result file.
///
/// `key_env`: when `Some`, the launch script reads the bearer token from the file
/// pointed to by that env var and sends an `Authorization: Bearer <token>` header;
/// when `None`, no auth header is emitted. The token never appears on argv.
#[cfg(windows)]
fn build_omlx_run_windows(base_url: &str, model: &str, key_env: Option<&str>) -> String {
    let model_q = ps_single_quote(model);
    let uri_q = ps_single_quote(&format!("{base_url}/chat/completions"));
    // F3: cap the HTTP request so a stalled oMLX server fails fast (Invoke-RestMethod
    // throws on timeout -> the try/catch swallows it -> clean `failed` fallback) instead
    // of holding the PTY until the wall-clock kill. Derived from the SAME constant as the
    // macOS python timeout (wall-clock cap minus a margin).
    let http_timeout = omlx_http_timeout_secs();
    // Optional Authorization header. The token is read from the env-passed key FILE
    // (never argv/log); if the env var is unset we send no header. `$headers` defaults
    // to an empty hashtable, so `-Headers $headers` is always valid.
    let header_block = match key_env {
        Some(env) => format!(
            "$headers = @{{}}\n\
if ($env:{env}) {{\n\
  $omlxKey = (Get-Content -Raw -LiteralPath $env:{env}).Trim()\n\
  if ($omlxKey) {{ $headers['Authorization'] = 'Bearer ' + $omlxKey }}\n\
  $omlxKey = $null\n\
}}\n"
        ),
        None => "$headers = @{}\n".to_string(),
    };
    // The prompt rides as a VALUE (`content = $prompt`) — ConvertTo-Json encodes it;
    // NEVER `'\"content\":\"' + $prompt`. -Compress keeps the body one line.
    //
    // The whole try/catch is wrapped in a `& { … }` script block so that the shared
    // `windows_stdout_to_result_wrapper`'s `{run} > $rawFile` redirects the ENTIRE
    // block's output stream (the `Write-Output $content`) — not just the last
    // statement. This keeps the wrapper UNCHANGED (same idiom as the single-pipeline
    // ollama/api `$run`).
    format!(
        "& {{\n\
try {{\n\
{header_block}\
$body = @{{ model = {model_q}; messages = @(@{{ role = 'user'; content = $prompt }}); stream = $false; temperature = 0.1 }} | ConvertTo-Json -Depth 6 -Compress\n\
$resp = Invoke-RestMethod -Method Post -Uri {uri_q} -ContentType 'application/json' -Headers $headers -Body $body -TimeoutSec {http_timeout}\n\
$content = $resp.choices[0].message.content\n\
if ($null -ne $content) {{ Write-Output $content }}\n\
}} catch {{ }}\n\
}}"
    )
}

/// Windows wrapper: run `$run` (a pipeline that writes the backend's answer to
/// stdout), redirect that stdout to a bounded RAW temp file (WARNING 7 — never hold
/// all output in a PS string / cap memory), then normalize it into a
/// `MiniCoderResult` JSON at `$result_path`.
///
/// BLOCKER 2 + MINOR 10: extraction is a BALANCED-BRACE walk, not first-`{` /
/// last-`}`. We strip ANSI CSI **and** OSC/DCS/APC/PM/SOS escape payloads (ollama
/// spinners can carry `{`/`}` inside an OSC), then for EACH `{` we attempt to parse
/// the balanced `{...}` candidate starting there (honoring JSON string literals so
/// a `}` inside `"output":"foo() {bar}"` does not end the object early). The FIRST
/// candidate that parses AND has a valid `status` wins; none -> best-effort
/// `failed`. This stops trailing prose `}` from downgrading a valid `done`.
///
/// B1: nothing sensitive is on argv; the prompt was on stdin.
#[cfg(windows)]
fn windows_stdout_to_result_wrapper(run: &str, result_path: &str, raw_path: &str) -> String {
    // WARNING 7: read the RAW file with a bounded byte cap so a runaway backend
    // cannot OOM us. Mirrors mini_coder::MAX_RESULT_BYTES (1 MiB).
    // NOTE: `$rawFile` is (re)declared here so the wrapper is self-contained — it is
    // also invoked standalone (e.g. by the balanced-walk behavioral test) WITHOUT the
    // build_mini_command_impl preamble, so it must not assume an externally-set var.
    let max_bytes = super::mini_coder::MAX_RESULT_BYTES;
    format!(
        "$rawFile = {raw_path}\n\
# WARNING 7: redirect the backend's stdout to a temp FILE (not a PS string).\n\
{run} > $rawFile 2>$null\n\
$out = $null\n\
try {{\n\
  # Read with a BOM-detecting StreamReader: Windows PowerShell's `>` writes UTF-16\n\
  # LE+BOM, while an external CLI's bytes are decoded via the console encoding — a\n\
  # detecting reader handles both. Bounded to MAX_RESULT_BYTES chars so a runaway\n\
  # backend cannot OOM us; loop because a single Read may return fewer chars.\n\
  $sr = New-Object System.IO.StreamReader($rawFile, $true)\n\
  try {{\n\
    $cap = {max_bytes}\n\
    $cbuf = New-Object char[] $cap\n\
    $total = 0\n\
    while ($total -lt $cap) {{\n\
      $n = $sr.Read($cbuf, $total, $cap - $total)\n\
      if ($n -le 0) {{ break }}\n\
      $total += $n\n\
    }}\n\
  }} finally {{ $sr.Close() }}\n\
  $raw = New-Object string($cbuf, 0, $total)\n\
  # MINOR 10: strip OSC/DCS/APC/PM/SOS payloads, then CSI escapes.\n\
  $clean = [regex]::Replace($raw, \"\\x1b\\][^\\x07\\x1b]*(\\x07|\\x1b\\\\)\", '')\n\
  $clean = [regex]::Replace($clean, \"\\x1b[P_^X][^\\x1b]*\\x1b\\\\\", '')\n\
  $clean = [regex]::Replace($clean, \"\\x1b\\[[0-9;?]*[A-Za-z]\", '')\n\
  # BLOCKER 2: balanced-brace walk. For each '{{' try the balanced object there.\n\
  for ($i = 0; $i -lt $clean.Length -and $null -eq $out; $i++) {{\n\
    if ($clean[$i] -ne '{{') {{ continue }}\n\
    $depth = 0; $inStr = $false; $esc = $false; $end = -1\n\
    for ($j = $i; $j -lt $clean.Length; $j++) {{\n\
      $ch = $clean[$j]\n\
      if ($inStr) {{\n\
        if ($esc) {{ $esc = $false }}\n\
        elseif ($ch -eq '\\') {{ $esc = $true }}\n\
        elseif ($ch -eq '\"') {{ $inStr = $false }}\n\
      }} else {{\n\
        if ($ch -eq '\"') {{ $inStr = $true }}\n\
        elseif ($ch -eq '{{') {{ $depth++ }}\n\
        elseif ($ch -eq '}}') {{ $depth--; if ($depth -eq 0) {{ $end = $j; break }} }}\n\
      }}\n\
    }}\n\
    if ($end -lt 0) {{ continue }}\n\
    $candidate = $clean.Substring($i, $end - $i + 1)\n\
    try {{\n\
      $parsed = $candidate | ConvertFrom-Json\n\
      if ($parsed.status -eq 'done' -or $parsed.status -eq 'needs_clarification') {{\n\
        $out = $candidate\n\
      }}\n\
    }} catch {{ }}\n\
  }}\n\
}} catch {{ $out = $null }}\n\
Remove-Item -LiteralPath $rawFile -Force -ErrorAction SilentlyContinue\n\
if ($null -eq $out) {{\n\
  $out = '{{\"status\":\"failed\",\"output\":\"mini backend produced no valid JSON result\"}}'\n\
}}\n\
[System.IO.File]::WriteAllText({result_path}, $out, (New-Object System.Text.UTF8Encoding $false))\n"
    )
}

/// PURE, platform-agnostic builder for the macOS `/bin/sh` cleanup preamble. Kept
/// uncfg'd (no `target_os` gate) so it is unit-testable on the Windows dev host.
///
/// FIX (BLOCKER): the paths are ASSIGNED to shell variables FIRST, then referenced
/// DOUBLE-QUOTED inside the trap's single-quoted body. The arguments `prompt_dir_q`
/// and `raw_path_q` are already `sh_single_quote_local`-wrapped (they expand to
/// `'…'`), so the assignment RHS is correctly quoted for ANY path (spaces, quotes).
/// Putting the wrapped paths directly inside the trap's own single-quoted string
/// (the previous code) terminated the outer trap delimiter on the first embedded
/// `'`, making the trap a shell syntax error for any space/quote-containing path —
/// so the `EXIT` trap never armed and the source-bearing prompt dir + `.raw` capture
/// leaked on disk. With the variable indirection only `$_MINI_*` expansion happens
/// at EXIT time, inside the still-intact single-quoted trap body.
///
/// The trap is armed BEFORE `set -e` so it fires even if a later command aborts under
/// `set -e`. `_MINI_RAW_FILE` is always set (a non-existent file in `rm -rf` is a
/// no-op, so the codex backend — which writes no `.raw` — is unaffected).
///
/// oMLX-P2: `key_dir_q` is the OPTIONAL restricted parent dir of the bearer-token
/// file. When `Some`, it is ALSO removed by the trap on every exit path (success /
/// error / `set -e` abort) so the token never lingers on disk. The token itself is
/// NEVER referenced here — only the directory path, which is not secret. `None` (no
/// key configured) leaves the trap removing only the prompt dir + raw capture.
//
// Used by the macOS `build_mini_command_impl` arm and by the platform-agnostic test;
// on a non-test, non-macOS build it is unreferenced, hence the conditional allow.
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
fn build_macos_trap_preamble(prompt_dir_q: &str, raw_path_q: &str, key_dir_q: Option<&str>) -> String {
    // `_MINI_KEY_DIR` is always assigned (empty when no key) so the trap body is a single
    // fixed string. max-recall FIX 9: GUARD the key-dir removal on a non-empty value —
    // `rm -rf ""` is POSIX-undefined on an empty operand (some shells treat it as the cwd),
    // so the no-key case (`_MINI_KEY_DIR=''`) must NOT reach `rm`. The prompt-dir and
    // raw-file removals stay unconditional (those paths are always set).
    let key_assign = match key_dir_q {
        Some(q) => format!("_MINI_KEY_DIR={q}\n"),
        None => "_MINI_KEY_DIR=''\n".to_string(),
    };
    format!(
        "_MINI_PROMPT_DIR={prompt_dir_q}\n\
_MINI_RAW_FILE={raw_path_q}\n\
{key_assign}\
trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true' EXIT\n\
set -e\n"
    )
}

/// PURE, platform-agnostic single-quote for embedding a value inside the macOS
/// `/bin/sh -c` script. Mirrors `sh_single_quote_local` but is uncfg'd so the oMLX
/// macOS-script builder (and its test) work on the Windows dev host.
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
fn sh_single_quote_portable(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// oMLX-P2 (macOS): build the `$run` block that POSTs an OpenAI chat-completion to the
/// loopback oMLX server via a `python3` + stdlib `urllib.request` heredoc and prints
/// `choices[0].message.content` on stdout. Its stdout is captured by the EXISTING
/// `macos_stdout_to_result_wrapper`, which extracts the MiniCoderResult JSON.
///
/// INJECTION-SAFETY: the prompt is read from the file at `$MINI_PROMPT_FILE` (path via
/// ENV, never argv) and the request body is built with `json.dumps`, so prompt content
/// is JSON-encoded by the encoder and can never break out of the JSON string. The base
/// URL, prompt path, and OPTIONAL key file path ALL ride in ENV vars — nothing on argv.
///
/// FAILURE = SILENCE: any exception (connection refused, non-2xx → `HTTPError`, missing
/// field, non-JSON body) prints NOTHING and exits, so the wrapper finds no valid JSON
/// and writes the clean `{"status":"failed",...}` fallback (no partial garbage).
///
/// `has_key`: when true the script reads the bearer token from the file at
/// `$OMLX_KEY_FILE` and adds `Authorization: Bearer <token>`; otherwise no auth header.
///
/// Kept uncfg'd (platform-agnostic) so it is unit-testable on the Windows dev host,
/// like `build_macos_trap_preamble`. The inner heredoc uses the `OMLXEOF` delimiter so
/// it never collides with the wrapper's own `PYEOF` heredoc.
/// Python payload for [`build_omlx_run_macos`]'s heredoc, kept as a module-scope
/// RAW string so its indentation survives verbatim. Inside a `format!` literal
/// the `\n\` line continuations strip each following line's leading whitespace,
/// which silently flattens the Python block structure and makes the script die
/// with `IndentationError` at runtime (found on the first real macOS run).
/// `@OMLX_KEY_FILE_ENV@` / `@OMLX_TIMEOUT_ENV@` / `@OMLX_TIMEOUT_DEFAULT@` are
/// substituted by the builder.
const OMLX_RUN_MACOS_PY: &str = r#"import os, json
import urllib.request, urllib.error
try:
    with open(os.environ['MINI_PROMPT_FILE'], 'r', encoding='utf-8') as f:
        prompt = f.read()
    body = json.dumps({
        'model': os.environ['OMLX_MODEL'],
        'messages': [{'role': 'user', 'content': prompt}],
        'stream': False,
        'temperature': 0.1,
    }).encode('utf-8')
    req = urllib.request.Request(os.environ['OMLX_URL'], data=body, method='POST')
    req.add_header('Content-Type', 'application/json')
    key_path = os.environ.get('@OMLX_KEY_FILE_ENV@')
    if key_path:
        with open(key_path, 'r', encoding='utf-8') as kf:
            token = kf.read().strip()
        if token:
            req.add_header('Authorization', 'Bearer ' + token)
    timeout = int(os.environ.get('@OMLX_TIMEOUT_ENV@', '@OMLX_TIMEOUT_DEFAULT@'))
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.loads(resp.read().decode('utf-8', 'replace'))
    content = data['choices'][0]['message']['content']
    if content is not None:
        import sys
        sys.stdout.write(content)
except Exception:
    pass
"#;

#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
fn build_omlx_run_macos(
    base_url: &str,
    model: &str,
    prompt_path_q: &str,
    has_key: bool,
) -> String {
    let url_q = sh_single_quote_portable(&format!("{base_url}/chat/completions"));
    let model_q = sh_single_quote_portable(model);
    // python reads OMLX_KEY_FILE only when present; the empty-string default means "no
    // key" (the `if key_path:` guard below skips the header). Never echo the token.
    let key_export = if has_key {
        format!("export {OMLX_KEY_FILE_ENV}\n")
    } else {
        // Ensure no stale env leaks in: explicitly clear so an inherited value can't
        // forge a header. (Belt-and-suspenders; the executor only sets it when keyed.)
        format!("unset {OMLX_KEY_FILE_ENV}\n")
    };
    // F2: the HTTP timeout (seconds) is derived from the SAME wall-clock cap as the PTY
    // kill (minus a margin) and rides a non-secret env var, so a stalled request aborts
    // JUST UNDER the cap with a clean `failed` fallback. The python default mirrors the
    // derived value so the two never silently diverge.
    let http_timeout = omlx_http_timeout_secs();
    // Export the base URL, model and prompt path for python (all via env, never argv).
    // `OMLX_MODEL` carries OUR validated bare tag; still passed via env for symmetry and
    // to keep argv empty.
    let py = OMLX_RUN_MACOS_PY
        .replace("@OMLX_KEY_FILE_ENV@", OMLX_KEY_FILE_ENV)
        .replace("@OMLX_TIMEOUT_ENV@", OMLX_TIMEOUT_ENV)
        .replace("@OMLX_TIMEOUT_DEFAULT@", &http_timeout.to_string());
    format!(
        "OMLX_URL={url_q}\nexport OMLX_URL\n\
OMLX_MODEL={model_q}\nexport OMLX_MODEL\n\
MINI_PROMPT_FILE={prompt_path_q}\nexport MINI_PROMPT_FILE\n\
{OMLX_TIMEOUT_ENV}={http_timeout}\nexport {OMLX_TIMEOUT_ENV}\n\
{key_export}\
python3 - <<'OMLXEOF'\n{py}OMLXEOF\n"
    )
}

#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
fn build_apple_fm_run_macos(prompt_pipe: &str, fm_path: &str, model: Option<&str>) -> String {
    let mut parts = vec![sh_single_quote_portable(fm_path), "respond".to_string()];
    if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
        parts.push("--model".to_string());
        parts.push(sh_single_quote_portable(model));
    }
    format!("{prompt_pipe} | {}", parts.join(" "))
}

#[cfg(target_os = "macos")]
fn build_mini_command_impl(
    backend: &MiniCoderBackend,
    project_root: &Path,
    result_target: &Path,
    prompt_file: &Path,
    key_file: Option<&Path>,
    mcp_roots: Option<&McpRoots>,
) -> Result<CommandBuilder, String> {
    // WARNING 6: use `/bin/sh` UNCONDITIONALLY (do not read the unvalidated $SHELL).
    let prompt_path = sh_single_quote_local(&prompt_file.to_string_lossy());
    let result_path = sh_single_quote_local(&result_target.to_string_lossy());
    // The prompt's per-launch restricted PARENT dir (WARNING 8: cleaned by the trap,
    // not leaked on disk). Removing the dir recursively also removes the prompt file.
    let prompt_dir = sh_single_quote_local(
        &prompt_file
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    // WARNING 7: a sibling RAW stdout file (next to the result file, under the
    // scratch root) so we never capture the backend's stdout into a shell variable
    // (which truncates silently at ARG_MAX ~128KB and is unbounded in memory).
    let raw_path = sh_single_quote_local(&format!("{}.raw", result_target.to_string_lossy()));

    // FIX 1: deliver the prompt by piping the restricted FILE directly into the
    // backend (`cat {prompt_path} | ...`) so the bytes are preserved VERBATIM — the
    // old `PROMPT="$(cat ...)"` capture silently stripped trailing newlines and
    // mutated the prompt. `cat` keeps the prompt off argv (B1: never on the PTY
    // stream / process args).
    let prompt_pipe = format!("cat {prompt_path}");

    // FIX 1 (source-content leak): a `trap ... EXIT` as the FIRST line guarantees the
    // restricted prompt dir (which front-loads SOURCE CODE) AND the `.raw` stdout
    // capture are ALWAYS removed on ANY exit — success, `set -e` abort, a missing
    // `cat`, or a missing `python3`. The old code deleted the prompt AFTER the read,
    // so a `set -e` abort before that line leaked the source-bearing file on disk.
    //
    // BLOCKER FIX: the trap body references the paths via DOUBLE-QUOTED shell
    // variables assigned before the trap, so a path containing whitespace/quotes (e.g.
    // `/Users/the owner/My Project/`) no longer terminates the trap's own single-quoted
    // delimiter and break the trap. See `build_macos_trap_preamble`.
    //
    // oMLX-P2: the OPTIONAL bearer-token file's restricted parent dir is added to the
    // trap so the token is removed on EVERY exit path. `key_file`'s parent is the
    // per-launch `*.d` restricted dir created by `create_restricted_temp_file`.
    let key_dir = key_file.map(|p| {
        sh_single_quote_local(
            &p.parent()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    });
    let preamble = build_macos_trap_preamble(&prompt_dir, &raw_path, key_dir.as_deref());

    let body = match backend.kind {
        MiniCoderBackendKind::Codex => {
            // MINOR 9: minis get NO MCP grant (mcp_roots intentionally unused — see
            // build_mini_command). codex writes the result file itself.
            let _ = mcp_roots; // MINOR 9: inert until a read-only oracle scope exists.
            let mut args: Vec<String> = vec!["exec".to_string()];
            if let Some(model) = backend.model.as_deref() {
                if !model.trim().is_empty() {
                    args.push("-m".to_string());
                    args.push(model.trim().to_string());
                }
            }
            let arg_line = args
                .iter()
                .map(|a| sh_single_quote_local(a))
                .collect::<Vec<_>>()
                .join(" ");
            // prompt on STDIN (piped from the file), never argv. `arg_line` already
            // leads with `exec` (so this is `codex exec [-m model]`).
            format!("{prompt_pipe} | codex {arg_line}\n")
        }
        MiniCoderBackendKind::Ollama => {
            let model = backend
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .ok_or_else(|| "ollama backend requires a model tag".to_string())?;
            // ollama run <tag>: our fixed tokens (the tag is validated to a bare
            // token), prompt on STDIN (piped from the file). Capture stdout via the
            // shared file wrapper.
            let run = format!(
                "{prompt_pipe} | ollama run {}",
                sh_single_quote_local(model)
            );
            macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::Api => {
            let command = backend
                .command
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .ok_or_else(|| "api backend requires a command line".to_string())?;
            // BLOCKER 1 / WARNING 5: `command` is a TRUSTED, operator-configured shell
            // command LINE — the same trust model as a `customAgentClients` command
            // (see projects::build_macos_agent_script's custom branch). It is placed
            // VERBATIM as a pipeline target so `/bin/sh` tokenizes the whole line
            // (`mycli chat --json` runs `mycli` with args `chat --json`). The prompt
            // is piped over stdin; the API key comes from the CLI's OWN env — never
            // injected by us, never on argv.
            let run = format!("{prompt_pipe} | {command}");
            macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::Omlx => {
            // oMLX-P2 (macOS): the one-shot script POSTs an OpenAI chat-completion to
            // the loopback oMLX server via a `python3`+`urllib` heredoc (stdlib only —
            // NO curl/jq), prints `choices[0].message.content` on stdout, and the
            // EXISTING wrapper extracts the MiniCoderResult JSON (Option A: keep PTY).
            // model + base_url REQUIRED (validated in oMLX-P1; re-checked for a clean
            // failure on a hand-edited config).
            let model = backend
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .ok_or_else(|| "omlx backend requires a model".to_string())?;
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .ok_or_else(|| "omlx backend requires a base URL".to_string())?;
            // The prompt path, base URL (+ optional key file path) ride in ENV vars —
            // NEVER on argv. The token never leaves the 0600 file; python reads it.
            let run = build_omlx_run_macos(base_url, model, &prompt_path, key_file.is_some());
            macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
        MiniCoderBackendKind::AppleFm => {
            let fm = crate::backend::provider_detect::resolve_program("fm")
                .ok_or_else(|| "Apple on-device requires macOS 27+.".to_string())?;
            let fm_path = fm.to_string_lossy();
            let run = build_apple_fm_run_macos(&prompt_pipe, fm_path.as_ref(), backend.model.as_deref());
            macos_stdout_to_result_wrapper(&run, &result_path, &raw_path)
        }
    };

    let script = format!("{preamble}{body}exit 0");
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args(["-c", &script]);
    cmd.cwd(project_root);
    // oMLX-P2: the OPTIONAL key file PATH rides in env (never argv/PTY). python reads
    // the token from this file and sends `Authorization: Bearer <token>`; unset ⇒ no
    // header. The base URL + prompt path are exported inline inside the `$run` block.
    if let Some(key_file) = key_file {
        cmd.env(OMLX_KEY_FILE_ENV, key_file.as_os_str());
    }
    Ok(cmd)
}

/// macOS wrapper: run `$run`, redirect its stdout to a bounded RAW temp FILE
/// (WARNING 7 — never into a shell var, which truncates at ARG_MAX and is unbounded
/// in memory), then normalize it into a `MiniCoderResult` at `$result_path`.
///
/// BLOCKER 2 + MINOR 10: python3 strips ANSI CSI **and** OSC/DCS/APC/PM/SOS escape
/// payloads, then does a PROGRESSIVE `json.JSONDecoder().raw_decode(clean, i)` at
/// each `{` index — the FIRST candidate that decodes to a dict with a valid `status`
/// wins. This is a true balanced parse (a `}` inside `"output":"foo() {bar}"` is
/// handled by the JSON grammar), so trailing prose `}` cannot downgrade a `done`.
/// python3 ships on macOS dev setups (the Oracle runtime already requires python);
/// the result/raw paths ride in env vars so nothing is on argv.
/// Python payload for [`macos_stdout_to_result_wrapper`]'s heredoc, kept as a
/// module-scope RAW string so its indentation survives verbatim (same
/// `IndentationError` pitfall as [`OMLX_RUN_MACOS_PY`] — `\n\` continuations in
/// a `format!` literal strip the next line's leading whitespace). `@MAX_BYTES@`
/// is substituted by the wrapper.
#[cfg(target_os = "macos")]
const MACOS_RESULT_EXTRACTOR_PY: &str = r#"import os, re, json
out = None
try:
    with open(os.environ['MINI_RAW_FILE'], 'rb') as f:
        raw = f.read(@MAX_BYTES@).decode('utf-8', 'replace')
    # MINOR 10: strip OSC/DCS/APC/PM/SOS payloads, then CSI escapes.
    clean = re.sub(r'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)', '', raw)
    clean = re.sub(r'\x1b[P_^X][^\x1b]*\x1b\\', '', clean)
    clean = re.sub(r'\x1b\[[0-9;?]*[A-Za-z]', '', clean)
    dec = json.JSONDecoder()
    i = 0
    n = len(clean)
    while i < n and out is None:
        if clean[i] != '{':
            i += 1
            continue
        try:
            obj, _end = dec.raw_decode(clean, i)
            if isinstance(obj, dict) and obj.get('status') in ('done', 'needs_clarification'):
                out = clean[i:_end]
        except Exception:
            pass
        i += 1
except Exception:
    out = None
try:
    os.remove(os.environ['MINI_RAW_FILE'])
except Exception:
    pass
if out is None:
    out = json.dumps({'status': 'failed', 'output': 'mini backend produced no valid JSON result'})
with open(os.environ['MINI_RESULT'], 'w', encoding='utf-8') as f:
    f.write(out)
"#;

#[cfg(target_os = "macos")]
fn macos_stdout_to_result_wrapper(run: &str, result_path: &str, raw_path: &str) -> String {
    let py = MACOS_RESULT_EXTRACTOR_PY
        .replace("@MAX_BYTES@", &super::mini_coder::MAX_RESULT_BYTES.to_string());
    format!(
        "MINI_RAW_FILE={raw_path}\nexport MINI_RAW_FILE\nMINI_RESULT={result_path}\nexport MINI_RESULT\n\
# WARNING 7: redirect the backend's stdout to a temp FILE (not a shell var).\n\
{{ {run} ; }} > \"$MINI_RAW_FILE\" 2>/dev/null || true\n\
python3 - <<'PYEOF'\n{py}PYEOF\n"
    )
}

/// macOS-only single-quote for embedding inside the `/bin/sh -c` script. Mirrors
/// `projects::sh_single_quote` (kept local so the executor does not depend on a
/// private projects fn): wrap in single quotes, escape embedded quotes via `'\''`.
#[cfg(target_os = "macos")]
fn sh_single_quote_local(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn build_mini_command_impl(
    backend: &MiniCoderBackend,
    _project_root: &Path,
    _result_target: &Path,
    _prompt_file: &Path,
    _key_file: Option<&Path>,
    _mcp_roots: Option<&McpRoots>,
) -> Result<CommandBuilder, String> {
    if backend.kind == MiniCoderBackendKind::AppleFm {
        return Err("Apple on-device requires macOS 27+.".into());
    }
    Err("Mini-coder is supported on Windows and macOS only.".into())
}

// ---------------------------------------------------------------------------
// P5: human Stop button -> mini_coder_kill (record killRequested THEN kill PTY)
// ---------------------------------------------------------------------------

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
        .map(|d| (mini_coder::chain_root_id(d).to_string(), d.status));
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
    // is what carries the PTY; an AwaitingRetry predecessor is flagged too so a racing
    // re-finalize honours the abort). A terminal chain-mate is left untouched.
    for d in state.mini_coder_directives.iter_mut() {
        let in_chain = d.id == root || d.parent_directive_id.as_deref() == Some(root.as_str());
        if in_chain && !d.status.is_terminal() {
            d.kill_requested = true;
        }
    }

    // WARNING 6 (KILL STALE AGENT_ID): the PTY to kill belongs to the chain's ACTIVE
    // (`Launching|Running`) attempt — NOT necessarily the directive matched by `agent_id`.
    // If the human hit Stop via an AwaitingRetry PREDECESSOR's stale agent id, that
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
    //    the human stopped via an AwaitingRetry predecessor's STALE id (a dead past mini);
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
mod tests {
    use super::*;
    use super::mini_coder::MiniCoderResult;

    fn directive(id: &str, parent: &str) -> MiniCoderDirective {
        MiniCoderDirective {
            id: id.into(),
            parent_agent_id: parent.into(),
            status: MiniCoderStatus::Pending,
            task: "docstring foo()".into(),
            files: vec!["src/a.rs".into()],
            backend: None,
            allow_oracle: false,
            kill_requested: false,
            result_path: format!("{id}.json"),
            agent_id: None,
            created_at: "2026-06-06T00:00:00Z".into(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: None,
            attempt: 0,
            parent_directive_id: None,
            retry_directive_id: None,
        }
    }

    #[test]
    fn mini_agent_id_is_allowlist_safe_and_namespaced() {
        let d = directive("abcd1234ef", "coder-1717459200000");
        let id = mini_agent_id(&d);
        assert!(id.starts_with("mini-"));
        // Only [A-Za-z0-9._-] (matches agent_pty::validate_agent_id allowlist).
        assert!(
            crate::backend::agent_pty::validate_agent_id(&id).is_ok(),
            "id: {id}"
        );
        // Parent short is the alnum head (no '-'): "coder171".
        assert!(id.contains("coder171"), "id: {id}");
        assert!(id.contains("abcd1234"), "id: {id}");
    }

    #[test]
    fn mini_agent_id_handles_empty_components() {
        let d = directive("", "");
        let id = mini_agent_id(&d);
        assert_eq!(id, "mini-p-x");
        assert!(crate::backend::agent_pty::validate_agent_id(&id).is_ok());
    }

    #[test]
    fn parent_is_gone_detects_absent_and_closed() {
        use crate::backend::model::AgentLiveState;
        let mut state = AgentLiveState {
            version: 2,
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
            git_push_requests: Vec::new(),
            plan_approval_requests: Vec::new(),
        };
        // Absent parent -> gone.
        assert!(parent_is_gone(&state, "coder-1"));
        // Active parent -> not gone.
        state.sessions.push(test_session("coder-1", "active"));
        assert!(!parent_is_gone(&state, "coder-1"));
        // Closed parent -> gone.
        state.sessions.push(test_session("coder-2", "closed"));
        assert!(parent_is_gone(&state, "coder-2"));
    }

    #[test]
    fn result_rel_path_traversal_is_rejected_before_launch() {
        // WARNING 4: claim_and_launch validates directive.result_path with the SAME
        // gate the result reader uses; a `..`/absolute path must be rejected (the
        // claim fails) so the write/read target can never escape the scratch dir.
        assert!(mini_coder::validate_result_rel_path("../../etc/passwd").is_err());
        assert!(mini_coder::validate_result_rel_path("sub/../../escape.json").is_err());
        #[cfg(windows)]
        assert!(mini_coder::validate_result_rel_path("C:\\Windows\\x.json").is_err());
        #[cfg(not(windows))]
        assert!(mini_coder::validate_result_rel_path("/etc/passwd").is_err());
        // A normal relative path under the scratch dir is accepted.
        assert!(mini_coder::validate_result_rel_path("d1.json").is_ok());
        assert!(mini_coder::validate_result_rel_path("nested/d1.json").is_ok());
    }

    #[test]
    fn read_result_outcome_missing_file_is_failed() {
        let dir = std::env::temp_dir().join(format!("mc_exec_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let outcome = read_result_outcome(&dir, "nope.json");
        assert_eq!(outcome.status, MiniCoderStatus::Failed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_outcome_valid_done_after_canonicalize() {
        let dir = std::env::temp_dir().join(format!("mc_exec_done_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("d1.json"),
            r#"{"status":"done","output":"ok","filesTouched":["src/a.rs"]}"#,
        )
        .unwrap();
        let outcome = read_result_outcome(&dir, "d1.json");
        assert_eq!(
            outcome.status,
            MiniCoderStatus::Done,
            "err: {:?}",
            outcome.error
        );
        assert_eq!(outcome.output.as_deref(), Some("ok"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn result_json_with_hostile_output_round_trips_through_reader() {
        // A result whose output contains a double-quote, a backslash, and a newline
        // must serialize to VALID JSON (serde_json escaping) and read back to a clean
        // `done` outcome whose output is EXACT. Guards the result-file contract the
        // ollama/api stdout wrapper and the codex self-write both target.
        use super::super::mini_coder::MiniCoderResult;
        let output = "fixed \"foo\" in C:\\src\\a.rs\nran tests";
        let result = MiniCoderResult {
            status: "done".to_string(),
            output: Some(output.to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed["status"], "done");
        assert_eq!(parsed["output"], output);

        // And the executor's own read path resolves it to a `done` with the output.
        let dir = std::env::temp_dir().join(format!("mc_hostile_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("h.json"), &json).unwrap();
        let outcome = read_result_outcome(&dir, "h.json");
        assert_eq!(
            outcome.status,
            MiniCoderStatus::Done,
            "err: {:?}",
            outcome.error
        );
        assert_eq!(outcome.output.as_deref(), Some(output));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_reads_persisted_scratch_path_not_a_live_lookup() {
        // BLOCKER 3: the result is read from the scratch root PERSISTED on the
        // directive (`scratch_path`), so a parent that switched projects after launch
        // cannot redirect the read. We assert the invariant directly: the result lives
        // in dir A (the launch-time scratch on the directive); a DIFFERENT dir B (the
        // hypothetical post-switch project) does NOT contain it. `read_result_outcome`
        // keyed on the persisted dir A finds it; keyed on dir B fails.
        let a = std::env::temp_dir().join(format!("mc_scratch_a_{}", std::process::id()));
        let b = std::env::temp_dir().join(format!("mc_scratch_b_{}", std::process::id()));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("r.json"), r#"{"status":"done","output":"in A"}"#).unwrap();

        let mut d = directive("r", "coder-1");
        d.status = MiniCoderStatus::Running;
        d.result_path = "r.json".into();
        d.scratch_path = Some(a.to_string_lossy().to_string());

        // The persisted dir (A) is what finalize uses.
        let persisted = PathBuf::from(d.scratch_path.as_deref().unwrap());
        assert_eq!(persisted, a);
        let from_a = read_result_outcome(&persisted, &d.result_path);
        assert_eq!(from_a.status, MiniCoderStatus::Done);
        assert_eq!(from_a.output.as_deref(), Some("in A"));
        // A re-resolution to the switched project (B) would NOT find the result.
        let from_b = read_result_outcome(&b, &d.result_path);
        assert_eq!(from_b.status, MiniCoderStatus::Failed);

        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    // INTEGRATION (Windows): the REAL headless one-shot backend. Build the command
    // the executor builds, spawn it through portable-pty exactly as `spawn_agent_pty`
    // does, drive the master until the child writes its result file and EOFs, then
    // assert the file holds a valid `done` result AND `read_result_outcome` resolves
    // it to MiniCoderStatus::Done with the task echoed. Proves spawn -> one-shot ->
    // result-file -> EOF -> read WITHOUT a full Tauri AppHandle (the loop's lock
    // plumbing is covered by the pure unit tests above + the Python poll tests).
    // Ignored by default; run locally with `cargo test -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "spawns a real PTY child; run locally with --ignored"]
    fn headless_one_shot_writes_result_and_eofs() {
        use portable_pty::PtySize;
        use std::io::{Read, Write};
        use std::time::Instant;

        let scratch = std::env::temp_dir().join(format!("mc_oneshot_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let result_target = scratch.join("d1.json");
        let project_root = std::env::temp_dir();
        // BLOCKER 2: a hostile task (double-quote + backslash + newline) must survive
        // the real PTY one-shot write and read back EXACTLY via serde_json escaping.
        let task = "docstring \"foo\" in C:\\src\\a.rs\nplease";

        let cmd = build_headless_mini_command(&project_root, &result_target, task).unwrap();

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 32,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);

        // Answer ConPTY's startup DSR so the child's render pipeline does not stall.
        let mut writer = pair.master.take_writer().expect("writer");
        let _ = writer.write_all(b"\x1b[1;1R");
        let _ = writer.flush();

        // Drain the master on a reader thread until EOF (the one-shot exits).
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        // Poll for the result file to appear (the child writes it then exits).
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !result_target.exists() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(result_target.exists(), "mini must write its result file");

        // Close the master so the reader EOFs, then reap the child (no zombie).
        drop(pair.master);
        let join_deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() && Instant::now() < join_deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = handle.join();
        let _ = child.wait();

        // The executor's read path resolves it to a clean `done` outcome.
        let outcome = read_result_outcome(&scratch, "d1.json");
        assert_eq!(
            outcome.status,
            MiniCoderStatus::Done,
            "err: {:?}",
            outcome.error
        );
        assert_eq!(outcome.output.as_deref(), Some(task));
        std::fs::remove_dir_all(&scratch).ok();
    }

    #[test]
    fn upsert_mini_session_stamps_parent_project_so_the_rail_groups_it() {
        // The mini must carry its coder's project, or ProjectsView.sessionsByProject
        // (keyed on current_project_id) filters it out and it never reaches the rail.
        let mut state = crate::backend::model::AgentLiveState {
            version: 2,
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
            git_push_requests: Vec::new(),
            plan_approval_requests: Vec::new(),
        };
        upsert_mini_session(
            &mut state,
            "mini-c-1",
            "coder-1",
            Some("p1".into()),
            "2026-06-06T00:00:00Z",
            "ollama",
        );
        let mini = state
            .sessions
            .iter()
            .find(|s| s.agent_id == "mini-c-1")
            .expect("mini session inserted");
        assert_eq!(mini.current_project_id.as_deref(), Some("p1"));
        assert_eq!(mini.parent_agent_id.as_deref(), Some("coder-1"));
        assert_eq!(mini.client.as_deref(), Some("ollama"));
        assert_eq!(mini.host.as_deref(), Some(super::super::agents::HOST_APP));

        // A later re-upsert with a TRANSIENT None must NOT clear the resolved project
        // (a momentarily-empty parent snapshot mustn't drop the mini from the rail).
        upsert_mini_session(
            &mut state,
            "mini-c-1",
            "coder-1",
            None,
            "2026-06-06T00:01:00Z",
            "ollama",
        );
        let mini = state
            .sessions
            .iter()
            .find(|s| s.agent_id == "mini-c-1")
            .unwrap();
        assert_eq!(
            mini.current_project_id.as_deref(),
            Some("p1"),
            "transient None cleared the project"
        );
    }

    fn empty_state() -> crate::backend::model::AgentLiveState {
        crate::backend::model::AgentLiveState {
            version: 2,
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
            git_push_requests: Vec::new(),
            plan_approval_requests: Vec::new(),
        }
    }

    #[test]
    fn snapshot_parent_project_reads_purely_from_the_given_snapshot() {
        // BLOCKER 1: the parent's project is resolved from the SAME pass snapshot
        // plan_tick saw — a later mutation of the live state must NOT change the value
        // the mini is launched with. We model that by resolving twice: once against
        // the captured snapshot (the pass snapshot), once against a mutated one. The
        // captured one wins (it is the value passed into claim_and_launch).
        let mut snapshot = empty_state();
        let mut parent = test_session("coder-1", "active");
        parent.current_project_id = Some("p1".into());
        snapshot.sessions.push(parent);

        // The value the executor pins for this pass.
        assert_eq!(
            snapshot_parent_project(&snapshot, "coder-1").as_deref(),
            Some("p1")
        );

        // A LATER mutation (parent switches project / goes None) is on a DIFFERENT
        // snapshot and cannot retroactively change the pinned value above.
        let mut later = snapshot.clone();
        later.sessions[0].current_project_id = Some("p2".into());
        assert_eq!(
            snapshot_parent_project(&later, "coder-1").as_deref(),
            Some("p2")
        );
        // The pass snapshot is unchanged — the mini still launches into p1.
        assert_eq!(
            snapshot_parent_project(&snapshot, "coder-1").as_deref(),
            Some("p1")
        );
    }

    /// WARNING 3 (REDUNDANT SNAPSHOTS + TOCTOU): project_id AND trusted are derived from
    /// ONE snapshot. The pure resolver maps the directive's agent_id -> session ->
    /// current_project_id, then feeds THAT id to the trust lookup — the two can never
    /// diverge (findings for p1 / trust for p2 was the bug).
    #[test]
    fn resolve_project_and_trust_derives_both_from_one_snapshot() {
        let mut snapshot = empty_state();
        let mut sess = test_session("mini-c-d1", "active");
        sess.current_project_id = Some("p1".into());
        snapshot.sessions.push(sess);
        let mut d = directive("d1", "coder-1");
        d.agent_id = Some("mini-c-d1".into());

        // The trust lookup is called with EXACTLY the project resolved from the snapshot.
        let mut seen: Option<String> = None;
        let (pid, trusted) = resolve_project_and_trust(Some(&snapshot), &d, |p| {
            seen = Some(p.to_string());
            p == "p1" // trusted for p1
        });
        assert_eq!(pid.as_deref(), Some("p1"));
        assert!(trusted, "p1 is trusted");
        assert_eq!(seen.as_deref(), Some("p1"), "trust checked for the SAME project");
    }

    /// WARNING 3: a missing snapshot / agent_id / session yields (None, false) and never
    /// invokes the trust lookup — fail-closed (never lint an unresolvable tree).
    #[test]
    fn resolve_project_and_trust_fails_closed_when_unresolvable() {
        let d = directive("d1", "coder-1"); // agent_id = None
        let mut called = false;
        let (pid, trusted) = resolve_project_and_trust(None, &d, |_p| {
            called = true;
            true
        });
        assert_eq!(pid, None);
        assert!(!trusted);
        assert!(!called, "trust lookup never runs for an unresolvable project");
    }

    /// BLOCKER 2 (TIMEOUT EXCLUSION): a directive whose deferred-verdict thread is in
    /// flight is Running with a long-elapsed started_at but must NOT be timed out.
    /// Without the exclusion it WOULD be (control). With it in the set, it is spared.
    #[test]
    fn plan_tick_excludes_inflight_verdict_directive_from_timeout() {
        use std::collections::HashSet;
        let mut d = directive("d1", "coder-1");
        d.status = MiniCoderStatus::Running;
        d.started_at = Some("2026-06-06T00:00:00Z".into());
        let directives = vec![d];
        let now = "2026-06-06T01:00:00Z"; // an hour later -> well past any cap.

        // Control: NOT in flight -> timed out.
        let plan = mini_coder::plan_tick_excluding(
            &directives,
            now,
            DEFAULT_WALL_CLOCK_CAP_SECS,
            mini_coder::DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            2,
            &HashSet::new(),
        );
        assert_eq!(plan.timeouts, vec!["d1".to_string()], "control: times out");

        // In flight -> excluded from timeouts.
        let inflight: HashSet<String> = ["d1".to_string()].into_iter().collect();
        let plan2 = mini_coder::plan_tick_excluding(
            &directives,
            now,
            DEFAULT_WALL_CLOCK_CAP_SECS,
            mini_coder::DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            2,
            &inflight,
        );
        assert!(
            plan2.timeouts.is_empty(),
            "a directive awaiting its verdict thread is NOT wall-cap-timed-out"
        );
    }

    /// BLOCKER 2 (IN-FLIGHT GUARD): only ONE verdict thread per directive can be claimed;
    /// a second claim for the same id fails until released. This is what stops `run_pass`
    /// from re-detecting + re-spawning (double-finalizing) the same finished mini.
    #[test]
    fn verdict_inflight_guard_prevents_double_claim_and_clears_on_release() {
        let state = MiniCoderState::new();
        assert!(state.claim_verdict("d1"), "first claim succeeds");
        assert!(!state.claim_verdict("d1"), "second claim for same id is refused");
        assert!(state.verdict_inflight_ids().contains("d1"));
        // A different id is independent.
        assert!(state.claim_verdict("d2"));
        // Release clears the guard so a future (legitimate) re-claim can proceed.
        state.release_verdict("d1");
        assert!(!state.verdict_inflight_ids().contains("d1"));
        assert!(state.claim_verdict("d1"), "re-claim after release succeeds");
    }

    /// BLOCKER 1: a POISONED `verdict_inflight` mutex must NOT silently disable the
    /// timeout-exclusion (return empty) nor no-op a release. After poisoning the lock from
    /// a panicking thread, `verdict_inflight_ids` still returns the LIVE set and
    /// `release_verdict` still removes — recovered via `into_inner`.
    #[test]
    fn verdict_inflight_recovers_from_poisoned_mutex() {
        let state = std::sync::Arc::new(MiniCoderState::new());
        // Two live ids before poisoning.
        assert!(state.claim_verdict("alive-1"));
        assert!(state.claim_verdict("alive-2"));

        // Poison the mutex: panic while holding the lock in another thread.
        let s2 = std::sync::Arc::clone(&state);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = s2.verdict_inflight.lock().unwrap();
            panic!("poison the verdict_inflight mutex");
        }));
        assert!(res.is_err(), "the panicking thread did panic");
        assert!(
            state.verdict_inflight.is_poisoned(),
            "the mutex is now poisoned"
        );

        // The fix: ids() recovers and returns the LIVE set (NOT empty — which would
        // permanently disable the timeout exclusion and time out awaiting-verdict
        // directives).
        let ids = state.verdict_inflight_ids();
        assert!(ids.contains("alive-1") && ids.contains("alive-2"), "live set survives poison: {ids:?}");

        // release still lands on a poisoned lock (a no-op release would leak forever).
        state.release_verdict("alive-1");
        assert!(!state.verdict_inflight_ids().contains("alive-1"), "release works under poison");

        // claim still inserts on a poisoned lock (a failing claim would re-spawn verdict
        // threads every pass).
        assert!(state.claim_verdict("alive-3"), "claim works under poison");
        assert!(state.verdict_inflight_ids().contains("alive-3"));
    }

    /// BLOCKER 2 (RAII): the in-flight id is released on EVERY exit path of the verdict
    /// thread body — even when BOTH the work closure AND the fail-closed closure panic.
    /// The `VerdictInflightGuard`'s `Drop` runs during unwind, so the id never leaks (a
    /// leaked id would make the directive un-timeout-able AND un-re-finalizable forever).
    #[test]
    fn verdict_thread_body_releases_id_on_double_panic() {
        let set: Arc<Mutex<std::collections::HashSet<String>>> =
            Arc::new(Mutex::new(std::collections::HashSet::new()));
        set.lock().unwrap().insert("d-double-panic".to_string());

        // Run the body on a thread so a panic that escapes (it must NOT) is contained,
        // and join it; the id must be released regardless.
        let set_for_thread = Arc::clone(&set);
        let join = std::thread::spawn(move || {
            run_verdict_thread_body(
                Some(set_for_thread),
                "d-double-panic".to_string(),
                || panic!("work panics"),
                || panic!("fail-closed ALSO panics"),
            );
        });
        let joined = join.join();
        assert!(joined.is_ok(), "the body must NOT let a double panic escape the thread");

        let ids = set.lock().unwrap();
        assert!(
            !ids.contains("d-double-panic"),
            "RAII guard released the id even on a double panic: {ids:?}"
        );
    }

    /// BLOCKER 2 (RAII): the guard also releases on the NORMAL (no-panic) path and on a
    /// work-only panic (fail-closed succeeds). Sanity over the happy + single-panic exits.
    #[test]
    fn verdict_thread_body_releases_id_on_normal_and_single_panic() {
        // Normal exit.
        let set: Arc<Mutex<std::collections::HashSet<String>>> =
            Arc::new(Mutex::new(std::collections::HashSet::new()));
        set.lock().unwrap().insert("ok".to_string());
        run_verdict_thread_body(Some(Arc::clone(&set)), "ok".to_string(), || {}, || {});
        assert!(!set.lock().unwrap().contains("ok"), "released on normal exit");

        // Work panics, fail-closed succeeds.
        set.lock().unwrap().insert("work-panic".to_string());
        run_verdict_thread_body(
            Some(Arc::clone(&set)),
            "work-panic".to_string(),
            || panic!("boom"),
            || {},
        );
        assert!(!set.lock().unwrap().contains("work-panic"), "released after work panic");
    }

    /// WARNING 6: the verdict thread threads the executor's REAL running/stop flag (so an
    /// in-flight linter run honors app exit) — NOT a throwaway `AtomicBool(true)`. The
    /// plumbing: `running_flag()` hands out a CLONE of the same `Arc<AtomicBool>` the loop
    /// uses, and `stop()` clears it, so a holder of the cloned flag observes the shutdown.
    #[test]
    fn verdict_stop_flag_is_the_executors_real_running_flag() {
        let state = MiniCoderState::new();
        let threaded = state.running_flag(); // the flag passed to real_censor_verdict.
        assert!(threaded.load(Ordering::SeqCst), "running flag starts true (alive)");
        // The executor signals shutdown -> the THREADED clone observes it (same Arc).
        state.stop();
        assert!(
            !threaded.load(Ordering::SeqCst),
            "stop() clears the flag the verdict thread holds -> linter run bails"
        );
        // And `real_censor_verdict` short-circuits on a cleared flag (no linter work) —
        // its guard is `if !stop.load(...) { return Vec::new() }`, exercised via the same
        // Arc. (No AppHandle harness exists in this crate, so the I/O path past the guard
        // is covered by the orchestrator's own `run_fine_batch_inner` early-return test.)
        assert!(!threaded.load(Ordering::SeqCst));
    }

    /// WARNING 3 (self-healing): the SAME reconcile logic the startup sweep uses is the
    /// one `run_pass` folds into every steady-state tick — so an AwaitingRetry whose retry
    /// child is TERMINAL is flagged for reconcile from the pass directives, not only at
    /// startup. (`reconcile_awaiting_retry_orphans` + `run_pass` apply this against the
    /// live state under the lock; here we assert the decision the steady-state pass acts
    /// on, since the crate has no AppHandle test harness.)
    #[test]
    fn steady_state_pass_reconciles_terminal_child_awaiting_retry() {
        let mut pred = directive("pred", "coder-1");
        pred.status = MiniCoderStatus::AwaitingRetry;
        pred.retry_directive_id = Some("retry".into());
        let mut child = directive("retry", "coder-1");
        child.status = MiniCoderStatus::Done; // terminal child, predecessor un-stamped.
        child.parent_directive_id = Some("pred".into());

        let directives = vec![pred, child];
        // This is EXACTLY what run_pass now passes to reconcile_awaiting_retry_orphans
        // every tick (folded in, not only at startup).
        let actions = mini_coder::awaiting_retry_needing_terminal(&directives);
        assert_eq!(actions.len(), 1, "the terminal-child AwaitingRetry is flagged");
        assert_eq!(actions[0].0, "pred");
        assert!(matches!(
            actions[0].1,
            mini_coder::RetrySweepAction::PropagateChildTerminal { ref child_id } if child_id == "retry"
        ));
    }

    #[test]
    fn snapshot_parent_project_is_none_when_parent_absent_or_projectless() {
        // Parent absent -> None (claim_and_launch fails the directive cleanly).
        let snapshot = empty_state();
        assert_eq!(snapshot_parent_project(&snapshot, "coder-1"), None);

        // Parent present but carrying no project -> None as well.
        let mut snapshot = empty_state();
        snapshot.sessions.push(test_session("coder-1", "active")); // current_project_id = None
        assert_eq!(snapshot_parent_project(&snapshot, "coder-1"), None);
    }

    #[test]
    fn close_mini_session_marks_done_so_the_rail_excludes_it() {
        // WARNING 3: after a mini directive reaches a terminal outcome, its SESSION is
        // closed (status "done") so isRecentProjectSession (TS) drops it from the rail
        // instead of letting it linger ~15min as a stale active agent.
        let mut state = empty_state();
        upsert_mini_session(
            &mut state,
            "mini-c-1",
            "coder-1",
            Some("p1".into()),
            "2026-06-06T00:00:00Z",
            "ollama",
        );
        assert_eq!(
            state
                .sessions
                .iter()
                .find(|s| s.agent_id == "mini-c-1")
                .unwrap()
                .status,
            "active"
        );
        close_mini_session(&mut state, "mini-c-1");
        assert_eq!(
            state
                .sessions
                .iter()
                .find(|s| s.agent_id == "mini-c-1")
                .unwrap()
                .status,
            "done"
        );
        // A missing session is a no-op (no panic).
        close_mini_session(&mut state, "nope");
    }

    // -- P5: killRequested WINS + mini_coder_kill order-of-operations ---------

    fn running_directive_with_scratch(id: &str, scratch: &std::path::Path) -> MiniCoderDirective {
        let mut d = directive(id, "coder-1");
        d.status = MiniCoderStatus::Running;
        d.agent_id = Some(format!("mini-c-{id}"));
        d.result_path = format!("{id}.json");
        d.scratch_path = Some(scratch.to_string_lossy().to_string());
        d
    }

    #[test]
    fn finalize_outcome_kill_requested_wins_over_present_done_file() {
        // P5 RACE: the mini wrote a valid `done` result file in the SAME instant the
        // human hit Stop. killRequested WINS — the outcome is aborted_by_human and the
        // result file is NOT even read (a racing done must never overwrite the abort).
        let dir = std::env::temp_dir().join(format!("mc_p5_killwin_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("k1.json"),
            r#"{"status":"done","output":"raced done"}"#,
        )
        .unwrap();

        let mut d = running_directive_with_scratch("k1", &dir);
        d.kill_requested = true;
        let outcome = finalize_outcome(&d);
        assert_eq!(
            outcome.status,
            MiniCoderStatus::AbortedByHuman,
            "human Stop must win the same-instant done; err: {:?}",
            outcome.error
        );
        // The mini's racing output must NOT leak into the abort outcome.
        assert_ne!(outcome.output.as_deref(), Some("raced done"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_outcome_kill_requested_no_file_is_aborted_not_failed() {
        // killRequested + NO result file -> aborted_by_human (NOT failed). The human
        // asserted control; absence of a result is not a mini failure here.
        let dir = std::env::temp_dir().join(format!("mc_p5_killnofile_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = running_directive_with_scratch("k2", &dir);
        d.kill_requested = true;
        let outcome = finalize_outcome(&d);
        assert_eq!(outcome.status, MiniCoderStatus::AbortedByHuman);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_outcome_no_kill_no_file_is_failed_unchanged() {
        // killRequested=false + no result file -> failed (the pre-P5 behavior, intact).
        let dir = std::env::temp_dir().join(format!("mc_p5_nokill_nofile_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = running_directive_with_scratch("k3", &dir); // kill_requested = false
        let outcome = finalize_outcome(&d);
        assert_eq!(outcome.status, MiniCoderStatus::Failed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_outcome_no_kill_with_done_file_is_done_unchanged() {
        // killRequested=false + a valid done file -> done (regression: the normal path).
        let dir = std::env::temp_dir().join(format!("mc_p5_nokill_done_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("k4.json"), r#"{"status":"done","output":"ok"}"#).unwrap();
        let d = running_directive_with_scratch("k4", &dir);
        let outcome = finalize_outcome(&d);
        assert_eq!(outcome.status, MiniCoderStatus::Done);
        assert_eq!(outcome.output.as_deref(), Some("ok"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn kill_requested_beats_timeout_in_transition_choice() {
        // The timeout path's locked closure consults the LIVE d.kill_requested: a
        // directive that BOTH blew its cap AND was Stopped reports aborted_by_human.
        let mut killed = directive("t1", "coder-1");
        killed.status = MiniCoderStatus::Running;
        killed.kill_requested = true;
        let aborted = if killed.kill_requested {
            mini_coder::apply_aborted(&killed, "stop").unwrap()
        } else {
            mini_coder::apply_timeout(&killed, "cap").unwrap()
        };
        assert_eq!(aborted.status, MiniCoderStatus::AbortedByHuman);

        // Without the kill flag the same code path yields timeout (unchanged).
        let mut not_killed = directive("t2", "coder-1");
        not_killed.status = MiniCoderStatus::Running;
        let timed = if not_killed.kill_requested {
            mini_coder::apply_aborted(&not_killed, "stop").unwrap()
        } else {
            mini_coder::apply_timeout(&not_killed, "cap").unwrap()
        };
        assert_eq!(timed.status, MiniCoderStatus::Timeout);
    }

    #[test]
    fn kill_requested_beats_parent_gone_in_transition_choice() {
        // The parent-gone path's closure consults the LIVE d.kill_requested too: a
        // human Stop overrides a parent-gone verdict (aborted, not failed).
        let mut killed = directive("pg1", "coder-1");
        killed.status = MiniCoderStatus::Running;
        killed.kill_requested = true;
        let aborted = if killed.kill_requested {
            mini_coder::apply_aborted(&killed, "stop").unwrap()
        } else {
            mini_coder::apply_failed(&killed, "parent gone").unwrap()
        };
        assert_eq!(aborted.status, MiniCoderStatus::AbortedByHuman);
    }

    #[test]
    fn mark_kill_requested_sets_flag_by_agent_id_before_any_kill() {
        // mini_coder_kill RECORDS killRequested (persisted) BEFORE the PTY kill. We
        // assert the recording step here: the flag is set on the directive whose
        // agentId matches, found, and idempotent on a re-mark.
        let mut state = empty_state();
        let mut d = directive("d1", "coder-1");
        d.status = MiniCoderStatus::Running;
        d.agent_id = Some("mini-c-d1".into());
        assert!(!d.kill_requested);
        state.mini_coder_directives.push(d);

        // Found + flagged. WARNING 6: returns the LIVE attempt's PTY id (here the matched
        // directive is itself the live attempt).
        assert_eq!(
            mark_kill_requested(&mut state, "mini-c-d1").as_deref(),
            Some("mini-c-d1")
        );
        assert!(state.mini_coder_directives[0].kill_requested);
        // Idempotent re-mark.
        assert_eq!(
            mark_kill_requested(&mut state, "mini-c-d1").as_deref(),
            Some("mini-c-d1")
        );
        assert!(state.mini_coder_directives[0].kill_requested);

        // WARNING 3: an unknown agentId (NOT a mini) is reported as None — the caller
        // must NOT kill any PTY for it (never kill a non-mini PTY).
        assert!(mark_kill_requested(&mut state, "mini-c-nope").is_none());
    }

    #[test]
    fn mini_coder_kill_has_no_vault_unlock_gate() {
        // FIX 2 (SAFETY OVERRIDE): Stop must work even when the vault is LOCKED, so
        // `mini_coder_kill` must NOT depend on BackendState/`ensure_unlocked`. We assert
        // the gate is structurally absent by reading this module's own source: the kill
        // command's body must not call `ensure_unlocked`, and the only retained gate is
        // the mini-only `mark_kill_requested`. (A behavioral call needs an AppHandle;
        // this guards against a future re-introduction of the lock gate.)
        let src = include_str!("mini_coder_executor.rs");
        let fn_start = src
            .find("pub fn mini_coder_kill(")
            .expect("mini_coder_kill defined");
        let fn_end = src[fn_start..]
            .find("\n}\n")
            .map(|i| fn_start + i)
            .expect("function body end");
        let body = &src[fn_start..fn_end];
        // Look for the ACTUAL gate (a `.ensure_unlocked()` call) and the BackendState
        // parameter — not the bare word, which legitimately appears in the doc/comment
        // explaining WHY the gate was removed.
        assert!(
            !body.contains(".ensure_unlocked()"),
            "mini_coder_kill must NOT call ensure_unlocked (safety override): {body}"
        );
        assert!(
            !body.contains("BackendState"),
            "mini_coder_kill must NOT take the vault BackendState (safety override): {body}"
        );
        assert!(
            body.contains("mark_kill_requested"),
            "mini_coder_kill must keep the mini-only kill gate: {body}"
        );
        assert!(
            body.contains("validate_agent_id"),
            "mini_coder_kill must keep agent-id validation: {body}"
        );
    }

    #[test]
    fn mark_kill_requested_skips_already_terminal_directive() {
        // WARNING 4: a mini that already reached a TERMINAL state must NOT get
        // killRequested set (terminal is terminal), and the fn reports not-found so the
        // caller does not kill a (non-existent) live PTY.
        let mut state = empty_state();
        let mut d = directive("d1", "coder-1");
        d.status = MiniCoderStatus::Done; // terminal
        d.agent_id = Some("mini-c-d1".into());
        state.mini_coder_directives.push(d);

        assert!(mark_kill_requested(&mut state, "mini-c-d1").is_none());
        assert!(
            !state.mini_coder_directives[0].kill_requested,
            "a terminal directive must NOT be flagged killRequested"
        );

        // A still-RUNNING mini IS flagged + reports its live PTY (contrast).
        let mut running = directive("d2", "coder-1");
        running.status = MiniCoderStatus::Running;
        running.agent_id = Some("mini-c-d2".into());
        state.mini_coder_directives.push(running);
        assert_eq!(
            mark_kill_requested(&mut state, "mini-c-d2").as_deref(),
            Some("mini-c-d2")
        );
        assert!(state.mini_coder_directives[1].kill_requested);
    }

    // -- P6: propagation + kill-chain + retry-lost ---------------------------

    /// Build a chain: root (AwaitingRetry) -> r1 (AwaitingRetry) -> r2 (Running leaf).
    /// Returns the state with the three directives.
    fn three_deep_chain() -> crate::backend::model::AgentLiveState {
        let mut state = empty_state();
        let mut root = directive("root", "coder-1");
        root.status = MiniCoderStatus::AwaitingRetry;
        root.retry_directive_id = Some("root-r1".into());
        let mut r1 = directive("root-r1", "coder-1");
        r1.status = MiniCoderStatus::AwaitingRetry;
        r1.attempt = 1;
        r1.parent_directive_id = Some("root".into());
        r1.retry_directive_id = Some("root-r2".into());
        let mut r2 = directive("root-r2", "coder-1");
        r2.status = MiniCoderStatus::Running;
        r2.attempt = 2;
        r2.parent_directive_id = Some("root".into());
        r2.agent_id = Some("mini-c-root-r2".into());
        state.mini_coder_directives.push(root);
        state.mini_coder_directives.push(r1);
        state.mini_coder_directives.push(r2);
        state
    }

    #[test]
    fn propagate_terminal_stamps_all_awaiting_retry_ancestors() {
        // A clean Done on the r2 leaf must propagate Done to root + r1 (both AwaitingRetry)
        // so the poll watching the ROOT id unblocks. First stamp the leaf terminal
        // (it is Running -> Done via apply_result), then propagate to ancestors.
        let mut state = three_deep_chain();
        let done = MiniCoderOutcome::done(MiniCoderResult {
            status: "done".into(),
            output: Some("clean".into()),
            ..Default::default()
        });
        transition_directive(&mut state, "root-r2", |d| {
            mini_coder::apply_result(d, done.clone())
        });
        propagate_terminal_to_ancestors(&mut state, "root-r2", &done);

        let by_id = |id: &str| {
            state
                .mini_coder_directives
                .iter()
                .find(|d| d.id == id)
                .unwrap()
        };
        assert_eq!(by_id("root-r2").status, MiniCoderStatus::Done);
        assert_eq!(by_id("root").status, MiniCoderStatus::Done, "root unblocked");
        assert_eq!(by_id("root-r1").status, MiniCoderStatus::Done, "r1 unblocked");
        assert!(by_id("root").result.is_some());
        assert!(by_id("root-r1").result.is_some());
    }

    #[test]
    fn propagate_escalated_leaf_stamps_ancestors_escalated() {
        let mut state = three_deep_chain();
        let escalated = MiniCoderOutcome::escalated(
            vec!["src/a.rs".into()],
            mini_coder::EscalationInfo {
                attempts: 3,
                findings: vec![],
            },
        );
        transition_directive(&mut state, "root-r2", |d| {
            mini_coder::apply_result(d, escalated.clone())
        });
        propagate_terminal_to_ancestors(&mut state, "root-r2", &escalated);
        let by_id = |id: &str| {
            state
                .mini_coder_directives
                .iter()
                .find(|d| d.id == id)
                .unwrap()
        };
        assert_eq!(by_id("root").status, MiniCoderStatus::Escalated);
        assert_eq!(by_id("root-r1").status, MiniCoderStatus::Escalated);
    }

    #[test]
    fn mark_kill_requested_aborts_the_whole_chain() {
        // Killing via the LIVE leaf attempt's agent id flags the live attempt; killing an
        // attempt in a chain reaches the live attempt (the one with the PTY). The chain's
        // AwaitingRetry predecessors are flagged too (belt-and-braces for a racing finalize).
        let mut state = three_deep_chain();
        // Kill via the leaf's agent id -> returns the leaf's (live) PTY.
        assert_eq!(
            mark_kill_requested(&mut state, "mini-c-root-r2").as_deref(),
            Some("mini-c-root-r2")
        );
        let by_id = |st: &crate::backend::model::AgentLiveState, id: &str| {
            st.mini_coder_directives
                .iter()
                .find(|d| d.id == id)
                .unwrap()
                .kill_requested
        };
        assert!(by_id(&state, "root-r2"), "live leaf flagged");
        assert!(by_id(&state, "root"), "root predecessor flagged");
        assert!(by_id(&state, "root-r1"), "r1 predecessor flagged");
    }

    /// WARNING 6 (KILL STALE AGENT_ID): stopping via an AwaitingRetry PREDECESSOR's stale
    /// agent id must return the LIVE retry's PTY (different agent id) — not the dead
    /// predecessor's — so `mini_coder_kill` kills the attempt that actually has a PTY.
    #[test]
    fn mark_kill_requested_via_stale_predecessor_returns_live_retry_pty() {
        let mut state = three_deep_chain();
        // Give the AwaitingRetry root a STALE agent id from a prior (dead) attempt.
        state.mini_coder_directives[0].agent_id = Some("mini-c-root-stale".into());
        // Human hits Stop on the root (the predecessor) via its stale id.
        let pty = mark_kill_requested(&mut state, "mini-c-root-stale");
        assert_eq!(
            pty.as_deref(),
            Some("mini-c-root-r2"),
            "must return the LIVE retry's PTY, not the dead predecessor's stale id"
        );
        // Whole chain still flagged.
        let by_id = |st: &crate::backend::model::AgentLiveState, id: &str| {
            st.mini_coder_directives
                .iter()
                .find(|d| d.id == id)
                .unwrap()
                .kill_requested
        };
        assert!(by_id(&state, "root"), "predecessor flagged");
        assert!(by_id(&state, "root-r2"), "live retry flagged");
    }

    #[test]
    fn sweep_pure_pieces_detect_and_fail_lost_retry() {
        // The startup sweep's two halves, exercised in-memory (the fn itself needs an
        // AppHandle): (1) detection via awaiting_retry_with_lost_child, (2) the direct
        // failed-stamp + propagation it applies. Build root(AwaitingRetry, retry=ghost).
        let mut state = empty_state();
        let mut root = directive("root", "coder-1");
        root.status = MiniCoderStatus::AwaitingRetry;
        root.retry_directive_id = Some("ghost".into()); // absent
        state.mini_coder_directives.push(root);

        let lost = mini_coder::awaiting_retry_with_lost_child(&state.mini_coder_directives);
        assert_eq!(lost, vec!["root".to_string()]);

        // Apply the sweep's stamp (same as sweep_orphaned_awaiting_retry's locked body).
        let outcome = MiniCoderOutcome::failed("retry lost (retry directive absent after restart)");
        for id in &lost {
            if let Some(d) = state.mini_coder_directives.iter_mut().find(|d| &d.id == id) {
                d.status = outcome.status;
                d.result = Some(outcome.clone());
            }
            propagate_terminal_to_ancestors(&mut state, id, &outcome);
        }
        let root = &state.mini_coder_directives[0];
        assert_eq!(root.status, MiniCoderStatus::Failed);
        assert!(root
            .result
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("retry lost"));
    }

    /// BLOCKER 1 (STRANDED ROOT): a retry that fails at LAUNCH must propagate `failed`
    /// to its AwaitingRetry root via the shared `stamp_terminal_and_propagate` (the exact
    /// body `fail_launching` now runs under the lock). Before the fix `fail_launching`
    /// only stamped the failed retry and the root sat AwaitingRetry forever.
    #[test]
    fn launch_failure_of_retry_propagates_failed_to_awaiting_retry_root() {
        let mut state = empty_state();
        // root(AwaitingRetry) -> r1(Launching, attempt 1). The retry is about to fail at
        // launch (no project / spawn error). agent_id is None (apply_launched never ran).
        let mut root = directive("root", "coder-1");
        root.status = MiniCoderStatus::AwaitingRetry;
        root.retry_directive_id = Some("root-r1".into());
        let mut r1 = directive("root-r1", "coder-1");
        r1.status = MiniCoderStatus::Launching;
        r1.attempt = 1;
        r1.parent_directive_id = Some("root".into());
        state.mini_coder_directives.push(root);
        state.mini_coder_directives.push(r1);

        // Apply the SHARED helper exactly as fail_launching does under the lock.
        let outcome = MiniCoderOutcome::failed("parent coder has no current project");
        stamp_terminal_and_propagate(&mut state, "root-r1", &outcome, None, |d| {
            mini_coder::apply_failed(d, "parent coder has no current project")
        });

        let by_id = |id: &str| {
            state
                .mini_coder_directives
                .iter()
                .find(|d| d.id == id)
                .unwrap()
        };
        assert_eq!(by_id("root-r1").status, MiniCoderStatus::Failed, "retry failed");
        assert_eq!(
            by_id("root").status,
            MiniCoderStatus::Failed,
            "BLOCKER 1: the AwaitingRetry root must be stamped failed so the poll unblocks"
        );
        assert!(by_id("root").result.is_some(), "root carries the propagated outcome");
    }

    /// BLOCKER 1 (second sweep rule): the sweep's body must catch an AwaitingRetry
    /// predecessor whose retry child is now TERMINAL (not absent) and re-propagate the
    /// CHILD's terminal outcome to it. Exercises `awaiting_retry_needing_terminal` plus
    /// the stamp the locked sweep body applies.
    #[test]
    fn sweep_catches_awaiting_retry_with_terminal_child() {
        let mut state = empty_state();
        let mut root = directive("root", "coder-1");
        root.status = MiniCoderStatus::AwaitingRetry;
        root.retry_directive_id = Some("root-r1".into());
        let mut r1 = directive("root-r1", "coder-1");
        r1.status = MiniCoderStatus::Failed; // terminal child, but root never stamped.
        r1.attempt = 1;
        r1.parent_directive_id = Some("root".into());
        r1.result = Some(MiniCoderOutcome::failed("mini spawn failed"));
        state.mini_coder_directives.push(root);
        state.mini_coder_directives.push(r1);

        let actions = mini_coder::awaiting_retry_needing_terminal(&state.mini_coder_directives);
        assert_eq!(
            actions,
            vec![(
                "root".to_string(),
                mini_coder::RetrySweepAction::PropagateChildTerminal { child_id: "root-r1".into() }
            )]
        );

        // Apply the sweep's locked body for the PropagateChildTerminal action.
        for (id, action) in &actions {
            let outcome = match action {
                mini_coder::RetrySweepAction::FailLost => {
                    MiniCoderOutcome::failed("retry lost")
                }
                mini_coder::RetrySweepAction::PropagateChildTerminal { child_id } => state
                    .mini_coder_directives
                    .iter()
                    .find(|d| d.id == *child_id)
                    .and_then(|c| c.result.clone())
                    .unwrap(),
            };
            if let Some(d) = state.mini_coder_directives.iter_mut().find(|d| &d.id == id) {
                d.status = outcome.status;
                d.result = Some(outcome.clone());
            }
            propagate_terminal_to_ancestors(&mut state, id, &outcome);
        }
        let root = state
            .mini_coder_directives
            .iter()
            .find(|d| d.id == "root")
            .unwrap();
        assert_eq!(root.status, MiniCoderStatus::Failed, "root re-stamped from child");
        assert!(root
            .result
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("mini spawn failed"));
    }

    #[test]
    fn finalize_gate_decision_dirty_builds_retry_via_pure_decision() {
        // The executor's verdict gate delegates to the PURE verdict_gate_decision; assert
        // the wiring end-state on a TRUSTED dirty Done: AwaitingRetryWith with a Pending
        // retry (attempt+1) whose feedback carries the High finding. (The executor applies
        // this decision under lock; the decision itself is what drives the state change.)
        let mut d = directive("root", "coder-1");
        d.status = MiniCoderStatus::Running;
        let outcome = MiniCoderOutcome::done(MiniCoderResult {
            status: "done".into(),
            files_touched: vec!["src/a.rs".into()],
            ..Default::default()
        });
        let findings = vec![mini_coder::EscalationFinding {
            file: "src/a.rs".into(),
            severity: "high".into(),
            source: "clippy".into(),
            title: "panics on empty input".into(),
            line: Some(7),
        }];
        let decision = mini_coder::verdict_gate_decision(
            &d,
            &outcome,
            true,
            findings,
            "root-r1",
            "root-r1.json",
            "2026-06-10T00:00:00Z",
        );
        match decision {
            mini_coder::GateDecision::AwaitingRetryWith { retry } => {
                assert_eq!(retry.attempt, 1);
                assert!(retry.task.contains("panics on empty input"));
            }
            other => panic!("expected AwaitingRetryWith, got {other:?}"),
        }
    }

    #[test]
    fn live_kill_override_turns_done_into_aborted_when_flagged() {
        let mut state = empty_state();
        let mut d = directive("d1", "coder-1");
        d.status = MiniCoderStatus::Running;
        d.kill_requested = true;
        state.mini_coder_directives.push(d);
        let done = MiniCoderOutcome::done(MiniCoderResult {
            status: "done".into(),
            ..Default::default()
        });
        let overridden = live_kill_override(&state, "d1", done);
        assert_eq!(overridden.status, MiniCoderStatus::AbortedByHuman);

        // Not flagged -> unchanged.
        state.mini_coder_directives[0].kill_requested = false;
        let done2 = MiniCoderOutcome::done(MiniCoderResult {
            status: "done".into(),
            ..Default::default()
        });
        let kept = live_kill_override(&state, "d1", done2);
        assert_eq!(kept.status, MiniCoderStatus::Done);
    }

    #[test]
    fn plan_result_file_sweep_keeps_live_drops_terminal_and_unknown() {
        // WARNING 5: the pure sweep plan keeps ONLY a live (non-terminal) directive's
        // result file in its scratch dir; a terminal directive's file is NOT kept (it
        // should have been deleted on finalize -> reclaim it), and a dir is keyed once.
        let scratch = "/proj/.aspis-mini";
        let mut live = directive("live1", "coder-1");
        live.status = MiniCoderStatus::Running;
        live.result_path = "live1.json".into();
        live.scratch_path = Some(scratch.to_string());

        let mut term = directive("term1", "coder-1");
        term.status = MiniCoderStatus::Done;
        term.result_path = "term1.json".into();
        term.scratch_path = Some(scratch.to_string());

        // A directive with no scratch path contributes nothing.
        let mut nodir = directive("nodir", "coder-1");
        nodir.status = MiniCoderStatus::Running;
        nodir.scratch_path = None;

        let plan = plan_result_file_sweep(&[live, term, nodir]);
        assert_eq!(plan.len(), 1, "one distinct scratch dir");
        let keep = plan.get(&PathBuf::from(scratch)).expect("dir present");
        assert!(keep.contains("live1.json"), "live result file kept");
        assert!(
            !keep.contains("term1.json"),
            "terminal result file NOT kept (reclaimed)"
        );
        // FIX 1: a live directive's in-flight `.raw` capture is also kept; a terminal
        // one's `.raw` is reclaimable.
        assert!(keep.contains("live1.json.raw"), "live raw capture kept");
        assert!(
            !keep.contains("term1.json.raw"),
            "terminal raw capture NOT kept (reclaimed)"
        );
    }

    #[test]
    fn sweep_orphaned_result_files_plan_deletes_only_orphan_json() {
        // WARNING 5 (fs behavior): given a real scratch dir with a live file, an
        // orphan (terminal) file, and a non-json file, the plan keeps the live file and
        // marks the orphan for deletion; the non-json is untouched by construction
        // (the fn only ever removes `*.json`). We exercise the plan + the same
        // delete/keep predicate the fn uses.
        let dir = std::env::temp_dir().join(format!("mc_p5_sweep_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("live.json"), "{}").unwrap();
        std::fs::write(dir.join("orphan.json"), "{}").unwrap();
        std::fs::write(dir.join("keep.txt"), "x").unwrap();
        // FIX 1: a live directive's in-flight `.raw` capture must SURVIVE; a stray
        // orphan `.raw` (left by a hard-killed mini) must be reclaimed.
        std::fs::write(dir.join("live.json.raw"), "x").unwrap();
        std::fs::write(dir.join("orphan.json.raw"), "x").unwrap();

        let mut live = directive("live", "coder-1");
        live.status = MiniCoderStatus::Running;
        live.result_path = "live.json".into();
        live.scratch_path = Some(dir.to_string_lossy().to_string());

        let plan = plan_result_file_sweep(&[live]);
        let keep = plan.get(&dir).cloned().unwrap_or_default();

        // Apply the exact same predicate the fn applies (now incl. `.raw`).
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            let is_json = ext.is_some_and(|e| e.eq_ignore_ascii_case("json"));
            let is_raw = ext.is_some_and(|e| e.eq_ignore_ascii_case("raw"));
            if (!is_json && !is_raw) || !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap()
                .to_string();
            if keep.contains(&name) {
                continue;
            }
            let _ = std::fs::remove_file(&path);
        }

        assert!(dir.join("live.json").exists(), "live result file survives");
        assert!(!dir.join("orphan.json").exists(), "orphan json deleted");
        assert!(dir.join("keep.txt").exists(), "non-json untouched");
        assert!(
            dir.join("live.json.raw").exists(),
            "live raw capture survives"
        );
        assert!(
            !dir.join("orphan.json.raw").exists(),
            "orphan raw capture deleted"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    fn test_session(agent_id: &str, status: &str) -> crate::backend::model::AgentSession {
        crate::backend::model::AgentSession {
            agent_id: agent_id.into(),
            role: "coder".into(),
            model: None,
            status: status.into(),
            client: None,
            message: None,
            current_project_id: None,
            current_task_id: None,
            current_file_path: None,
            first_seen_at: None,
            last_seen_at: None,
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
        }
    }

    // -- P4: prompt + per-kind command build ---------------------------------

    fn backend(
        kind: MiniCoderBackendKind,
        model: Option<&str>,
        command: Option<&str>,
    ) -> MiniCoderBackend {
        MiniCoderBackend {
            kind,
            model: model.map(|s| s.to_string()),
            command: command.map(|s| s.to_string()),
            base_url: None,
            max_concurrent: None,
        }
    }

    /// oMLX-P2 test helper: an omlx backend with a (normalized, loopback) base URL +
    /// model, as oMLX-P1 validation would produce.
    fn omlx_backend(model: &str, base_url: &str) -> MiniCoderBackend {
        MiniCoderBackend {
            kind: MiniCoderBackendKind::Omlx,
            model: Some(model.to_string()),
            command: None,
            base_url: Some(base_url.to_string()),
            max_concurrent: None,
        }
    }

    fn p4_directive(allow_oracle: bool) -> MiniCoderDirective {
        MiniCoderDirective {
            id: "d1".into(),
            parent_agent_id: "coder-1".into(),
            status: MiniCoderStatus::Running,
            task: "add a docstring to foo()".into(),
            files: vec!["src/a.rs".into(), "src/b.rs".into()],
            backend: None,
            allow_oracle,
            kill_requested: false,
            result_path: "d1.json".into(),
            agent_id: None,
            created_at: "2026-06-06T00:00:00Z".into(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: None,
            attempt: 0,
            parent_directive_id: None,
            retry_directive_id: None,
        }
    }

    #[test]
    fn read_prompt_file_rejects_symlink_escaping_the_root() {
        // WARNING 3: a `files` entry inside the project root that is a SYMLINK to a
        // file OUTSIDE the root must NOT be front-loaded (canonicalize-after-join).
        let base = std::env::temp_dir().join(format!("mc_symlink_{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "TOP SECRET — must not be read").unwrap();

        // A plain in-root file IS read (control: confinement does not over-reject).
        std::fs::write(root.join("ok.txt"), "in-root contents").unwrap();
        assert_eq!(
            read_prompt_file(&root, "ok.txt").as_deref(),
            Some("in-root contents"),
            "a normal in-root file must still be read"
        );

        // Create a symlink inside the root pointing at the outside secret.
        let link = root.join("link.txt");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&secret, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&secret, &link).is_ok();
        #[cfg(not(any(unix, windows)))]
        let made = false;

        if made {
            // The symlink resolves outside the canonical root -> NOT read.
            assert_eq!(
                read_prompt_file(&root, "link.txt"),
                None,
                "a symlink escaping the root must not be front-loaded"
            );
        }
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn build_mini_prompt_has_constraints_file_scope_and_schema() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let codex = backend(MiniCoderBackendKind::Codex, None, None);
        let prompt = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target);

        // The task + the explicit file scope are embedded.
        assert!(prompt.contains("add a docstring to foo()"), "task missing");
        assert!(prompt.contains("src/a.rs"), "file scope missing a.rs");
        assert!(prompt.contains("src/b.rs"), "file scope missing b.rs");
        // Anti-destructive constraints present.
        assert!(prompt.contains("rm -rf"), "anti-destructive block missing");
        assert!(
            prompt.contains("force-push"),
            "anti-destructive block missing"
        );
        assert!(
            prompt.contains("outside the FILE SCOPE"),
            "scope constraint missing"
        );
        assert!(prompt.contains("visual_check"), "visual-check handoff missing");
        // Result schema present.
        assert!(
            prompt.contains("needs_clarification"),
            "schema status missing"
        );
        assert!(prompt.contains("filesTouched"), "schema field missing");
        // codex writes the file itself; the exact resultPath is named.
        assert!(
            prompt.contains(&result_target.to_string_lossy().to_string()),
            "resultPath missing"
        );
        assert!(
            prompt.contains("WRITE this JSON object to the file"),
            "codex write instruction missing"
        );
    }

    #[test]
    fn build_mini_prompt_never_advertises_oracle_minor9() {
        // MINOR 9: a mini gets NO MCP/oracle grant for ANY kind, even codex with
        // allow_oracle=true. The prompt must never advertise `oracle_context`; it must
        // instead tell the mini it has NO tools and works from front-loaded context.
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");

        let codex = backend(MiniCoderBackendKind::Codex, None, None);
        let p = build_mini_prompt(&codex, &p4_directive(true), &root, &result_target);
        assert!(
            !p.contains("oracle_context"),
            "mini must never advertise oracle (codex+allow)"
        );
        assert!(
            p.contains("You have NO external tools"),
            "must tell the mini it has no tools"
        );
        // codex still WRITES the result file itself.
        assert!(
            p.contains("WRITE this JSON object to the file"),
            "codex write instruction missing"
        );

        let p = build_mini_prompt(&codex, &p4_directive(false), &root, &result_target);
        assert!(
            !p.contains("oracle_context"),
            "no oracle when allow_oracle=false"
        );

        // ollama is text-only: no tool, and it must OUTPUT (not write a file).
        let ollama = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
        let p = build_mini_prompt(&ollama, &p4_directive(true), &root, &result_target);
        assert!(
            !p.contains("oracle_context"),
            "ollama (text-only) must never advertise oracle"
        );
        assert!(
            p.contains("OUTPUT this JSON object to stdout"),
            "ollama must output, not write"
        );
    }

    #[cfg(windows)]
    fn argv_strings(cmd: &portable_pty::CommandBuilder) -> Vec<String> {
        cmd.get_argv()
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    #[cfg(windows)]
    #[test]
    fn build_command_applefm_windows_returns_clean_macos_only_error() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("fake-prompt.txt");
        let b = backend(MiniCoderBackendKind::AppleFm, Some("apple-model"), None);
        let err =
            build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None)
                .unwrap_err();
        assert_eq!(err, "Apple on-device requires macOS 27+.");
    }

    #[cfg(windows)]
    #[test]
    fn build_command_codex_uses_codex_exec_and_pipes_prompt_via_stdin() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("fake-prompt.txt");
        let b = backend(MiniCoderBackendKind::Codex, Some("gpt-5-codex"), None);
        // No mcp_roots -> no oracle grant flags.
        let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None).unwrap();
        let argv = argv_strings(&cmd);
        assert_eq!(argv[0], "powershell.exe");
        let script = argv.last().unwrap();
        // codex exec with the model, prompt piped over stdin.
        assert!(script.contains("'exec'"), "codex exec missing: {script}");
        assert!(
            script.contains("'-m', 'gpt-5-codex'"),
            "model flag missing: {script}"
        );
        assert!(
            script.contains("$prompt | & codex @codexArgs"),
            "stdin pipe missing: {script}"
        );
        // NO MCP -c flags when oracle not granted.
        assert!(
            !script.contains("mcp_servers.aspis-management"),
            "oracle grant leaked: {script}"
        );
        // B1: the prompt is read from the file then DELETED; never Write-Host'd.
        assert!(
            script.contains("Get-Content -Raw -LiteralPath $promptFile"),
            "prompt-file read missing"
        );
        assert!(
            !script.contains("Write-Host $prompt"),
            "prompt must never be echoed to the PTY"
        );
        // FIX 1: cleanup of the source-bearing prompt dir + raw capture lives in a
        // `finally` so it ALWAYS runs (even if Get-Content / the backend errors).
        assert!(
            script.contains("finally {"),
            "finally cleanup block missing: {script}"
        );
        assert!(
            script.contains("Remove-Item -LiteralPath $promptDir -Recurse -Force"),
            "prompt dir cleanup must be in finally: {script}"
        );
        // F5: codex writes NO `.raw` file (it does not use the stdout wrapper), so the
        // raw-file removal is guarded by Test-Path — it never runs Remove-Item on a
        // non-existent file.
        assert!(
            script.contains("if (Test-Path -LiteralPath $rawFile) { Remove-Item -LiteralPath $rawFile -Force"),
            "raw capture removal must be Test-Path-guarded: {script}"
        );
        // F4: a non-keyed backend (codex here) carries NO oMLX key-cleanup collateral.
        assert!(
            !script.contains("$env:OMLX_KEY_FILE"),
            "non-keyed codex script must not carry the oMLX key cleanup: {script}"
        );
    }

    // ---- oMLX-P2 (Windows launch script) -----------------------------------

    #[cfg(windows)]
    #[test]
    fn build_command_omlx_windows_posts_chat_completions_via_rest() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("p").join("fake-prompt.txt");
        let b = omlx_backend("qwen2.5-coder", "http://localhost:8000/v1");
        let cmd =
            build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None).unwrap();
        let argv = argv_strings(&cmd);
        assert_eq!(argv[0], "powershell.exe");
        let script = argv.last().unwrap();

        // POSTs via Invoke-RestMethod to <base>/chat/completions (no double slash).
        assert!(
            script.contains("Invoke-RestMethod -Method Post"),
            "must POST via Invoke-RestMethod: {script}"
        );
        // F3: a -TimeoutSec cap (derived from DEFAULT_WALL_CLOCK_CAP_SECS minus a margin)
        // makes a stalled server fail fast instead of holding the PTY to the wall-clock
        // kill. On timeout Invoke-RestMethod throws -> the try/catch yields the clean
        // failed fallback.
        let expected_timeout =
            super::mini_coder::DEFAULT_WALL_CLOCK_CAP_SECS - OMLX_HTTP_TIMEOUT_MARGIN_SECS;
        assert!(
            script.contains(&format!("-TimeoutSec {expected_timeout}")),
            "Invoke-RestMethod must carry a -TimeoutSec derived from the wall-clock cap: {script}"
        );
        assert!(
            script.contains("'http://localhost:8000/v1/chat/completions'"),
            "must target <base>/chat/completions, quoted, no double slash: {script}"
        );
        assert!(
            !script.contains("/v1//chat") && !script.contains(".0/chat//"),
            "no double slash in the URI: {script}"
        );
        // Body built by the ConvertTo-Json ENCODER (never string-concatenated).
        assert!(
            script.contains("| ConvertTo-Json -Depth 6 -Compress"),
            "body must be JSON-encoded by ConvertTo-Json: {script}"
        );
        assert!(
            script.contains("content = $prompt"),
            "prompt must ride as a VALUE encoded by ConvertTo-Json: {script}"
        );
        // INJECTION-SAFETY: the prompt is NEVER string-concatenated into the JSON body.
        assert!(
            !script.contains("\"content\":\"' +") && !script.contains("'+ $prompt") && !script.contains("+ $prompt +"),
            "prompt must NOT be concatenated into the JSON: {script}"
        );
        assert!(
            script.contains("temperature = 0.1") && script.contains("stream = $false"),
            "OpenAI envelope fields missing: {script}"
        );
        // Extracts the model's content and writes it to stdout for the wrapper.
        assert!(
            script.contains("$resp.choices[0].message.content"),
            "must extract choices[0].message.content: {script}"
        );
        assert!(
            script.contains("Write-Output $content"),
            "content must be written to stdout: {script}"
        );
        // FAILURE = SILENCE: the request is wrapped in try/catch so any HTTP/parse error
        // writes nothing -> the wrapper writes the failed fallback.
        assert!(
            script.contains("try {") && script.contains("} catch { }"),
            "request must be wrapped in try/catch: {script}"
        );
        // Still feeds the EXISTING result-file write wrapper (balanced walk + write).
        assert!(
            script.contains("> $rawFile 2>$null"),
            "must feed the shared stdout->result wrapper: {script}"
        );
        assert!(
            script.contains("[System.IO.File]::WriteAllText"),
            "result-file write (the wrapper) must run: {script}"
        );
        assert!(
            script.contains("\\\"status\\\":\\\"failed\\\"")
                || script.contains("status\":\"failed"),
            "the wrapper's failed fallback must be present: {script}"
        );
        // No model on argv-visible token issues; model is OUR validated bare tag, quoted.
        assert!(
            script.contains("model = 'qwen2.5-coder'"),
            "model must be the configured tag: {script}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn build_command_omlx_windows_no_key_emits_no_auth_header_and_no_key_env() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("p").join("fake-prompt.txt");
        let b = omlx_backend("m", "http://127.0.0.1:8000");
        // No key file passed (the default; omlx_api_key returns None today).
        let cmd =
            build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None).unwrap();
        let argv = argv_strings(&cmd);
        let script = argv.last().unwrap();
        // No auth header construction anywhere (no key configured).
        assert!(
            !script.contains("Authorization"),
            "no auth header without a key: {script}"
        );
        // F4: a non-keyed spawn carries NO key-env collateral ANYWHERE — neither the
        // request body nor the shared `finally` reference `$env:OMLX_KEY_FILE` (the
        // key-dir cleanup line is emitted only when a key is configured for this spawn).
        assert!(
            !script.contains("$env:OMLX_KEY_FILE"),
            "non-keyed script must not reference the key env anywhere: {script}"
        );
        // The key file env must NOT be set on the command when there is no key.
        assert!(
            cmd.get_env("OMLX_KEY_FILE").is_none(),
            "OMLX_KEY_FILE env must be absent without a key"
        );
        // F5: the raw-file removal is guarded by Test-Path (codex writes no raw file; here
        // the wrapper does, but the guard is uniform and harmless).
        assert!(
            script.contains("if (Test-Path -LiteralPath $rawFile) { Remove-Item -LiteralPath $rawFile"),
            "raw-file removal must be Test-Path-guarded: {script}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn build_command_omlx_windows_with_key_rides_env_file_not_argv_and_cleans_up() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("p").join("fake-prompt.txt");
        let key_file = root.join("kdir").join("omlx-key.txt");
        let b = omlx_backend("m", "http://localhost:8000/v1");
        let cmd = build_mini_command_impl(
            &b,
            &root,
            &result_target,
            &prompt_file,
            Some(&key_file),
            None,
        )
        .unwrap();
        let argv = argv_strings(&cmd);
        let script = argv.last().unwrap();

        // The token is read from the env-passed FILE and sent as a Bearer header.
        assert!(
            script.contains("$env:OMLX_KEY_FILE") && script.contains("Get-Content -Raw -LiteralPath $env:OMLX_KEY_FILE"),
            "key must be read from the env-passed file: {script}"
        );
        assert!(
            script.contains("'Bearer ' + $omlxKey"),
            "token must ride an Authorization: Bearer header: {script}"
        );
        // max-recall FIX 8: the key variable is zeroed right after the header is set so the
        // token does not linger in PS scope for the rest of the script.
        assert!(
            script.contains("$omlxKey = $null"),
            "key variable must be zeroed after the header is set: {script}"
        );
        // The KEY FILE PATH rides on env, NEVER on argv. No argv entry contains the path.
        let key_str = key_file.to_string_lossy().to_string();
        assert!(
            !argv.iter().any(|a| a.contains(&key_str)),
            "key file path must NOT appear on argv: {argv:?}"
        );
        // The env var IS set on the command (path only — the token itself stays in the
        // file, never in env/argv).
        let env_val = cmd
            .get_env("OMLX_KEY_FILE")
            .map(|v| v.to_string_lossy().to_string());
        assert_eq!(
            env_val.as_deref(),
            Some(key_str.as_str()),
            "OMLX_KEY_FILE env must carry the key file PATH"
        );
        // The finally removes the key file's restricted dir on every exit path.
        assert!(
            script.contains("if ($env:OMLX_KEY_FILE) { Remove-Item -LiteralPath ([System.IO.Path]::GetDirectoryName($env:OMLX_KEY_FILE)) -Recurse -Force"),
            "finally must remove the key dir on every exit: {script}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn build_command_windows_cleanup_always_runs_in_finally() {
        // FIX 1 (source-content leak): the read of the prompt happens INSIDE the try,
        // and the prompt dir + raw file are removed in the finally — so a failing
        // Get-Content can no longer skip cleanup and leak the front-loaded source.
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("p").join("fake-prompt.txt");
        let b = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
        let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None).unwrap();
        let script = argv_strings(&cmd).pop().unwrap();
        // The prompt read is inside the try (the try opens before Get-Content).
        let try_idx = script.find("try {").expect("try block");
        let read_idx = script
            .find("Get-Content -Raw -LiteralPath $promptFile")
            .expect("read");
        let finally_idx = script.find("finally {").expect("finally block");
        assert!(
            try_idx < read_idx,
            "Get-Content must be inside the try: {script}"
        );
        assert!(
            read_idx < finally_idx,
            "finally must come after the body: {script}"
        );
        // Both the prompt dir and the raw file are torn down in the finally.
        let finally_tail = &script[finally_idx..];
        assert!(
            finally_tail.contains("$promptDir"),
            "promptDir not cleaned in finally: {script}"
        );
        assert!(
            finally_tail.contains("$rawFile"),
            "rawFile not cleaned in finally: {script}"
        );
    }

    // FIX 1 (behavioral, Windows): run the REAL script with a backend that ERRORS
    // and prove the source-bearing prompt dir + the .raw capture are gone afterward.
    #[cfg(windows)]
    #[test]
    fn windows_finally_cleans_files_even_when_backend_errors() {
        use std::process::Command;
        let scratch = std::env::temp_dir().join(format!("mc_fix1win_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let prompt_dir = scratch.join("prompt");
        std::fs::create_dir_all(&prompt_dir).unwrap();
        let prompt_file = prompt_dir.join("p.txt");
        std::fs::write(&prompt_file, "secret source code\n").unwrap();
        let result_target = scratch.join("d1.json");
        // An api command that EXITS NON-ZERO / errors (a non-existent executable). The
        // body throws under ErrorActionPreference=Stop, but the finally must still run.
        let b = backend(
            MiniCoderBackendKind::Api,
            None,
            Some("this_executable_does_not_exist_xyz"),
        );
        let cmd =
            build_mini_command_impl(&b, &scratch, &result_target, &prompt_file, None, None).unwrap();
        let script = argv_strings(&cmd).pop().unwrap();
        let _ = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .current_dir(&scratch)
            .status()
            .expect("run script");
        assert!(
            !prompt_dir.exists(),
            "prompt dir must be removed by finally even on error"
        );
        let raw = scratch.join("d1.json.raw");
        assert!(
            !raw.exists(),
            "raw capture must be removed by finally even on error"
        );
        std::fs::remove_dir_all(&scratch).ok();
    }

    #[cfg(windows)]
    #[test]
    fn build_command_codex_never_adds_mcp_config_flags_even_with_roots() {
        // MINOR 9: a mini gets NO MCP grant. Even when McpRoots are supplied (the
        // plumbing is kept for a future read-only oracle scope), build_mini_command_impl
        // must NOT emit any `-c mcp_servers...` flags — the mini works from front-loaded
        // context only, never the full mutation-capable aspis-management server.
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("fake-prompt.txt");
        let b = backend(MiniCoderBackendKind::Codex, None, None);
        let roots = McpRoots {
            management_root: PathBuf::from("C:/mgmt"),
            projects_dir: PathBuf::from("C:/mgmt/projects"),
        };
        let cmd =
            build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, Some(&roots)).unwrap();
        let script = argv_strings(&cmd).pop().unwrap();
        assert!(
            !script.contains("mcp_servers"),
            "mini must never get an MCP grant: {script}"
        );
        assert!(
            !script.contains("'-c'"),
            "mini must never get a -c flag: {script}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn build_mini_command_drops_mcp_roots_entirely() {
        // MINOR 9 at the public boundary: even the public build_mini_command, given
        // McpRoots, must produce a script with no MCP grant (mcp_roots is inert).
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let b = backend(MiniCoderBackendKind::Codex, None, None);
        let directive = p4_directive(true); // allow_oracle = true
        let prompt = build_mini_prompt(&b, &directive, &root, &result_target);
        let roots = McpRoots {
            management_root: PathBuf::from("C:/mgmt"),
            projects_dir: PathBuf::from("C:/mgmt/projects"),
        };
        let build =
            build_mini_command(&b, &root, &result_target, &prompt, Some(&roots)).unwrap();
        let script = argv_strings(&build.command).pop().unwrap();
        assert!(
            !script.contains("mcp_servers"),
            "mcp_roots must be inert: {script}"
        );
        super::super::projects::remove_restricted_temp_file(&build.prompt_file.unwrap());
    }

    #[test]
    fn remove_mini_temp_files_removes_both_prompt_and_key_files() {
        // max-recall FIX 10: a spawn-failure cleanup must remove BOTH restricted temp
        // files (prompt AND the oMLX key file), each in its OWN 0600 dir. Today
        // `omlx_api_key` returns None so production never creates the key file, but the
        // cleanup is wired leak-free for when the key config field lands. We create two
        // real restricted temp files (mirroring what build_mini_command writes for the
        // prompt and the key) and assert the cleanup removes both files AND their dirs.
        let prompt_file = super::super::projects::write_restricted_prompt_file("prompt body")
            .expect("prompt file created");
        let key_file = super::super::projects::write_restricted_prompt_file("secret-token")
            .expect("key file created");
        // Distinct restricted directories (each call makes a fresh per-launch *.d dir).
        let prompt_dir = prompt_file.parent().unwrap().to_path_buf();
        let key_dir = key_file.parent().unwrap().to_path_buf();
        assert_ne!(prompt_dir, key_dir, "the two temp files live in distinct dirs");
        assert!(prompt_file.exists() && key_file.exists());

        remove_mini_temp_files(Some(&prompt_file), Some(&key_file));

        assert!(!prompt_file.exists(), "prompt file must be removed");
        assert!(!key_file.exists(), "key file must be removed (no leak)");
        assert!(!prompt_dir.exists(), "prompt dir must be removed");
        assert!(!key_dir.exists(), "key dir must be removed (no leak)");
    }

    #[cfg(windows)]
    #[test]
    fn build_command_ollama_runs_model_pipes_stdin_and_wraps_stdout() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("fake-prompt.txt");
        let b = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
        let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None).unwrap();
        let script = argv_strings(&cmd).pop().unwrap();
        assert!(
            script.contains("ollama run 'qwen2.5-coder'"),
            "ollama run missing: {script}"
        );
        assert!(
            script.contains("$prompt | & ollama"),
            "stdin pipe missing: {script}"
        );
        // The stdout->result-file wrapper writes the normalized result.
        assert!(
            script.contains("ConvertFrom-Json"),
            "stdout wrapper missing: {script}"
        );
        assert!(
            script.contains("WriteAllText"),
            "result-file write missing: {script}"
        );
        // WARNING 7: stdout is redirected to a temp file, read bounded.
        assert!(
            script.contains("$rawFile"),
            "raw stdout temp file missing: {script}"
        );
        assert!(
            script.contains("StreamReader"),
            "bounded raw read missing: {script}"
        );
        // BLOCKER 2: balanced-brace walk (not first-{/last-}).
        assert!(
            script.contains("$depth"),
            "balanced-brace walk missing: {script}"
        );
        // No MCP/oracle for text-only ollama.
        assert!(
            !script.contains("mcp_servers"),
            "ollama must not get MCP: {script}"
        );
        // WARNING 8: the prompt-file parent restricted dir is removed too.
        assert!(
            script.contains("Remove-Item -LiteralPath $promptDir"),
            "parent dir cleanup missing: {script}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn build_command_api_runs_configured_command_and_keeps_key_off_argv() {
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("fake-prompt.txt");
        // The user's CLI command. Any API key must come from the CLI's OWN env, not
        // from us — we never inject a key, so it can't be on argv.
        let b = backend(MiniCoderBackendKind::Api, None, Some("mycli chat --json"));
        let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None).unwrap();
        let argv = argv_strings(&cmd);
        let script = argv.last().unwrap();
        // BLOCKER 1 / WARNING 5: the multi-word command is piped to WITHOUT the `&`
        // call operator, so PowerShell tokenizes `mycli chat --json` itself (running
        // `mycli` with args `chat --json`). `& {command}` would treat the whole string
        // as a single executable name and fail.
        assert!(
            script.contains("$prompt | mycli chat --json"),
            "command must tokenize natively (no &): {script}"
        );
        assert!(
            !script.contains("$prompt | & mycli"),
            "must NOT use the & call operator on a command line: {script}"
        );
        assert!(
            script.contains("WriteAllText"),
            "stdout wrapper missing: {script}"
        );
        // B1: no secret token anywhere on argv (we never place one). The whole argv
        // joined must not contain an env-style key marker we'd inject.
        let joined = argv.join(" ");
        assert!(
            !joined.contains("API_KEY="),
            "a key must never be put on argv: {joined}"
        );
        assert!(
            !joined.contains("Authorization"),
            "no auth header on argv: {joined}"
        );
    }

    // BLOCKER (macOS trap): the EXIT trap must use DOUBLE-QUOTED shell variables, not
    // the raw single-quoted paths, so a path containing whitespace (common on macOS,
    // e.g. `/Users/the owner/My Project/`) does not terminate the trap's own single-quoted
    // delimiter and turn the trap into a syntax error (which would leave it unarmed and
    // leak the source-bearing prompt dir + `.raw` capture). This targets the pure,
    // platform-agnostic `build_macos_trap_preamble`, so it runs on the Windows dev host.
    #[test]
    fn macos_trap_preamble_uses_quoted_vars_and_survives_spaces() {
        // Mirror sh_single_quote_local: wrap in single quotes, escape embedded quotes.
        fn q(v: &str) -> String {
            format!("'{}'", v.replace('\'', "'\\''"))
        }
        let prompt_dir = q("/Users/the owner/My Project/.aspis-mini-xyz");
        let raw_path = q("/Users/the owner/My Project/scratch/d1.json.raw");
        // No key configured here (the common case): key dir is None.
        let preamble = build_macos_trap_preamble(&prompt_dir, &raw_path, None);

        // The paths are assigned to shell variables first.
        assert!(
            preamble.contains(&format!("_MINI_PROMPT_DIR={prompt_dir}")),
            "prompt dir must be assigned to a var with the quoted RHS: {preamble}"
        );
        assert!(
            preamble.contains(&format!("_MINI_RAW_FILE={raw_path}")),
            "raw path must always be assigned to a var with the quoted RHS: {preamble}"
        );
        // No-key case still assigns an empty _MINI_KEY_DIR so the trap body is fixed.
        assert!(
            preamble.contains("_MINI_KEY_DIR=''\n"),
            "no-key case must assign an empty key dir: {preamble}"
        );

        // The trap body references DOUBLE-QUOTED variables, NOT the raw quoted paths. The
        // key-dir removal is GUARDED on a non-empty value (max-recall FIX 9) so the no-key
        // case never runs `rm -rf ""`; the prompt-dir/raw-file removal is unconditional.
        assert!(
            preamble.contains(
                "trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true' EXIT"
            ),
            "trap must reference double-quoted vars and guard the key-dir removal: {preamble}"
        );

        // The trap is armed BEFORE `set -e` (so it fires even on a set -e abort).
        let trap_idx = preamble.find("trap '").expect("trap present");
        let set_e_idx = preamble.find("\nset -e").expect("set -e present");
        assert!(trap_idx < set_e_idx, "trap must precede set -e: {preamble}");

        // The space-containing path must NOT appear literally inside the trap body —
        // only the variable expansion does. Isolate the trap line and check it.
        let trap_line_start = trap_idx;
        let trap_line_end = preamble[trap_line_start..]
            .find('\n')
            .map(|o| trap_line_start + o)
            .unwrap_or(preamble.len());
        let trap_line = &preamble[trap_line_start..trap_line_end];
        assert!(
            !trap_line.contains("My Project"),
            "the literal (space-containing) path must not appear in the trap body: {trap_line}"
        );
        assert!(
            !trap_line.contains(prompt_dir.as_str()),
            "the raw single-quoted prompt dir must not be embedded in the trap: {trap_line}"
        );
        assert!(
            !trap_line.contains(raw_path.as_str()),
            "the raw single-quoted raw path must not be embedded in the trap: {trap_line}"
        );
    }

    // ---- oMLX-P2 (macOS launch script — platform-agnostic source-text) ------
    // These target the PURE `build_omlx_run_macos` / `build_macos_trap_preamble`, so
    // they run on the Windows dev host (the macOS cargo target cannot build here).

    #[test]
    fn omlx_macos_run_posts_via_python_urllib_json_dumps_env_only() {
        // prompt_path arrives sh-quoted (as the macOS arm passes it).
        let prompt_q = "'/tmp/aspis-agent-prompt-abc.d/p.txt'";
        let run = build_omlx_run_macos("http://localhost:8000/v1", "qwen2.5-coder", prompt_q, false);

        // stdlib python3 + urllib, NO curl/jq.
        assert!(run.contains("python3 - <<'OMLXEOF'"), "must use a python3 heredoc: {run}");
        assert!(run.contains("import urllib.request"), "must use stdlib urllib: {run}");
        assert!(!run.contains("curl") && !run.contains("jq "), "must not shell out to curl/jq: {run}");
        // Body via json.dumps ENCODER (injection-safe), prompt as a VALUE.
        assert!(run.contains("json.dumps("), "body must be json.dumps-encoded: {run}");
        assert!(
            run.contains("'content': prompt"),
            "prompt must ride as a json.dumps VALUE: {run}"
        );
        // INJECTION-SAFETY: prompt is NOT string-concatenated into JSON.
        assert!(
            !run.contains("'\"content\":\"' +") && !run.contains("+ prompt +"),
            "prompt must NOT be concatenated into the JSON body: {run}"
        );
        assert!(
            run.contains("'temperature': 0.1") && run.contains("'stream': False"),
            "OpenAI envelope fields missing: {run}"
        );
        // base URL + prompt path ride via ENV (never argv).
        assert!(
            run.contains("OMLX_URL='http://localhost:8000/v1/chat/completions'"),
            "base URL must be exported via env, /chat/completions appended, no double slash: {run}"
        );
        assert!(
            run.contains("MINI_PROMPT_FILE='/tmp/aspis-agent-prompt-abc.d/p.txt'")
                && run.contains("os.environ['MINI_PROMPT_FILE']"),
            "prompt path must ride env MINI_PROMPT_FILE, read in python: {run}"
        );
        assert!(
            run.contains("urllib.request.Request(os.environ['OMLX_URL']")
                && run.contains("method='POST'"),
            "must POST to the env-passed URL: {run}"
        );
        // F2: the HTTP timeout is NOT hardcoded — it rides the OMLX_TIMEOUT env (derived
        // from DEFAULT_WALL_CLOCK_CAP_SECS minus a margin) and python reads it with a
        // matching default, so the two can never silently diverge.
        let expected_timeout =
            super::mini_coder::DEFAULT_WALL_CLOCK_CAP_SECS - OMLX_HTTP_TIMEOUT_MARGIN_SECS;
        assert!(
            run.contains(&format!("OMLX_TIMEOUT={expected_timeout}\nexport OMLX_TIMEOUT")),
            "HTTP timeout must be exported via env, derived from the wall-clock cap: {run}"
        );
        assert!(
            run.contains(&format!(
                "timeout = int(os.environ.get('OMLX_TIMEOUT', '{expected_timeout}'))"
            )) && run.contains("urlopen(req, timeout=timeout)"),
            "python must read the env timeout (not a hardcoded 600): {run}"
        );
        assert!(
            !run.contains("timeout=600"),
            "the hardcoded urlopen timeout must be gone: {run}"
        );
        // extracts choices[0].message.content, prints to stdout for the wrapper.
        assert!(
            run.contains("data['choices'][0]['message']['content']"),
            "must extract choices[0].message.content: {run}"
        );
        assert!(run.contains("sys.stdout.write(content)"), "content to stdout: {run}");
        // FAILURE = SILENCE: any exception prints nothing (the outer try wraps the whole
        // request; its handler is a bare `pass`, so a non-2xx HTTPError / refused
        // connection / missing field writes no stdout and the wrapper emits the failed
        // fallback). Check the handler + its bare `pass` body independently of exact
        // whitespace.
        assert!(
            run.contains("except Exception:"),
            "the request must be wrapped in a catch-all except: {run}"
        );
        assert!(
            run.trim_end().ends_with("pass\nOMLXEOF") || run.contains("\n    pass\n"),
            "the catch-all handler must be a bare pass (no stdout on error): {run}"
        );
        // No-key case: key env cleared, no Authorization unless a key path is present.
        assert!(run.contains("unset OMLX_KEY_FILE"), "no-key case must clear the key env: {run}");
    }

    #[test]
    fn omlx_macos_run_with_key_reads_env_file_and_sends_bearer() {
        let prompt_q = "'/tmp/p.d/p.txt'";
        let run = build_omlx_run_macos("http://127.0.0.1:8000", "m", prompt_q, true);
        // The key path rides env; python reads the FILE and sends a Bearer header.
        assert!(run.contains("export OMLX_KEY_FILE"), "key env must be exported when keyed: {run}");
        assert!(
            run.contains("key_path = os.environ.get('OMLX_KEY_FILE')")
                && run.contains("with open(key_path"),
            "token must be read from the env-passed key file: {run}"
        );
        assert!(
            run.contains("req.add_header('Authorization', 'Bearer ' + token)"),
            "token must ride an Authorization: Bearer header: {run}"
        );
        // The token VALUE never appears literally — only the file is read.
        assert!(!run.contains("Bearer sk-"), "no literal token in the script: {run}");
    }

    #[test]
    fn apple_fm_macos_run_uses_fixed_fm_respond_and_prompt_pipe_only() {
        let run = build_apple_fm_run_macos(
            "cat '/tmp/aspis prompt.d/p.txt'",
            "/usr/bin/fm",
            Some("apple-default"),
        );
        assert_eq!(
            run,
            "cat '/tmp/aspis prompt.d/p.txt' | '/usr/bin/fm' respond --model 'apple-default'"
        );
        assert!(!run.contains("TOP_SECRET_PROMPT"));
    }

    #[test]
    fn omlx_macos_trap_cleans_key_dir_when_keyed() {
        fn q(v: &str) -> String {
            format!("'{}'", v.replace('\'', "'\\''"))
        }
        let prompt_dir = q("/tmp/aspis-agent-prompt-abc.d");
        let raw = q("/tmp/scratch/d1.json.raw");
        let key_dir = q("/tmp/aspis-agent-prompt-key.d");
        let preamble = build_macos_trap_preamble(&prompt_dir, &raw, Some(&key_dir));
        // The key dir is assigned and removed by the trap (double-quoted var) on EXIT.
        assert!(
            preamble.contains(&format!("_MINI_KEY_DIR={key_dir}")),
            "key dir must be assigned to a var: {preamble}"
        );
        assert!(
            preamble.contains(
                "trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true' EXIT"
            ),
            "trap must remove the key dir on every exit (guarded on non-empty): {preamble}"
        );
        // The literal key path must NOT appear inside the trap body (only the var does).
        let trap_idx = preamble.find("trap '").unwrap();
        let trap_end = preamble[trap_idx..].find('\n').map(|o| trap_idx + o).unwrap();
        assert!(
            !preamble[trap_idx..trap_end].contains("aspis-agent-prompt-key.d"),
            "literal key path must not be embedded in the trap body"
        );
    }

    #[cfg(windows)]
    #[test]
    fn build_command_full_writes_prompt_file_off_argv() {
        // The public build_mini_command writes the prompt to a restricted temp file
        // and the prompt text NEVER appears on argv (only the file path + script do).
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let b = backend(MiniCoderBackendKind::Codex, None, None);
        let directive = p4_directive(false);
        let prompt = build_mini_prompt(&b, &directive, &root, &result_target);
        let build =
            build_mini_command(&b, &root, &result_target, &prompt, None).unwrap();
        let prompt_file = build.prompt_file.expect("a prompt file is created");
        let joined = argv_strings(&build.command).join(" ");
        // The full prompt body (the task text) must NOT be on argv.
        assert!(
            !joined.contains("add a docstring to foo()"),
            "prompt body leaked onto argv"
        );
        // The script references the prompt FILE, not the prompt content.
        assert!(
            joined.contains(&prompt_file.to_string_lossy().to_string()),
            "prompt file path missing"
        );
        super::super::projects::remove_restricted_temp_file(&prompt_file);
    }

    // BLOCKER 2 (behavioral, Windows): the REAL wrapper PowerShell, given a model
    // output where prose contains `}` AND the JSON output value itself contains `{`/`}`
    // and a trailing `}`, must extract the CORRECT `done` object — not be tricked by a
    // first-`{`/last-`}` slice into a `failed`. Runs the generated wrapper for real.
    #[cfg(windows)]
    #[test]
    fn windows_wrapper_balanced_walk_extracts_done_with_braces_in_output() {
        use std::process::Command;
        let scratch = std::env::temp_dir().join(format!("mc_b2win_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let result_target = scratch.join("d1.json");
        let result_path = ps_single_quote(&result_target.to_string_lossy());
        let raw_path = ps_single_quote(&format!("{}.raw", result_target.to_string_lossy()));

        // The hostile model output: leading prose with a stray `}`, then the REAL
        // result object whose `output` value embeds `foo() {bar}`, then trailing prose
        // with another `}`. first-{/last-} would over-capture and fail to parse.
        let model_line = r#"Here is the result } see below: {"status":"done","output":"fixed foo() {bar}"} done now }."#;
        // `$run` simply writes that line to stdout (Write-Output), exactly what a real
        // backend pipeline would do; the wrapper redirects it to the raw file.
        let run = format!("Write-Output {}", ps_single_quote(model_line));
        let wrapper = windows_stdout_to_result_wrapper(&run, &result_path, &raw_path);

        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &wrapper,
            ])
            .status()
            .expect("run wrapper");
        assert!(status.success(), "wrapper exited non-zero");

        let written = std::fs::read_to_string(&result_target).expect("result file");
        let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON result");
        assert_eq!(
            parsed["status"], "done",
            "balanced walk must pick the done object, got: {written}"
        );
        assert_eq!(
            parsed["output"], "fixed foo() {bar}",
            "output value must survive intact: {written}"
        );
        // The raw temp file is cleaned up by the wrapper.
        assert!(
            !result_target.with_extension("json.raw").exists()
                && !std::path::Path::new(&format!("{}.raw", result_target.to_string_lossy()))
                    .exists(),
            "raw temp file must be removed"
        );
        std::fs::remove_dir_all(&scratch).ok();
    }

    // BLOCKER 1 / WARNING 5 (behavioral, Windows): a MULTI-WORD api command must
    // tokenize natively and actually run (here `cmd /c echo ...` proves the words are
    // split into an executable + args), producing a valid `done` result via the same
    // stdout->file wrapper.
    #[cfg(windows)]
    #[test]
    fn windows_api_multiword_command_tokenizes_and_runs() {
        use std::process::Command;
        let scratch = std::env::temp_dir().join(format!("mc_apiwin_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let result_target = scratch.join("d1.json");
        let prompt_file = scratch.join("p.txt");
        std::fs::write(&prompt_file, "ignored prompt").unwrap();
        // The backend's OUTPUT is a valid result JSON; we stage it in a file the
        // multi-word command prints (braces live in the FILE, not on the command line
        // — a real api CLI command line never embeds JSON braces either).
        let json_file = scratch.join("out.json");
        std::fs::write(&json_file, r#"{"status":"done","output":"multiword ok"}"#).unwrap();

        // A real MULTI-WORD command: `cmd /c type <file>` (executable `cmd`, args
        // `/c type <path>`). If the `&`-call-operator bug were present, PowerShell
        // would try to run a single executable literally named the whole string and
        // FAIL. Native tokenization splits it correctly.
        let command = format!("cmd /c type {}", json_file.to_string_lossy());
        let b = backend(MiniCoderBackendKind::Api, None, Some(command.as_str()));
        let cmd =
            build_mini_command_impl(&b, &scratch, &result_target, &prompt_file, None, None).unwrap();
        let script = argv_strings(&cmd).pop().unwrap();

        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .current_dir(&scratch)
            .status()
            .expect("run api script");
        assert!(status.success(), "api script exited non-zero");

        let written = std::fs::read_to_string(&result_target).expect("result file");
        let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(
            parsed["status"], "done",
            "multi-word command must run; got: {written}"
        );
        assert_eq!(parsed["output"], "multiword ok", "got: {written}");
        std::fs::remove_dir_all(&scratch).ok();
    }

    // oMLX-P2 (behavioral, Windows): a DOWN oMLX server (connection refused on a dead
    // loopback port) makes Invoke-RestMethod throw -> the try/catch swallows it -> the
    // run writes NOTHING -> the EXISTING wrapper writes the CLEAN `failed` fallback. No
    // partial garbage, valid JSON, script exits 0. This proves the "non-2xx / refused
    // -> clean failed" contract end-to-end (a non-2xx response also throws, same path).
    #[cfg(windows)]
    #[test]
    fn windows_omlx_down_server_yields_clean_failed_fallback() {
        use std::process::Command;
        let scratch = std::env::temp_dir().join(format!(
            "aspis-omlx-down-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&scratch).unwrap();
        let result_target = scratch.join("d1.json");
        let prompt_file = scratch.join("p.txt");
        std::fs::write(&prompt_file, "summarize this").unwrap();

        // Port 1 on loopback: nothing listens -> immediate connection refused.
        let b = omlx_backend("any-model", "http://127.0.0.1:1");
        let cmd =
            build_mini_command_impl(&b, &scratch, &result_target, &prompt_file, None, None).unwrap();
        let script = argv_strings(&cmd).pop().unwrap();

        let status = Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .current_dir(&scratch)
            .status()
            .expect("run omlx script");
        // The script must NOT propagate the HTTP error (try/catch + exit 0).
        assert!(status.success(), "omlx script must exit 0 even when the server is down");

        let written = std::fs::read_to_string(&result_target).expect("result file written");
        let parsed: serde_json::Value =
            serde_json::from_str(&written).expect("result must be VALID JSON, not partial garbage");
        assert_eq!(
            parsed["status"], "failed",
            "a down/non-2xx oMLX server must yield the clean failed fallback; got: {written}"
        );
        // The raw capture must have been cleaned by the wrapper/finally.
        assert!(
            !scratch.join("d1.json.raw").exists(),
            "the .raw capture must be removed"
        );
        std::fs::remove_dir_all(&scratch).ok();
    }

    // -- macOS command-build parity (compiled + run only on macOS) -----------

    #[cfg(target_os = "macos")]
    fn macos_script(cmd: &portable_pty::CommandBuilder) -> String {
        // /bin/sh -c <script>: the script is the last argv entry.
        cmd.get_argv()
            .last()
            .map(|a| a.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_api_multiword_command_tokenizes_no_call_operator() {
        // BLOCKER 1 / WARNING 5 (macOS): the multi-word command is a pipeline target
        // for /bin/sh, which tokenizes it natively. There is no `&` call operator on
        // sh; we just assert the verbatim command rides after the stdin pipe.
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("p.txt");
        let b = backend(MiniCoderBackendKind::Api, None, Some("mycli chat --json"));
        let cmd = build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, None).unwrap();
        let script = macos_script(&cmd);
        assert!(
            script.contains("cat '") && script.contains("| mycli chat --json"),
            "api command must tokenize natively after the piped prompt file: {script}"
        );
        // FIX 1: the prompt is delivered by piping the FILE directly (bytes
        // preserved), NOT captured into a $PROMPT var (which strips trailing
        // newlines). No `printf '%s' "$PROMPT"` and no `$(cat ...)` capture.
        assert!(
            !script.contains("\"$PROMPT\""),
            "must not deliver prompt via a $PROMPT var: {script}"
        );
        assert!(
            !script.contains("PROMPT=\"$(cat"),
            "must not capture prompt into a var: {script}"
        );
        // FIX 1 (BLOCKER-safe preamble): the trap removes the prompt dir + raw file on
        // ANY exit (success, set -e abort, missing python3). The path VARIABLES are
        // assigned BEFORE the trap (the whitespace-safe variable-indirection), so the
        // script starts with `_MINI_PROMPT_DIR=`, not the trap itself.
        assert!(
            script.starts_with("_MINI_PROMPT_DIR="),
            "preamble must assign the prompt-dir var first: {script}"
        );
        let prompt_dir_idx = script
            .find("_MINI_PROMPT_DIR=")
            .expect("prompt-dir var assigned");
        let trap_idx = script
            .find("trap 'rm -rf ")
            .expect("trap cleanup present");
        assert!(
            prompt_dir_idx < trap_idx,
            "the _MINI_PROMPT_DIR assignment must precede the trap: {script}"
        );
        assert!(
            script.contains("' EXIT\n"),
            "trap must fire on EXIT: {script}"
        );
        // WARNING 7: stdout redirected to a temp file (not MINI_RAW var).
        assert!(
            script.contains("MINI_RAW_FILE"),
            "raw stdout file missing: {script}"
        );
        assert!(
            !script.contains("MINI_RAW=\"$("),
            "must not capture stdout into a var: {script}"
        );
        // BLOCKER 2: raw_decode balanced walk.
        assert!(
            script.contains("raw_decode"),
            "balanced raw_decode walk missing: {script}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_trap_cleans_prompt_dir_and_raw_on_any_exit() {
        // FIX 1 (source-content leak + newline corruption): the trap must remove BOTH
        // the restricted prompt parent dir AND the `.raw` capture, and must be the
        // very first line so it is armed before any `set -e`-abortable command.
        let scratch = std::env::temp_dir();
        let result_target = scratch.join("d1.json");
        let prompt_dir = scratch.join("mc_prompt_dir");
        let prompt_file = prompt_dir.join("p.txt");
        let b = backend(MiniCoderBackendKind::Ollama, Some("qwen2.5-coder"), None);
        let cmd =
            build_mini_command_impl(&b, &scratch, &result_target, &prompt_file, None, None).unwrap();
        let script = macos_script(&cmd);
        // The path VARIABLES are assigned first (whitespace-safe indirection), then the
        // trap references them via double-quoted `$_MINI_*` expansions on EXIT.
        assert!(
            script.starts_with("_MINI_PROMPT_DIR="),
            "preamble must assign the prompt-dir var first: {script}"
        );
        assert!(
            script.contains("trap 'rm -rf \"$_MINI_PROMPT_DIR\" \"$_MINI_RAW_FILE\" 2>/dev/null || true; [ -n \"$_MINI_KEY_DIR\" ] && rm -rf \"$_MINI_KEY_DIR\" 2>/dev/null || true' EXIT"),
            "trap must remove prompt dir + raw + (guarded) key dir via vars on EXIT: {script}"
        );
        // Both the prompt DIR and the .raw file are assigned to the vars the trap removes.
        assert!(
            script.contains(&format!(
                "_MINI_PROMPT_DIR={}",
                sh_single_quote_local(&prompt_dir.to_string_lossy())
            )),
            "the prompt dir must be assigned to _MINI_PROMPT_DIR: {script}"
        );
        assert!(
            script.contains(".raw'\n"),
            "the .raw capture must be assigned to _MINI_RAW_FILE: {script}"
        );
    }

    // BLOCKER (FIX 1, behavioral, macOS): the prompt bytes reach the backend
    // VERBATIM (trailing newline preserved), and the prompt dir + .raw are gone after
    // the script exits — even when the backend writes nothing.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_prompt_bytes_preserved_and_files_cleaned_after_exit() {
        use std::process::Command;
        let scratch = std::env::temp_dir().join(format!("mc_fix1_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let prompt_dir = scratch.join("prompt");
        std::fs::create_dir_all(&prompt_dir).unwrap();
        let prompt_file = prompt_dir.join("p.txt");
        // A prompt WITH a trailing newline — the old $(...) capture would strip it.
        std::fs::write(&prompt_file, "line1\nline2\n").unwrap();
        let result_target = scratch.join("d1.json");
        let echoed = scratch.join("echoed.bin");
        // api backend whose "command" tees stdin to a file then emits a valid result.
        let command = format!(
            "tee {} >/dev/null; printf '%s' '{{\"status\":\"done\",\"output\":\"ok\"}}'",
            echoed.to_string_lossy()
        );
        let b = backend(MiniCoderBackendKind::Api, None, Some(command.as_str()));
        let cmd =
            build_mini_command_impl(&b, &scratch, &result_target, &prompt_file, None, None).unwrap();
        let script = macos_script(&cmd);
        let status = Command::new("/bin/sh")
            .args(["-c", &script])
            .status()
            .expect("run script");
        assert!(status.success(), "script exited non-zero");
        // Prompt bytes preserved EXACTLY (trailing newline kept).
        let seen = std::fs::read(&echoed).expect("echoed prompt");
        assert_eq!(
            seen, b"line1\nline2\n",
            "prompt bytes must be delivered verbatim"
        );
        // The restricted prompt dir + .raw capture are GONE (trap fired on EXIT).
        assert!(
            !prompt_dir.exists(),
            "prompt dir must be removed by the trap"
        );
        let raw = scratch.join("d1.json.raw");
        assert!(!raw.exists(), "raw capture must be removed by the trap");
        std::fs::remove_dir_all(&scratch).ok();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_codex_never_adds_mcp_flags() {
        // MINOR 9 (macOS): no MCP grant even with roots supplied.
        let root = std::env::temp_dir();
        let result_target = root.join("d1.json");
        let prompt_file = root.join("p.txt");
        let b = backend(MiniCoderBackendKind::Codex, Some("gpt-5-codex"), None);
        let roots = McpRoots {
            management_root: PathBuf::from("/mgmt"),
            projects_dir: PathBuf::from("/mgmt/projects"),
        };
        let cmd =
            build_mini_command_impl(&b, &root, &result_target, &prompt_file, None, Some(&roots)).unwrap();
        let script = macos_script(&cmd);
        // Every arg is single-quoted by sh_single_quote_local (semantically
        // identical for /bin/sh: 'exec' is still the literal word exec).
        assert!(
            script.contains("| codex 'exec'"),
            "codex exec missing: {script}"
        );
        assert!(
            script.contains("'-m' 'gpt-5-codex'"),
            "model flag missing: {script}"
        );
        assert!(
            !script.contains("mcp_servers"),
            "mini must never get MCP: {script}"
        );
        assert!(
            !script.contains("-c "),
            "mini must never get a -c flag: {script}"
        );
    }

    // BLOCKER 2 (behavioral, macOS): run the REAL python wrapper to prove the
    // balanced raw_decode walk extracts the correct done object despite trailing `}`.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_wrapper_balanced_walk_extracts_done_with_braces_in_output() {
        use std::process::Command;
        let scratch = std::env::temp_dir().join(format!("mc_b2mac_{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let result_target = scratch.join("d1.json");
        let result_path = sh_single_quote_local(&result_target.to_string_lossy());
        let raw_path = sh_single_quote_local(&format!("{}.raw", result_target.to_string_lossy()));

        let model_line = r#"Here is the result } see below: {"status":"done","output":"fixed foo() {bar}"} done now }."#;
        let run = format!("printf '%s' {}", sh_single_quote_local(model_line));
        let wrapper = macos_stdout_to_result_wrapper(&run, &result_path, &raw_path);

        let status = Command::new("/bin/sh")
            .args(["-c", &wrapper])
            .status()
            .expect("run wrapper");
        assert!(status.success(), "wrapper exited non-zero");

        let written = std::fs::read_to_string(&result_target).expect("result file");
        let parsed: serde_json::Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(parsed["status"], "done", "got: {written}");
        assert_eq!(parsed["output"], "fixed foo() {bar}", "got: {written}");
        std::fs::remove_dir_all(&scratch).ok();
    }

    // ---- TRAINING RAIL: record_directive_result is called after finalize ----

    /// Exercises the `record_directive_result` call-site inside `finalize_finished_mini`
    /// without a full AppHandle/Tauri runtime, by calling the training-export function
    /// directly with the same arguments the call-site would produce.
    ///
    /// LOCK-ORDERING: the call is placed after `mutate_agent_live_state` returns (the
    /// agent-state lock is released); we verify this structurally: the call is outside
    /// the `mutate_agent_live_state` closure, after `applied.is_ok()`.
    #[test]
    fn finalize_training_rail_writes_directive_result_line() {
        use super::super::training_export;

        // Build a project_root / scratch_root structure:
        //   <tmp>/project_root/.aspis-mini/
        let base = std::env::temp_dir()
            .join(format!("mc_train_rail_{}", std::process::id()));
        let project_root = base.join("project_root");
        let scratch = project_root.join(".aspis-mini");
        std::fs::create_dir_all(&scratch).unwrap();

        // Build a `done` outcome that touched src/a.rs.
        std::fs::create_dir_all(project_root.join("src")).unwrap();
        std::fs::write(project_root.join("src").join("a.rs"), b"fn a() {}").unwrap();

        let mut d = directive("train1", "coder-1");
        d.status = MiniCoderStatus::Running;
        d.task = "add a docstring".into();
        d.files = vec!["src/a.rs".into()];
        d.result_path = "train1.json".into();
        d.scratch_path = Some(scratch.to_string_lossy().to_string());

        let mut outcome = MiniCoderOutcome::default();
        outcome.status = MiniCoderStatus::Done;
        outcome.output = Some("added docstring".into());
        outcome.files_touched = vec!["src/a.rs".into()];

        // Derive project_root exactly as finalize_finished_mini does: parent of scratch.
        let derived_root = Path::new(d.scratch_path.as_deref().unwrap()).parent().unwrap();
        assert_eq!(
            derived_root.canonicalize().ok(),
            project_root.canonicalize().ok(),
            "derived project root must equal the actual project root"
        );

        // LOCK-ORDERING CONTRACT: call with no lock held (no Mutex around this block).
        training_export::record_directive_result(derived_root, &d, &outcome);

        // Verify a `directive_result` line was written to pairs.jsonl.
        let pairs_path = project_root
            .join(".aspis-training")
            .join("pairs.jsonl");
        assert!(pairs_path.exists(), ".aspis-training/pairs.jsonl must be created");
        let body = std::fs::read_to_string(&pairs_path).unwrap();
        let lines: Vec<serde_json::Value> = body
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("valid JSON line"))
            .collect();
        assert_eq!(lines.len(), 1, "one directive_result line");
        let rec = &lines[0];
        assert_eq!(rec["type"], "directive_result", "type field");
        assert_eq!(rec["directiveId"], "train1", "directiveId field");
        assert_eq!(rec["parentAgentId"], "coder-1", "parentAgentId field");
        assert_eq!(rec["task"], "add a docstring", "task field");
        assert_eq!(rec["status"], "done", "status field");
        assert_eq!(
            rec["output"].as_str(),
            Some("added docstring"),
            "output field"
        );
        // filesTouched must contain src/a.rs.
        let files_touched = rec["filesTouched"].as_array().expect("filesTouched array");
        assert!(
            files_touched.iter().any(|v| v == "src/a.rs"),
            "filesTouched must contain the changed file"
        );

        std::fs::remove_dir_all(&base).ok();
    }
}
