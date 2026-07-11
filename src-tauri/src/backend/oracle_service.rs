//! Resident, app-supervised Oracle HTTP server (Step 4b).
//!
//! The Oracle retrieval server (`python -m oracle.server.main`) runs as a
//! resident process tied to the APP PROCESS lifecycle (NOT the vault lock). Once
//! a workspace index is available the supervisor brings it up and publishes a
//! discovery file so MCP thin-clients can find it. The server, the supervisor,
//! and the discovery file then KEEP RUNNING across a vault lock — they are torn
//! down only when the app process EXITS (see [`on_app_exit`]).
//!
//! ## SECURITY POSTURE (deliberate relaxation — read this)
//! The resident server, its scoped AGENT token (published in the discovery
//! file), and its corpus access now PERSIST ACROSS A VAULT LOCK BY DESIGN, so
//! agents enjoy always-on MCP access and any in-flight index/embedding job keeps
//! running through a screen-lock. This is a deliberate relaxation of the prior
//! "lock ⇒ no agent access / no discovery file" invariant: the trust boundary is
//! now the app PROCESS, not the vault session. The discovery file is removed and
//! the server killed only on app exit ([`on_app_exit`]). The vault's own
//! in-memory secret clearing on lock/idle-expiry is unchanged and no longer
//! touches the Oracle server.
//!
//! ## Two-tier auth
//! The server is spawned with BOTH tokens (see `oracle::python_oracle`): the
//! OPERATOR token (`ORACLE_AUTH_TOKEN`, used by the app/UI path to reach every
//! endpoint) and the AGENT token (`ORACLE_AGENT_AUTH_TOKEN`, bounded endpoints
//! only). The discovery file publishes ONLY the AGENT token.
//!
//! ## Threading model
//! State lives in process-wide statics (mirroring `python_oracle`'s child/lock
//! statics) so the lock/unlock hooks in `BackendState` — which have no
//! `AppHandle` — can drive it. `init()` (called from the Tauri setup hook, which
//! DOES have the handle) records the resolved projects dir up front.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;

use crate::oracle::oracle_setup::install_in_progress;
use crate::oracle::python_oracle::{
    ensure_oracle_server, oracle_agent_token, oracle_server_ready, oracle_session_endpoint,
    run_python_oracle_http_post,
};

/// Discovery filename read by the Python thin-client
/// (`aspis_mcp.ORACLE_DISCOVERY_FILENAME`). MUST stay in sync with that constant.
const DISCOVERY_FILENAME: &str = ".oracle-server.json";

/// Supervisor poll interval. Every tick re-evaluates [`should_restart`].
const SUPERVISOR_TICK: Duration = Duration::from_secs(10);

/// The projects directory under which the discovery file is published. Seeded
/// (with the `AppHandle`) in [`init`]; the lock/unlock hooks read it without a
/// handle. An unset value disables publishing — the operator path to `/ask`
/// still works regardless. Stored as a `Mutex<Option<…>>` (not a write-once
/// `OnceLock`) so that if the first `init` resolution was `None` (the projects
/// dir was not yet creatable at setup), [`ensure_projects_dir_resolved`] can
/// re-resolve it later via the handle-free fallback instead of disabling
/// publishing permanently for the process.
static PROJECTS_DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

fn projects_dir_slot() -> &'static Mutex<Option<PathBuf>> {
    PROJECTS_DIR.get_or_init(|| Mutex::new(None))
}

/// Handle + stop flag for the running supervisor thread. `None` when stopped.
static SUPERVISOR: OnceLock<Mutex<Option<Supervisor>>> = OnceLock::new();

/// Process-wide "the app is EXITING — no discovery file may exist" flag. Set
/// `true` by [`on_app_exit`] BEFORE the file is deleted (and never cleared — the
/// process is going away). [`publish_discovery`] re-checks it while holding
/// [`discovery_lock`] and refuses to write when set — this is what closes the
/// publish-vs-delete resurrection race at shutdown (a supervisor tick that began
/// before `on_app_exit` could otherwise rewrite the file after its delete).
///
/// NOTE (lifecycle change): this is NO LONGER tied to the VAULT lock. A vault
/// lock keeps the server + discovery file ALIVE by design (always-on agent MCP),
/// so [`on_lock`] no longer touches this flag and publishing is NOT suppressed
/// while the vault is locked. Only app exit flips it.
static EXITING: AtomicBool = AtomicBool::new(false);

/// Process-wide "the file watcher has been started AND the index warmed" flag.
/// Set `true` after the supervisor POSTs `/index/watch/start` + the one warm
/// `/index/run` (see [`reconcile_once`]). This is what makes the auto-watch/warm
/// idempotent: the supervisor's ~10s tick must NOT restart the watcher or re-kick
/// a full index run every interval (that would fight the watcher's own
/// incremental runs and churn CPU). The Python `start_watcher` is itself
/// idempotent (returns "watching" if an observer already exists), but gating here
/// also avoids a needless HTTP round-trip every tick.
///
/// LIFECYCLE: armed once per APP PROCESS now, NOT once per unlock. The server (and
/// thus its watcher) survives a vault lock, so [`on_lock`] no longer re-arms this;
/// a manual watcher stop still re-arms it via [`reset_watcher_armed`]. If the
/// server ever dies and the supervisor respawns it, the new server has no watcher,
/// so [`maybe_start_watcher_and_warm`] re-arms on a watch-start failure (the flag
/// is reset on the failed HTTP call) and the next tick re-claims.
static WATCHER_STARTED: AtomicBool = AtomicBool::new(false);

/// Process-wide "the resident Oracle server must be restarted because its LLM
/// credentials changed" flag. The resident server captures the LLM key/provider
/// in its spawn ENV and never re-reads the vault, so after the user saves new
/// Oracle LLM settings an already-running server would keep its STALE key. The
/// save command MUST NOT block on `child.kill()` + `child.wait()` (a slow reap
/// freezes the Tauri command → the UI looks crashed), so instead it sets THIS
/// flag and returns immediately. The supervisor's ~10s tick observes the flag,
/// claims it ATOMICALLY (single-winner via compare_exchange), and tears the
/// server down OFF the UI thread; the next tick / `/ask` respawns it with the
/// freshly-saved credentials. A no-op when no server is running.
static LLM_RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request a resident-server restart on the next supervisor tick (non-blocking).
/// Called from the save-LLM-settings command so the command returns within ~100ms
/// regardless of server teardown time. Idempotent: setting an already-set flag is
/// harmless; the supervisor coalesces multiple requests into one teardown.
pub fn request_llm_restart() {
    LLM_RESTART_REQUESTED.store(true, Ordering::SeqCst);
}

/// Serializes [`publish_discovery`] and [`delete_discovery`] so a publish can
/// never interleave with (and thereby resurrect) a delete, and two briefly
/// overlapping supervisors cannot race on the same target/backup. Held for the
/// WHOLE of each operation.
static DISCOVERY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn discovery_lock() -> &'static Mutex<()> {
    DISCOVERY_LOCK.get_or_init(|| Mutex::new(()))
}

struct Supervisor {
    stop: Arc<AtomicBool>,
    /// Owns the thread and lets `start_supervisor` check `is_finished()` (and the
    /// `stop` flag) for idempotency vs. supersede. [`stop_supervisor`] (the `on_lock`
    /// path) NEVER joins it and LEAVES it in the slot — stop is non-blocking there; the
    /// signalled thread exits on its own via the `stop` flag and is reaped later.
    /// [`start_supervisor`] (the `on_unlock` path), by contrast, finds this retained
    /// handle, signals stop, and does a BOUNDED join (outside the slot lock) before
    /// spawning a replacement, so at most one supervisor is ever actively (re)spawning
    /// (the single-instance invariant). The bounded join is near-instant because the
    /// loop honors the stop flag inside its bring-up wait (see
    /// `wait_for_oracle_server_ready`'s and `wait_for_oracle_port_free`'s abort).
    handle: JoinHandle<()>,
}

/// Upper bound on how long [`start_supervisor`] waits for a superseded supervisor to
/// exit before spawning its replacement. `on_unlock` runs on the unlock command
/// thread and MUST stay responsive, so this is deliberately short. With the
/// stop-flag-honoring bring-up wait (Fix #1) the old thread exits within ~one 250ms
/// poll slice, so this bound is virtually never reached; if it is (the old thread is
/// wedged in something that does not check the flag), we log and proceed degraded.
const SUPERVISOR_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

fn supervisor_slot() -> &'static Mutex<Option<Supervisor>> {
    SUPERVISOR.get_or_init(|| Mutex::new(None))
}

/// The discovery file contract (camelCase JSON). `auth_token` is ALWAYS the AGENT
/// token (bounded-only); `base_url` is ALWAYS loopback. Other fields are
/// bookkeeping for humans/diagnostics.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryFile {
    base_url: String,
    auth_token: String,
    index_root: String,
    pid: u32,
    updated_at: String,
}

/// Record the projects directory (resolved with the `AppHandle` in setup).
/// Overwrites any prior value; the normal path calls it once from setup.
pub fn init(projects_dir: PathBuf) {
    let mut slot = projects_dir_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *slot = Some(projects_dir);
}

/// Ensure the projects dir is resolved, returning the resolved value. If [`init`]
/// already seeded it, return that. Otherwise attempt a handle-free re-resolution
/// (`ASPIS_PROJECTS_DIR` env override, then a `config.json`/`projects`-bearing
/// cwd/parent — the same logic as `lib::resolve_projects_dir`, minus the
/// `AppHandle`-only app-data fallback, which the supervisor cannot reach).
/// `None` ⇒ publishing stays disabled this tick, retried on the next.
fn ensure_projects_dir_resolved() -> Option<PathBuf> {
    let mut slot = projects_dir_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = slot.as_ref() {
        return Some(dir.clone());
    }
    if let Some(dir) = resolve_projects_dir_handle_free() {
        *slot = Some(dir.clone());
        return Some(dir);
    }
    None
}

/// Handle-free projects-dir resolution shared with setup (`lib::resolve_projects_dir`
/// calls this first, then falls back to the app-data dir which needs the handle).
/// Pure except for reading the env + cwd. Extracted so the supervisor — which has
/// no `AppHandle` — can re-resolve a late-available projects dir.
pub(crate) fn resolve_projects_dir_handle_free() -> Option<PathBuf> {
    const PROJECTS_SUBDIR: &str = "projects";
    if let Ok(value) = std::env::var("ASPIS_PROJECTS_DIR") {
        let path = PathBuf::from(value.trim());
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("config.json").exists() || cwd.join(PROJECTS_SUBDIR).exists() {
            return Some(cwd.join(PROJECTS_SUBDIR));
        }
        if let Some(parent) = cwd.parent() {
            if parent.join("config.json").exists() || parent.join(PROJECTS_SUBDIR).exists() {
                return Some(parent.join(PROJECTS_SUBDIR));
            }
        }
    }
    None
}

/// The resident server's workspace/index root, or `None` if no usable workspace
/// root is configured. This delegates to the SAME shared resolver the operator
/// `/ask` path uses (`commands::current_oracle_index_root`, reading the vault
/// index preferences), so the resident server is started against — and
/// `oracle_server_ready` matched against — the exact root the UI queries. If the
/// two diverged, `oracle_server_ready` would never match the server's
/// `server_root` and the supervisor + operator path would fight over the single
/// child/port in an endless kill-restart loop, timing out every `ask_oracle`.
fn index_root() -> Option<PathBuf> {
    crate::oracle::commands::current_oracle_index_root().ok()
}

/// The absolute discovery-file path, or `None` if publishing is disabled (no
/// projects dir resolvable). Triggers a (cheap, handle-free) re-resolution if the
/// dir was never seeded, so a late-available projects dir still enables publishing.
fn discovery_path() -> Option<PathBuf> {
    ensure_projects_dir_resolved().map(|dir| dir.join(DISCOVERY_FILENAME))
}

/// Whether the discovery file currently exists on disk. `false` when publishing
/// is disabled (no projects dir) so the steady-state tick stays a no-op there.
fn discovery_file_present() -> bool {
    discovery_path().map(|p| p.is_file()).unwrap_or(false)
}

/// PURE restart predicate: restart the resident server ONLY when the app is
/// unlocked, a workspace root exists, no install is mid-flight, and the server
/// is not currently ready. Every other combination is a no-op.
pub(crate) fn should_restart(
    unlocked: bool,
    has_root: bool,
    mid_install: bool,
    server_ready: bool,
) -> bool {
    unlocked && has_root && !mid_install && !server_ready
}

/// PURE auto-watch predicate: start the file watcher (and kick the one warm
/// index run) ONLY when the server is ready, the `autoWatchOnUnlock` preference
/// is enabled, and we have not already armed the watcher this session. Every
/// other combination is a no-op, which keeps the supervisor's ~10s tick from
/// restarting the watcher / re-running a full index every interval.
pub(crate) fn should_start_watcher(
    server_ready: bool,
    auto_watch_pref: bool,
    already_watching: bool,
) -> bool {
    server_ready && auto_watch_pref && !already_watching
}

/// PURE commit-index predicate: kick an incremental Oracle index ONLY when the
/// `index_mode` preference is `"commit"` AND the resident server is reachable.
/// Every other combination is a no-op so normal "watch" mode is never affected.
pub(crate) fn should_kick_commit_index(mode: Option<&str>, server_ready: bool) -> bool {
    mode == Some("commit") && server_ready
}

/// Build the `/index/watch/start` query URL from a root and an optional mode.
///
/// * `mode == Some("commit")` → appends `&mode=commit` (git-refs watcher).
/// * Any other value (including `Some("watch")` and `None`) → no `mode` param
///   (server default: heavy fs watcher).
///
/// Kept as a pure function so it can be unit-tested without touching the
/// process-wide statics or the keyring.
pub(crate) fn build_watch_start_url(root_query: &str, mode: Option<&str>) -> String {
    if mode == Some("commit") {
        format!("/index/watch/start?{root_query}&mode=commit")
    } else {
        format!("/index/watch/start?{root_query}")
    }
}

/// Atomically claim the one-shot auto-watch/warm for THIS session against the
/// given flag. Returns `true` for exactly one caller (the winner), `false` for
/// every other (the flag was already armed). This closes the load→check→store
/// TOCTOU where two briefly-overlapping supervisor ticks could both pass
/// [`should_start_watcher`] and double-POST `/index/watch/start`.
///
/// On a watch-start failure the winner must reset the flag (see
/// [`maybe_start_watcher_and_warm`]) so the next tick can re-claim and retry.
fn try_claim_watcher_start(flag: &AtomicBool) -> bool {
    flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// React to an unlock: start the supervisor, which will bring up the resident
/// server (if a workspace index is available) and publish the discovery file on
/// its first tick.
///
/// This MUST NOT block the caller: `unlock_after_verification` runs on the
/// command's thread and a COLD server start loads the embedding model (~30-60s);
/// doing that synchronously would freeze the unlock. `ensure_oracle_server` also
/// builds/uses the blocking reqwest client, which must never run on a tokio async
/// worker. The supervisor thread owns both concerns: it does the (blocking)
/// bring-up + publish off the caller's thread, and re-publishes/restarts on its
/// poll loop. The operator `/ask` path lazily starts the server on demand anyway,
/// so a slow first tick never blocks a query either.
pub fn on_unlock() {
    if !oracle_is_enabled() {
        return;
    }
    // Opportunistically (re)resolve the projects dir now that the app is unlocked:
    // if setup's first resolution was None, a config-bearing cwd may exist by now,
    // re-enabling discovery publishing. Cheap and best-effort; the supervisor's
    // ticks re-resolve via `discovery_path()` regardless.
    let _ = ensure_projects_dir_resolved();
    // Idempotent: if a supervisor is already live (it survives a vault lock now),
    // `start_supervisor` leaves it in place. On the FIRST unlock it brings the
    // server up + publishes; on a re-unlock after a lock it is a no-op because the
    // server/supervisor never went away.
    start_supervisor();
}

/// React to a vault LOCK / idle-expiry.
///
/// LIFECYCLE CHANGE (always-on agent MCP): a vault lock NO LONGER tears down the
/// Oracle server lifecycle. The supervisor, the resident server, and the discovery
/// file all KEEP RUNNING across the lock by design, so agents keep querying the
/// bounded endpoints and any in-flight index/embedding job continues through a
/// screen-lock. The server is process-scoped and is torn down only on app exit
/// ([`on_app_exit`]).
///
/// Consequently this is now a NO-OP for the Oracle server: it does NOT stop the
/// supervisor, does NOT delete the discovery file, and does NOT touch the
/// `EXITING` gate or the `WATCHER_STARTED` one-shot (the watcher survives the lock
/// with its server). It also does NOT clear `LLM_RESTART_REQUESTED`: a pending
/// restart should still apply to the still-running server.
///
/// Kept as an explicit (empty-bodied) hook so the lock call sites remain wired and
/// a future non-server lock concern has an obvious home. Performs no I/O and never
/// blocks, so it is safe from any context (including a tokio async worker).
pub fn on_lock() {
    // Intentionally empty: the Oracle server + discovery file are tied to the APP
    // PROCESS lifecycle, not the vault session (see module docs / `on_app_exit`).
}

/// Tear down the Oracle server lifecycle on APP EXIT (and ONLY on app exit).
///
/// This is the single teardown point now that the server survives a vault lock:
/// set the `EXITING` gate so no in-flight supervisor tick can re-publish, stop the
/// supervisor (non-blocking signal), kill the resident server child (bounded reap,
/// no network I/O — safe during process shutdown / from `Drop`), and delete the
/// discovery file so no stale AGENT token is left on disk after the process is
/// gone. Idempotent and best-effort: safe to call more than once (e.g. from both
/// the `RunEvent::Exit` handler and `BackendState::drop`).
///
/// On Windows the child does NOT die automatically when the parent exits, so this
/// EXPLICIT teardown is REQUIRED to avoid orphaning the server on app close.
pub fn on_app_exit() {
    // Set the EXITING gate FIRST. Any `publish_discovery` that has not yet taken
    // `discovery_lock` will, once it does, observe this and skip the write; any
    // that is mid-flight already holds the lock, so the `delete_discovery` below
    // runs strictly after it and removes whatever it wrote. Either way no stale
    // discovery file survives process exit. Never cleared — the process is going
    // away. (`stop_supervisor` also sets the per-thread stop flag.)
    EXITING.store(true, Ordering::SeqCst);
    stop_supervisor();
    // Kill the resident server child. NO network I/O (the courtesy watcher-stop
    // HTTP is intentionally skipped here): `kill_python_oracle_child` only kills +
    // bounded-reaps the child, so this is safe during tokio runtime teardown / from
    // `Drop` without risking a blocking-client drop panic. On Windows this is what
    // prevents an orphaned server after app close.
    let _ = crate::oracle::python_oracle::kill_python_oracle_child();
    delete_discovery();
}

/// Re-arm the one-shot auto-watch flag WITHOUT tearing anything else down.
///
/// FIX 4: when the user manually stops the watcher (`stop_oracle_index_watcher`),
/// the supervisor's [`WATCHER_STARTED`] one-shot would otherwise stay armed for the
/// rest of the unlocked session, so the supervisor would never re-start the watcher
/// even though autoWatch is still enabled. Resetting the flag lets the next
/// supervisor tick re-claim the start (respecting the autoWatch preference), so a
/// manual stop is a one-off, not a permanent session-wide disable.
///
/// This performs NO teardown (no `EXITING` flip, no `stop_supervisor`, no
/// `delete_discovery`): the resident server keeps running; only the watcher
/// one-shot is re-armed.
pub(crate) fn reset_watcher_armed() {
    WATCHER_STARTED.store(false, Ordering::SeqCst);
}

/// Start the supervisor thread, guaranteeing AT MOST ONE live supervisor.
///
/// SINGLE-INSTANCE INVARIANT (the double-spawn fix): exactly one supervisor at a time,
/// enforced by SLOT-RETENTION + a BOUNDED JOIN on replacement. `stop_supervisor` /
/// `on_lock` only SIGNAL stop and LEAVE the Supervisor in the slot (they never join —
/// they must stay responsive). So `start_supervisor` (the on_unlock path) ALWAYS finds
/// the previous supervisor in the slot and reacts to its state:
///   * FINISHED or stop-flag-SET → take it, (re-)signal stop, and BOUNDED-join it (up
///     to [`SUPERVISOR_JOIN_TIMEOUT`]) before spawning the replacement, so the old
///     thread is no longer (re)spawning a server when the new one starts. (We check the
///     STOP FLAG, not just `is_finished()`: a stop-signalled thread still mid-bring-up
///     has not finished yet but must still be superseded.)
///   * Genuinely LIVE and NOT stop-set (idempotent re-entry — same session, no
///     intervening lock) → leave it in place and return; no second supervisor.
///
/// This join is the REAL mechanism, not a no-op: because the slot is retained across a
/// stop, the previous thread is always present to be joined. `oracle_server_start_lock`
/// (+ the stop-honoring bring-up waits — `wait_for_oracle_server_ready` /
/// `wait_for_oracle_port_free`, which abort within ~one poll slice on the stop flag)
/// are the SERIALIZATION BACKSTOP that makes the brief overlap / empty-slot window safe.
///
/// `on_unlock` calls this on the unlock command thread, which MUST stay responsive, so
/// the join is bounded AND performed OUTSIDE the slot lock (Fix #1): if the old thread
/// does not exit within the bound we LOG (redacted) and proceed degraded (it was
/// signalled, will exit shortly, and a brief overlap is safe). With the stop-honoring
/// waits the old thread exits within ~one poll slice, so the join is near-instant in
/// practice.
fn start_supervisor() {
    // STEP 1 — decide under the slot lock, then RELEASE it before any join (Fix #1).
    // We either (a) leave a genuinely-live, not-stopped supervisor in place and return
    // (idempotent re-entry), or (b) TAKE the previous supervisor out of the slot so we
    // can bounded-join it OUTSIDE the lock. Holding the slot mutex across `bounded_join`
    // (up to SUPERVISOR_JOIN_TIMEOUT) would let a rapid on_unlock stall `stop_supervisor`
    // / `on_lock` on the slot mutex for up to that bound — so the join MUST be lock-free.
    let previous = {
        let mut slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            // A genuinely LIVE supervisor that has NOT been asked to stop is already
            // doing the job (idempotent re-entry, e.g. a duplicate unlock hook with no
            // intervening lock): leave it in the slot and return — no second supervisor.
            Some(existing)
                if !existing.handle.is_finished() && !existing.stop.load(Ordering::SeqCst) =>
            {
                return;
            }
            // Otherwise the slot holds a previous supervisor that is FINISHED or whose
            // stop flag is set (signalled by `on_lock`/`stop_supervisor` and retained in
            // the slot): take it out so we can join it below, then spawn the replacement.
            // (`None` → nothing to supersede.)
            _ => slot.take(),
        }
    };

    // STEP 2 — supersede the previous supervisor OUTSIDE the slot lock. It was already
    // signalled (by stop_supervisor/on_lock) OR has finished; set the flag idempotently
    // to be safe, then bounded-join so it is no longer (re)spawning before we start the
    // replacement. The brief empty-slot window here is safe: unlock is UI-serialized and
    // `oracle_server_start_lock` (+ the stop-honoring bring-up waits) backstop any
    // double-spawn — and on_lock/stop_supervisor can now run without stalling on the
    // slot mutex because we are NOT holding it.
    if let Some(previous) = previous {
        previous.stop.store(true, Ordering::SeqCst);
        if !bounded_join(previous.handle, SUPERVISOR_JOIN_TIMEOUT) {
            // Degraded path: the old thread did not exit within the bound. It has been
            // signalled and will exit on its next flag check; a brief overlap is safe
            // (ensure_oracle_server is serialized + the stopping thread will not spawn).
            // Logged WITHOUT any path/secret — purely a diagnostic count.
            eprintln!(
                "oracle supervisor: previous instance did not exit within {SUPERVISOR_JOIN_TIMEOUT:?}; \
                 proceeding (it will exit shortly)"
            );
        }
    }

    // STEP 3 — spawn the replacement and re-acquire the slot lock to store it.
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("oracle-supervisor".into())
        .spawn(move || supervisor_loop(&thread_stop))
        .expect("failed to spawn Oracle supervisor thread");
    *supervisor_slot().lock().unwrap_or_else(|e| e.into_inner()) =
        Some(Supervisor { stop, handle });
}

/// Join `handle`, but give up after `timeout`. Returns `true` if the thread finished
/// (and was joined) within the bound, `false` if it was still running when the bound
/// elapsed (the handle is then dropped/detached — the thread exits on its own via the
/// already-set stop flag). Implemented by polling `is_finished()` in short slices
/// because `std::thread::JoinHandle` has no timed join; the poll never busy-spins
/// (10ms slices) and, in the common case, the thread is already finished on the first
/// check so this returns immediately.
fn bounded_join(handle: JoinHandle<()>, timeout: Duration) -> bool {
    const SLICE: Duration = Duration::from_millis(10);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if handle.is_finished() {
            // Reap it so no zombie thread state lingers; the thread is done so this
            // join returns immediately.
            let _ = handle.join();
            return true;
        }
        if std::time::Instant::now() >= deadline {
            // Bound reached: drop (detach) the handle. The thread was already signalled
            // to stop and will exit on its next flag check.
            return false;
        }
        std::thread::sleep(SLICE);
    }
}

/// Signal the supervisor to stop. Idempotent and NON-BLOCKING: it does NOT join.
///
/// `on_app_exit` is the only caller now (the lock path no longer stops the
/// supervisor). It must stay responsive even though the supervisor may be inside a
/// blocking `ensure_oracle_server` (a cold model load is ~30-60s); joining would
/// hang the shutdown that whole time. Instead we set the stop flag (honored after
/// the current blocking op) and detach: the thread exits on its next flag check.
/// A stopping supervisor refuses to publish (it re-checks `stop` + `EXITING` in
/// [`reconcile_once`]), so it cannot resurrect a discovery file `on_app_exit`
/// deleted.
fn stop_supervisor() {
    let slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(supervisor) = slot.as_ref() {
        supervisor.stop.store(true, Ordering::SeqCst);
        // SINGLE-INSTANCE: we SIGNAL stop only and LEAVE the Supervisor in the slot —
        // we do NOT `take()` it. Retaining it is what lets the next `start_supervisor`
        // (the on_unlock path) actually FIND and BOUNDED-JOIN this thread before
        // spawning a replacement, so the join is the real single-instance mechanism
        // (not an inert no-op on a None slot). Still NON-BLOCKING here. The signalled
        // thread observes the flag and returns on its own; a finished-but-retained
        // Supervisor is harmless (its thread has exited) and is reaped by the next
        // start_supervisor.
    }
}

/// The supervisor body: reconcile immediately (tick 0, so the discovery file
/// appears right after unlock without waiting a full interval), then every
/// [`SUPERVISOR_TICK`]. The loop owns NO BackendState. It runs for the APP PROCESS
/// lifetime now (it survives a vault lock) and is stopped (via the flag) only by
/// `on_app_exit` / a `start_supervisor` supersede.
fn supervisor_loop(stop: &AtomicBool) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        reconcile_once(stop);

        // Sleep the policy interval in small slices so that BOTH a stop signal and
        // a freshly-requested LLM restart are honored within ~one slice (~250ms),
        // even though the regular reconcile cadence is ~10s.
        match wait_for_next_tick(stop, &LLM_RESTART_REQUESTED, SUPERVISOR_TICK) {
            TickWake::Stop => return,
            // RestartRequested / TickElapsed both fall through to the next
            // `reconcile_once`; the difference is only how soon we got here.
            TickWake::RestartRequested | TickWake::TickElapsed => {}
        }
    }
}

/// Why [`wait_for_next_tick`] returned.
#[derive(Debug, PartialEq, Eq)]
enum TickWake {
    /// The stop flag was observed — the supervisor must exit.
    Stop,
    /// An LLM restart was requested mid-wait — break early to reconcile NOW.
    RestartRequested,
    /// The full policy interval elapsed with no signal — normal ~10s cadence.
    TickElapsed,
}

/// Sleep `interval` in ~250ms slices, returning EARLY the moment `stop` or
/// `restart` is observed so a save→respawn happens within ~1-2s instead of waiting
/// out the full interval. Only the two cheap atomic loads run per slice (no
/// reconcile / health check), so this never busy-spins the CPU. `restart` is only
/// PEEKED (load, not claimed): the caller's `reconcile_once` does the atomic
/// compare_exchange claim, so multiple slices observing the flag still yield a
/// single teardown. Extracted (and pure w.r.t. the two flags + a real sleep) so a
/// test can drive it with a pre-set flag and a SHORT interval — no real long sleep.
fn wait_for_next_tick(stop: &AtomicBool, restart: &AtomicBool, interval: Duration) -> TickWake {
    const SLICE: Duration = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    while waited < interval {
        if stop.load(Ordering::SeqCst) {
            return TickWake::Stop;
        }
        if restart.load(Ordering::SeqCst) {
            return TickWake::RestartRequested;
        }
        std::thread::sleep(SLICE);
        waited += SLICE;
    }
    TickWake::TickElapsed
}

/// One reconciliation pass (also the unit of work in the ignored e2e test):
/// if a workspace root exists and no install is mid-flight, ensure the server is
/// up ([`should_restart`]) and publish the discovery file whenever the server is
/// ready. Publishing on the "already ready" path recreates the file if it was ever
/// removed out-of-band. `unlocked = true` is passed UNCONDITIONALLY because the
/// server is now app-process-scoped: the supervisor keeps the server up regardless
/// of the vault lock state (always-on agent MCP), and is only stopped on app exit.
///
/// EDGE CASE (respawn while the vault is locked): if the server dies and is
/// respawned here while the vault is LOCKED, `resolve_oracle_llm_runtime_config`
/// reads the locked vault, fails the keyring read, and DEGRADES to no LLM key (it
/// never panics — see that function's FINDING 4 policy). The respawned server
/// therefore serves retrieval / bounded endpoints normally; only LLM-backed
/// answers degrade to extractive until the next unlock. We deliberately do NOT
/// block the respawn on the vault being unlocked.
///
/// The `stop` flag is re-checked immediately before every publish: a stop/exit may
/// arrive during the blocking `ensure_oracle_server`, and `on_app_exit` deletes the
/// discovery file. Refusing to publish once stopping prevents this thread from
/// resurrecting a stale file after that deletion.
fn reconcile_once(stop: &AtomicBool) {
    let Some(root) = index_root() else {
        return;
    };
    let mid_install = install_in_progress();
    if mid_install {
        // An install STOPS the server and pip-clobbers the venv; do not fight it.
        return;
    }
    // LLM credentials changed via the save command (which never blocks on
    // teardown): tear the resident server down HERE, off the UI thread, so the
    // restart below respawns it with the fresh key. Claim the flag atomically so
    // two briefly-overlapping ticks cannot both kill+respawn; the loser sees it
    // already cleared and does nothing. kill_python_oracle_child does NO network
    // I/O and is safe on this thread. After the kill the server is not ready, so
    // should_restart fires below and respawns it on this same tick.
    if LLM_RESTART_REQUESTED
        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let _ = crate::oracle::python_oracle::kill_python_oracle_child();
    }

    let mut server_ready = oracle_server_ready(&root);
    if should_restart(true, true, mid_install, server_ready) {
        // Cold/dead server: (re)start it. ensure_oracle_server waits for ready and
        // honors `stop` — a superseded/stopping supervisor aborts the (re)spawn and
        // releases the start lock promptly (returns the Aborted error) so two
        // supervisors never spawn two servers (the double-spawn root cause).
        if ensure_oracle_server(&root, stop).is_ok() && !stop.load(Ordering::SeqCst) {
            server_ready = true;
            let _ = publish_discovery(&root);
        }
    } else if server_ready && !stop.load(Ordering::SeqCst) && !discovery_file_present() {
        // Warm server but no discovery file (e.g. the server was started by the
        // operator path, or the file was removed out-of-band): recreate it. Gated
        // on absence so the steady-state tick does not rewrite the file (and churn
        // its `updatedAt`) every interval.
        let _ = publish_discovery(&root);
    }

    // Auto-watch + warm the index, once per app process, while the server is
    // ready. Keeping the index warm means an agent query never lands on a
    // not-ready index. Re-checks `stop` (a stop/exit may have arrived during a
    // blocking start) and `mid_install` is already known false here.
    if !stop.load(Ordering::SeqCst) {
        maybe_start_watcher_and_warm(&root, server_ready);
    }
}

/// Start the file watcher and kick exactly one incremental ("warm") index run,
/// once per session, when the server is ready and `autoWatchOnUnlock` is set.
///
/// Idempotency: gated on the process-wide [`WATCHER_STARTED`] flag (armed once per
/// app process), claimed ATOMICALLY via [`try_claim_watcher_start`] so two
/// briefly-overlapping supervisor ticks cannot both pass and double-POST
/// `/index/watch/start`. The claim is also the only place the flag is set true,
/// and it is reset to `false` if the watch-start HTTP call fails, so a transient
/// failure does NOT permanently disable auto-watch for the session — the next
/// ~10s tick re-claims and retries.
///
/// Non-blocking-by-policy: this runs on the supervisor thread (never the UI),
/// and `/index/run` is dispatched with `background=true` so the server returns
/// immediately and indexing proceeds on its own worker.
fn maybe_start_watcher_and_warm(root: &Path, server_ready: bool) {
    // Cheap gate FIRST: a not-ready server never starts the watcher.
    if !server_ready {
        return;
    }
    // Claim the one-shot BEFORE touching the keyring (FIX 7): a ticking
    // supervisor with the watcher already armed must not read the vault every
    // ~10s. compare_exchange makes the claim single-winner against overlapping
    // ticks; a loser returns here without any keyring or HTTP I/O.
    if !try_claim_watcher_start(&WATCHER_STARTED) {
        return;
    }
    // We hold the claim. A vault read failure must not crash the supervisor;
    // default to NOT auto-watching (fail-safe: an unreadable preference — e.g. a
    // locked vault — is treated as opt-out). If the pref is off, release the claim
    // so a later tick that can read an enabled pref is not blocked, and return.
    let prefs = crate::backend::vault::read_oracle_index_preferences().unwrap_or_else(|_| {
        crate::backend::vault::default_oracle_index_preferences()
    });
    let auto_watch = prefs.auto_watch_on_unlock;
    // We just won the claim, so `already_watching` is false here; the predicate
    // collapses to (server_ready && auto_watch). If it says no (pref off), release
    // the claim so a later tick can re-evaluate, and return.
    if !should_start_watcher(server_ready, auto_watch, false) {
        WATCHER_STARTED.store(false, Ordering::SeqCst);
        return;
    }

    let root_query = format!("root={}", urlencoding::encode(&root.to_string_lossy()));
    // Append `&mode=commit` when the preference asks for git-refs watcher mode;
    // otherwise omit the param so the server uses the default heavy fs watcher.
    let watch_start_url = build_watch_start_url(&root_query, prefs.index_mode.as_deref());
    // Start the watcher (idempotent server-side). On failure, RESET the flag so
    // the next tick retries, and return WITHOUT firing the warm run.
    if run_python_oracle_http_post::<serde_json::Value>(root, &watch_start_url).is_err() {
        WATCHER_STARTED.store(false, Ordering::SeqCst);
        return;
    }
    // Watch-start succeeded: kick ONE FULL run so "open the app -> it indexes
    // everything". `manual=true` makes the route resolve to idle=false +
    // unbounded max_batches (resolve_index_run_params), so this single
    // background job processes ALL pending files, cooling-and-resuming through
    // thermal/low-RAM events (chunk_index cool-and-resume) until it completes,
    // instead of the old opportunistic single ~16-file batch. background=true so
    // the call returns immediately and does not block the supervisor tick; the
    // server's single-job guard (start_background) prevents a second concurrent
    // job. This is one-shot per app process via the WATCHER_STARTED claim above. The
    // watcher's incremental on_batch_ready kicks (max_batches=1) still cover live
    // edits afterward. Its failure is benign — the watcher is already running and
    // the `/ask` path warms on demand — so do NOT reset the flag for the warm run.
    let _ = run_python_oracle_http_post::<serde_json::Value>(
        root,
        &format!("/index/run?{root_query}&force=false&background=true&manual=true"),
    );
}

/// In-flight guard for the on-commit index kick. Set while a kick thread is
/// running; a concurrent commit/pull that finds it already set skips spawning a
/// second redundant kick (the running one already covers the new delta, and the
/// server's own single-job guard would coalesce them anyway). Cleared when the
/// kick thread finishes.
static COMMIT_KICK_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// WARNING 4 (debounce): timestamp of the last kick actually spawned. The in-flight flag
/// only coalesces CONCURRENT commits; a rebase replaying N commits fires N SEQUENTIAL
/// kicks (each a 5s server-ready probe + POST). This debounce skips spawning when the last
/// kick was within `COMMIT_KICK_DEBOUNCE` — the server's incremental run already covers
/// the freshly-replayed commits. Lazily initialized; `None` = never kicked.
static COMMIT_KICK_LAST: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

/// WARNING 4: the debounce window. A rebase replays commits in well under this; a genuine
/// later commit (minutes apart) is past it and kicks normally.
const COMMIT_KICK_DEBOUNCE: Duration = Duration::from_secs(10);

/// WARNING 4 (PURE, unit-testable): should a kick be SKIPPED because the previous one was
/// too recent? `None` (never kicked) -> never debounce (false). Otherwise debounce iff the
/// elapsed time since `last` is strictly less than `window`.
fn should_debounce_commit_kick(
    last: Option<std::time::Instant>,
    now: std::time::Instant,
    window: Duration,
) -> bool {
    match last {
        None => false,
        Some(last) => now.saturating_duration_since(last) < window,
    }
}

/// PURE scope predicate: is the committed project root within the Oracle index
/// root, so that kicking a reindex of `index_root` actually covers the committed
/// code? A commit OUTSIDE `index_root` must NOT trigger a reindex of the wrong
/// tree.
///
/// Canonicalizes both paths so symlinks / `.`/`..` / case differences compare
/// correctly. If canonicalization fails (e.g. the path no longer exists), falls
/// back to a normalized lexical `starts_with` on the raw paths so a transient
/// canonicalize miss never silently drops a legitimate in-scope commit.
/// True if any component of `p` is a `..` (ParentDir) — used to fail-closed the
/// lexical scope fallback below so a `..`-bearing path can't falsely match.
fn has_parent_dir(p: &Path) -> bool {
    p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

pub(crate) fn commit_root_in_index_scope(project_root: &Path, index_root: &Path) -> bool {
    match (project_root.canonicalize(), index_root.canonicalize()) {
        (Ok(p), Ok(i)) => p.starts_with(&i),
        _ => {
            // Canonicalize miss: fall back to a lexical compare on cleaned paths.
            // Fail-closed on `..`: normalize_lexical keeps `..` literal, so a path like
            // `<root>/sub/../../../outside` would falsely `starts_with(<root>)`. Deny it;
            // the next supervisor tick re-evaluates once the path canonicalizes.
            if has_parent_dir(project_root) || has_parent_dir(index_root) {
                return false;
            }
            let p = normalize_lexical(project_root);
            let i = normalize_lexical(index_root);
            p.starts_with(&i)
        }
    }
}

/// Lexical path normalization for the canonicalize-fallback compare: strips
/// `CurDir`/redundant components without touching the filesystem. NOT a security
/// boundary (the canonicalize path is the primary one) — just a best-effort
/// `starts_with` that tolerates a `.` or trailing separator.
fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Called after a successful in-app git commit or pull. If the `index_mode`
/// preference is `"commit"`, the committed project lives within the configured
/// Oracle index root, and the resident server is up, kicks an incremental
/// background index run so Oracle reflects the new commits without waiting for
/// the next supervisor tick or a manual job.
///
/// `project_root` is the root of the project that was just committed/pulled. It
/// is used to skip the kick when the committed tree is OUTSIDE the saved index
/// root (otherwise Oracle would reindex the wrong tree and never the committed
/// code).
///
/// Best-effort: failures are only logged; they MUST NOT propagate to the caller
/// (the git command must succeed regardless of Oracle availability).
///
/// CRITICAL: this returns IMMEDIATELY. The entire body — prefs read, the 5s
/// blocking `oracle_server_ready` probe, the predicate, and the blocking reqwest
/// POST — runs INSIDE the spawned thread, so the Tauri command thread is never
/// blocked (an in-app commit/pull must not hang up to 5s when Oracle is down).
pub fn notify_local_commit(project_root: &Path) {
    if !oracle_is_enabled() {
        return;
    }
    let project_root = project_root.to_path_buf();
    // Coalesce concurrent kicks: if a kick thread is already running, don't
    // spawn another. compare_exchange so exactly one caller wins the slot.
    if COMMIT_KICK_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // WARNING 4 (debounce): the in-flight flag won the slot, but a rebase replaying N
    // commits fires its kicks SEQUENTIALLY (each finishes + clears the flag before the
    // next), so the flag alone never coalesces them. Debounce: if a kick was spawned
    // within the window, skip this one (the server's incremental run already covers the
    // freshly-replayed commits) and RELEASE the slot we just took. On a kick, stamp now.
    {
        let last_cell = COMMIT_KICK_LAST.get_or_init(|| Mutex::new(None));
        let now = std::time::Instant::now();
        let mut last = last_cell.lock().unwrap_or_else(|e| e.into_inner());
        if should_debounce_commit_kick(*last, now, COMMIT_KICK_DEBOUNCE) {
            COMMIT_KICK_IN_FLIGHT.store(false, Ordering::Release);
            return;
        }
        *last = Some(now);
    }
    std::thread::spawn(move || {
        // Always clear the in-flight flag on EVERY exit path of the thread.
        struct ClearGuard;
        impl Drop for ClearGuard {
            fn drop(&mut self) {
                COMMIT_KICK_IN_FLIGHT.store(false, Ordering::Release);
            }
        }
        let _clear = ClearGuard;
        run_commit_index_kick(&project_root);
    });
}

/// The full on-commit index-kick body, run on the detached thread (never the
/// Tauri command thread). Reads prefs, gates on mode + scope + server-ready, and
/// issues the blocking POST. Extracted so [`notify_local_commit`] only spawns
/// (and so this is unit-testable without spawning a thread).
fn run_commit_index_kick(project_root: &Path) {
    let prefs = match crate::backend::vault::read_oracle_index_preferences() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[oracle] notify_local_commit: vault read failed — {e}");
            return;
        }
    };
    let Some(root_str) = prefs.index_root.clone() else {
        // No index root configured — nothing to kick.
        return;
    };
    let root = PathBuf::from(&root_str);
    // Scope gate: if the committed project is OUTSIDE the index root, kicking a
    // reindex of `root` would reindex the wrong tree and never the committed
    // code. Skip with a PATH-ONLY log (no content) and bail.
    if !commit_root_in_index_scope(project_root, &root) {
        eprintln!(
            "[oracle] notify_local_commit: committed project {} is outside index root {} — skipping kick",
            project_root.display(),
            root.display()
        );
        return;
    }
    let server_ready = oracle_server_ready(&root);
    if !should_kick_commit_index(prefs.index_mode.as_deref(), server_ready) {
        return;
    }
    let root_query = format!("root={}", urlencoding::encode(&root.to_string_lossy()));
    let url = format!("/index/run?{root_query}&force=false&background=true");
    if let Err(e) = run_python_oracle_http_post::<serde_json::Value>(&root, &url) {
        eprintln!("[oracle] notify_local_commit: index kick failed (best-effort) — {e}");
    }
}

/// Atomically publish the discovery file with the AGENT token. The temp file is
/// locked owner-only BEFORE the token is written (fail-closed on Windows), then
/// renamed over the target via the shared atomic-replace helper.
///
/// Acquires [`discovery_lock`] for the WHOLE operation and re-checks [`EXITING`]
/// while holding it: if app exit began (so `on_app_exit` set the flag and is about
/// to — or already did — delete the file), this is a no-op. Combined with
/// `on_app_exit` setting the flag before it deletes, this guarantees no publish can
/// resurrect a just-deleted discovery file at shutdown (the publish-vs-delete
/// race). The lock also serializes two briefly-overlapping supervisors writing the
/// same target/backup. NOTE: publishing is NOT suppressed by a vault lock — the
/// discovery file is meant to persist across a lock (always-on agent MCP).
/// The pid to publish in the discovery file. Max-recall finding (2026-07-02):
/// this must be the PYTHON SERVER's pid, not our own — the MCP children
/// liveness-gate the field to skip a dead target, and `std::process::id()`
/// (the app) is alive even when the server child crashed/hung, so the gate
/// watched the wrong process. Falls back to the app pid only when no child is
/// tracked (still a truthful "the supervisor lives" signal, and better than
/// 0/absent for older readers of this file). Extracted so a unit test pins the
/// child-pid linkage against a regression back to the app pid.
fn discovery_pid() -> u32 {
    crate::oracle::python_oracle::oracle_child_pid().unwrap_or_else(std::process::id)
}

fn publish_discovery(root: &Path) -> Result<(), String> {
    let _guard = discovery_lock().lock().unwrap_or_else(|e| e.into_inner());
    if EXITING.load(Ordering::SeqCst) {
        // The app is exiting; `on_app_exit` owns the file now. Refuse to write so
        // we cannot leave a stale AGENT token behind after its delete.
        return Ok(());
    }
    let Some(target) = discovery_path() else {
        // No projects dir recorded ⇒ publishing disabled. Not an error: the
        // operator path still works; only agent auto-discovery is unavailable.
        return Ok(());
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create discovery folder: {e}"))?;
    }

    let (base_url, _port) = oracle_session_endpoint();
    let payload = DiscoveryFile {
        base_url,
        auth_token: oracle_agent_token().to_string(),
        index_root: root.to_string_lossy().to_string(),
        pid: discovery_pid(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    let json = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("Could not serialize discovery file: {e}"))?;

    let temp = write_restricted_temp(target.parent(), &json)?;
    let backup = sibling_with_suffix(&target, ".bak");
    crate::backend::fs_replace::replace_file_with_backup(
        &temp,
        &target,
        &backup,
        "Oracle discovery file",
    )
}

/// Delete the discovery file. Idempotent: a missing file (or no projects dir) is
/// `Ok`. Never logs the path content. Holds [`discovery_lock`] for the whole
/// removal so it cannot interleave with a concurrent [`publish_discovery`] (the
/// publish, under the same lock, will see [`EXITING`] and skip). Only called from
/// the app-exit teardown ([`on_app_exit`]) now.
fn delete_discovery() {
    let _guard = discovery_lock().lock().unwrap_or_else(|e| e.into_inner());
    let Some(target) = discovery_path() else {
        return;
    };
    match std::fs::remove_file(&target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            // Best-effort: a transient failure is non-fatal. The AGENT token is
            // session-scoped and the server is being torn down regardless.
        }
    }
}

/// Append a suffix to a path's filename (`x.json` + `.bak` -> `x.json.bak`).
fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Create an owner-only temp file in `dir` (or the system temp dir) and write
/// `contents` into it only AFTER the restriction is confirmed. Mirrors the
/// agent-launch restricted-write contract: O_EXCL + 0o600 on Unix; create-empty,
/// icacls owner-only + verify, then write on Windows (fail-closed). The caller
/// renames it over the real target.
#[allow(clippy::needless_return)]
fn write_restricted_temp(dir: Option<&Path>, contents: &str) -> Result<PathBuf, String> {
    let mut name_bytes = [0u8; 16];
    getrandom::fill(&mut name_bytes)
        .map_err(|e| format!("Could not generate discovery temp name: {e}"))?;
    let file_name = format!(".oracle-server-{}.tmp", hex::encode(name_bytes));
    let base = dir
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let path = base.join(file_name);

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("Could not create discovery temp file: {e}"))?;
        file.write_all(contents.as_bytes())
            .map_err(|e| format!("Could not write discovery temp file: {e}"))?;
        return Ok(path);
    }

    #[cfg(windows)]
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("Could not create discovery temp file: {e}"))?;

        // FAIL CLOSED: grant ONLY to the resolved current-user SID. We must NOT
        // fall back to `%USERNAME%` — that env var is attacker-controllable, and a
        // value like `Everyone` (or any broad group) would make icacls grant world
        // access to the file holding the AGENT token. If the SID can't be resolved
        // (and validated), delete the temp and error: no discovery file is far
        // safer than a possibly world-readable one. Publishing simply skips this
        // tick; the operator `/ask` path is unaffected.
        let Some(principal) = current_user_sid_string() else {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err("Could not determine current user to lock the discovery file.".into());
        };
        let restricted = restrict_to_user(&path, &principal);
        if !restricted {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err("Could not restrict the discovery file to the current user.".into());
        }
        file.write_all(contents.as_bytes())
            .map_err(|e| format!("Could not write discovery temp file: {e}"))?;
        return Ok(path);
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (contents, &path);
        Err("Restricted discovery files are supported on Windows and Unix only.".into())
    }
}

/// Build the `icacls /grant[:r]` principal argument for `principal` (a Windows
/// SID string like `S-1-5-21-…`).
///
/// CRITICAL: the SID MUST be prefixed with `*`. Without it, `icacls` treats the
/// `S-1-5-…` text as an ACCOUNT NAME, attempts a name→SID lookup, fails with
/// "No mapping between account names and security IDs was done" (exit 52), and the
/// grant does not apply. Since [`write_restricted_temp`] is FAIL-CLOSED, that
/// failure made `publish_discovery` error out (silently, via `let _ =`) so the
/// `.oracle-server.json` discovery file was NEVER written on Windows — every MCP
/// agent then degraded to the in-process fallback. The `*` is what makes icacls
/// bind the grant to the SID directly. Pure + not `cfg(windows)`-gated so the
/// regression is unit-testable on any host.
#[allow(dead_code)] // used only by the Windows `restrict_to_user`; the test refs it on any host
fn icacls_grant_principal(principal: &str) -> String {
    format!("*{principal}:F")
}

#[cfg(windows)]
fn restrict_to_user(path: &Path, principal: &str) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(icacls_grant_principal(principal))
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Best-effort: tighten an ALREADY-EXISTING file to owner-only permissions, reusing
/// the same restricted-permission mechanism the discovery file uses
/// (`write_restricted_temp`): on Windows an `icacls /inheritance:r /grant:r <SID>:F`
/// to the resolved current-user SID, on Unix `chmod 0600`.
///
/// FINDING 5: the oracle-server stdout/stderr logs now run alongside a server whose
/// ENV carries the LLM key; a future stray `os.environ`/traceback dump could land in
/// a world-readable file. Tighten those logs on open.
///
/// Unlike the fail-closed discovery path, this is BEST-EFFORT: it returns `false`
/// (and the caller continues) on any failure, because a slightly-too-open log file
/// must never block server startup. It logs nothing itself (no secrets, no paths) —
/// the caller decides whether to note the failure.
#[allow(dead_code)] // only non-test caller is oracle::python_oracle (app builds)
pub(crate) fn restrict_existing_path_to_owner(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).is_ok()
    }

    #[cfg(windows)]
    {
        let Some(principal) = current_user_sid_string() else {
            return false;
        };
        return restrict_to_user(path, &principal);
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

/// Validate that `candidate` has the strict shape of a Windows SID string:
/// it must start with `S-1-` AND every remaining character must be a digit or
/// `-`. This REJECTS account names and groups (`Everyone`, a `%USERNAME%`
/// value, `Administrators`, …), empty input, and malformed/garbage values
/// (`S-1-x`, BOM-prefixed text) so only a real SID is ever handed to icacls.
///
/// Not `#[cfg(windows)]`-gated: it is pure and lets the unit test run on any host.
/// (`allow(dead_code)` because the only non-test caller is the Windows-only
/// `current_user_sid_string`; the test exercises it on every platform.)
#[allow(dead_code)]
fn validate_sid(candidate: &str) -> bool {
    let Some(rest) = candidate.strip_prefix("S-1-") else {
        return false;
    };
    // A bare `S-1-` is not a usable SID; require at least one more component char.
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// Windows: resolve the current process user's SID (e.g. `S-1-5-21-...`) for an
/// unambiguous, principal-safe icacls grant. Returns `None` (⇒ caller FAILS
/// CLOSED, no fallback) on any subprocess failure or if the parsed value is not a
/// strictly-valid SID — including UTF-16/BOM-mangled output, where the first line
/// will not parse to a valid SID.
#[cfg(windows)]
fn current_user_sid_string() -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .creation_flags(CREATE_NO_WINDOW)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let sid = text
        .lines()
        .next()?
        .rsplit(',')
        .next()?
        .trim()
        // Strip a possible UTF-8 BOM / stray quotes defensively: a UTF-16-LE BOM
        // run through `from_utf8_lossy` yields replacement chars that won't match
        // the strict SID shape, so it fails closed via `validate_sid` below.
        .trim_start_matches('\u{feff}')
        .trim_matches('"')
        .trim()
        .to_string();
    validate_sid(&sid).then_some(sid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::python_oracle::{random_token, stop_python_oracle_runtime};

    /// Serializes the tests that mutate the process-wide `EXITING` / `PROJECTS_DIR`
    /// statics so they don't clobber each other under the parallel test runner.
    static GLOBAL_STATE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Max-recall regression pin: the discovery file must publish the tracked
    /// PYTHON CHILD's pid (the process the MCP liveness gate needs to watch),
    /// falling back to the app's own pid only when no child is tracked. Guards
    /// against a regression back to a bare `std::process::id()`.
    #[test]
    fn discovery_pid_publishes_the_tracked_child_not_the_app() {
        // Spawn a real short-lived child to own a genuine OS pid.
        let child = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "ping -n 30 127.0.0.1 > NUL"])
                .spawn()
        } else {
            std::process::Command::new("sleep").arg("30").spawn()
        }
        .expect("spawn test child");
        let child_pid = child.id();
        assert_ne!(child_pid, std::process::id());

        let previous =
            crate::oracle::python_oracle::swap_oracle_child_for_test(Some(child));
        let published = discovery_pid();
        // Restore the registry BEFORE asserting so a failure cannot leak state,
        // then reap the test child.
        let mut test_child =
            crate::oracle::python_oracle::swap_oracle_child_for_test(previous);
        if let Some(ref mut c) = test_child {
            let _ = c.kill();
            let _ = c.wait();
        }
        assert_eq!(published, child_pid);
    }

    #[test]
    fn validate_sid_accepts_real_sids_and_rejects_names_and_garbage() {
        // FIX 3: only a strictly-shaped SID may ever reach icacls — never an
        // account/group name (attacker-settable via %USERNAME%), empty, or garbage.
        assert!(validate_sid("S-1-5-21-1-2-3"));
        assert!(validate_sid("S-1-5-18")); // LocalSystem
        assert!(validate_sid(
            "S-1-5-21-1234567890-1234567890-1234567890-1001"
        ));

        assert!(!validate_sid("Everyone"));
        assert!(!validate_sid("Administrators"));
        assert!(!validate_sid(""));
        assert!(!validate_sid("S-1-")); // prefix only, no components
        assert!(!validate_sid("S-1-x")); // non-digit component
        assert!(!validate_sid("S-1-5-21-abc"));
        assert!(!validate_sid("\u{feff}S-1-5-18")); // BOM-prefixed garbage
        assert!(!validate_sid("s-1-5-18")); // wrong case
        assert!(!validate_sid("XS-1-5-18"));
    }

    #[test]
    fn icacls_grant_principal_prefixes_the_sid_with_star() {
        // REGRESSION: `icacls /grant:r` needs the SID prefixed with `*`, else it
        // treats `S-1-5-…` as an account NAME, fails the name→SID lookup ("No
        // mapping between account names and security IDs", exit 52), and the grant
        // never applies. Because `write_restricted_temp` is fail-closed, the bare
        // form silently broke discovery-file publishing on every Windows host. This
        // pins the `*` so the bug cannot creep back (and is checkable on any OS,
        // since CI's Unix path uses chmod and never exercises icacls).
        let arg = icacls_grant_principal("S-1-5-21-1234567890-1234567890-1234567890-1001");
        assert!(
            arg.starts_with('*'),
            "icacls SID arg must start with '*': {arg}"
        );
        assert_eq!(arg, "*S-1-5-21-1234567890-1234567890-1234567890-1001:F");
    }

    #[test]
    fn exiting_flag_makes_publish_a_no_op_and_delete_after_publish_leaves_no_file() {
        // FIX 2: closes the publish-vs-delete resurrection race — now gated on the
        // app-EXIT flag (the discovery file persists across a vault lock; only app
        // exit suppresses publishing + deletes the file).
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = std::env::temp_dir().join(format!(
            "aspis-oracle-discovery-race-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        init(dir.clone());
        let target = dir.join(DISCOVERY_FILENAME);

        // (a) EXITING set BEFORE publish ⇒ publish is a no-op, NO file written.
        // The gate is checked under `discovery_lock` BEFORE any temp/restriction
        // work, so this proves the race close without exercising the OS-level
        // icacls/0o600 restriction (environment-dependent; integration-tested in
        // the `#[ignore]` e2e test, mirroring `atomic_replace_round_trip`).
        EXITING.store(true, Ordering::SeqCst);
        publish_discovery(&dir).expect("publish must not error when gated");
        assert!(
            !target.exists(),
            "publish must not write while EXITING (stale-token resurrection)"
        );

        // (b) A file that survived a publish (simulated with a plain write — the
        // restricted write is integration-tested) is removed by `delete_discovery`
        // under the same `discovery_lock`, leaving nothing behind. This is the
        // "delete wins / runs after publish" arm of the race.
        std::fs::write(&target, b"{\"stale\":true}").unwrap();
        assert!(target.exists());
        delete_discovery();
        assert!(
            !target.exists(),
            "delete-after-publish must leave no discovery file"
        );

        // Reset shared state so later tests see a clean slate.
        EXITING.store(false, Ordering::SeqCst);
        *projects_dir_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projects_dir_is_resolvable_once_a_dir_becomes_available() {
        // FIX 4: a None first resolution must not permanently disable publishing —
        // `init` seeds it (and `ensure_projects_dir_resolved` returns it) so a
        // late-available projects dir still enables discovery.
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Start from the "never resolved" state.
        *projects_dir_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;

        let dir = std::env::temp_dir().join(format!(
            "aspis-oracle-discovery-late-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Simulate the projects dir becoming available and being recorded.
        init(dir.clone());
        let resolved = ensure_projects_dir_resolved().expect("must resolve once seeded");
        assert_eq!(resolved, dir);
        assert_eq!(
            discovery_path(),
            Some(dir.join(DISCOVERY_FILENAME)),
            "publishing target must be derived from the resolved projects dir"
        );

        // Reset shared state.
        *projects_dir_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_token_is_nonempty_distinct_and_hex64() {
        let agent = oracle_agent_token();
        // Same RNG/length as the operator token: 32 random bytes hex-encoded = 64
        // lowercase hex chars.
        assert_eq!(agent.len(), 64, "agent token must be 64 hex chars");
        assert!(
            agent.chars().all(|c| c.is_ascii_hexdigit()),
            "agent token must be hex"
        );
        assert!(!agent.is_empty());
        // Distinct from a fresh operator-style token (different draw).
        let operator_like = random_token();
        assert_ne!(agent, operator_like.as_str());
        // Stable across calls (generated once).
        assert_eq!(agent, oracle_agent_token());
    }

    #[test]
    fn discovery_file_serializes_exact_contract_keys_with_agent_token_and_loopback() {
        let (base_url, _port) = oracle_session_endpoint();
        let payload = DiscoveryFile {
            base_url: base_url.clone(),
            auth_token: oracle_agent_token().to_string(),
            index_root: "C:\\Users\\dev\\Workspace".to_string(),
            pid: 4242,
            updated_at: "2026-06-01T12:00:00+00:00".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Exact key set, camelCase.
        let obj = value.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["authToken", "baseUrl", "indexRoot", "pid", "updatedAt"]
        );

        // baseUrl is loopback.
        assert!(
            value["baseUrl"]
                .as_str()
                .unwrap()
                .starts_with("http://127.0.0.1:"),
            "baseUrl must be loopback"
        );

        // authToken is the AGENT token, NEVER the operator token.
        assert_eq!(value["authToken"].as_str().unwrap(), oracle_agent_token());
        assert_ne!(
            value["authToken"].as_str().unwrap(),
            "operator-token-placeholder"
        );

        assert_eq!(
            value["indexRoot"].as_str().unwrap(),
            "C:\\Users\\dev\\Workspace"
        );
        assert_eq!(value["pid"].as_u64().unwrap(), 4242);
        assert_eq!(
            value["updatedAt"].as_str().unwrap(),
            "2026-06-01T12:00:00+00:00"
        );
    }

    #[test]
    fn index_root_uses_the_same_shared_resolver_as_the_operator_path() {
        // FIX 1: the supervisor's workspace root MUST be the very same value the
        // operator `/ask` path resolves, so `oracle_server_ready` matches and the
        // two never fight over the single server/port. Both entrypoints delegate
        // to `commands::current_oracle_index_root`; whatever the vault prefs
        // currently yield, the supervisor seam and the operator seam agree.
        let supervisor = index_root();
        let operator = crate::oracle::commands::current_oracle_index_root().ok();
        assert_eq!(
            supervisor, operator,
            "supervisor index_root() must equal the operator resolver result"
        );
    }

    #[test]
    fn should_restart_is_true_only_for_unlocked_has_root_no_install_not_ready() {
        // Exhaustive 2^4 matrix: exactly one tuple is true.
        for unlocked in [false, true] {
            for has_root in [false, true] {
                for mid_install in [false, true] {
                    for server_ready in [false, true] {
                        let expected = unlocked && has_root && !mid_install && !server_ready;
                        assert_eq!(
                            should_restart(unlocked, has_root, mid_install, server_ready),
                            expected,
                            "({unlocked},{has_root},{mid_install},{server_ready})"
                        );
                    }
                }
            }
        }
        // The one true case, spelled out.
        assert!(should_restart(true, true, false, false));
    }

    #[test]
    fn should_start_watcher_is_true_only_for_ready_pref_and_not_already_watching() {
        // Exhaustive 2^3 matrix: true iff server_ready && auto_watch && !already.
        for server_ready in [false, true] {
            for auto_watch in [false, true] {
                for already in [false, true] {
                    let expected = server_ready && auto_watch && !already;
                    assert_eq!(
                        should_start_watcher(server_ready, auto_watch, already),
                        expected,
                        "({server_ready},{auto_watch},{already})"
                    );
                }
            }
        }
        // The one true case, spelled out: ready + pref on + not yet watching.
        assert!(should_start_watcher(true, true, false));
        // Already-watching short-circuits even when ready + pref on (idempotency).
        assert!(!should_start_watcher(true, true, true));
        // Pref off never starts the watcher, even on a ready server.
        assert!(!should_start_watcher(true, false, false));
        // A not-ready server never starts the watcher (warm-kick is gated too).
        assert!(!should_start_watcher(false, true, false));
    }

    #[test]
    fn try_claim_watcher_start_is_single_winner() {
        // compare_exchange semantics: exactly one caller claims an unset flag;
        // every subsequent caller observes it armed and loses.
        let flag = AtomicBool::new(false);
        assert!(
            try_claim_watcher_start(&flag),
            "first claim on an unset flag must win"
        );
        assert!(flag.load(Ordering::SeqCst), "winner leaves the flag armed");
        assert!(
            !try_claim_watcher_start(&flag),
            "a second claim on an armed flag must lose (no double-start)"
        );
        assert!(
            !try_claim_watcher_start(&flag),
            "every further claim on an armed flag also loses"
        );
    }

    #[test]
    fn watch_start_failure_leaves_flag_retryable_success_leaves_it_armed() {
        // Models the FIX 1 contract that maybe_start_watcher_and_warm encodes:
        // claim → on watch-start Err, reset to false (retryable next tick);
        // on Ok, leave it true (one-shot armed for the session). We exercise the
        // claim + reset directly (the HTTP call needs a live server) to assert the
        // flag transitions without depending on the network.
        //
        // Failure path: claim wins, then a simulated Err resets the flag.
        let flag = AtomicBool::new(false);
        assert!(try_claim_watcher_start(&flag));
        let watch_start_result: Result<(), String> = Err("simulated transient failure".into());
        if watch_start_result.is_err() {
            flag.store(false, Ordering::SeqCst);
        }
        assert!(
            !flag.load(Ordering::SeqCst),
            "a failed watch-start must leave the flag false so the next tick retries"
        );

        // Success path: claim wins, an Ok leaves the flag armed (one-shot).
        let flag = AtomicBool::new(false);
        assert!(try_claim_watcher_start(&flag));
        let watch_start_result: Result<(), String> = Ok(());
        if watch_start_result.is_err() {
            flag.store(false, Ordering::SeqCst);
        }
        assert!(
            flag.load(Ordering::SeqCst),
            "a successful watch-start must leave the flag armed (single warm per session)"
        );
    }

    /// Pure bounded-join semantics: an already-finished thread is joined and reported
    /// `true` immediately; a long-running thread is given up on at the bound and
    /// reported `false` (detached). No real long sleep in either arm.
    #[test]
    fn bounded_join_reaps_finished_and_times_out_on_running() {
        // (a) An immediately-finishing thread is joined within the bound.
        let done = std::thread::spawn(|| {});
        // Give it a moment to actually finish so is_finished() is true on the first poll.
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            bounded_join(done, Duration::from_secs(1)),
            "a finished thread must be joined and reported true"
        );

        // (b) A thread parked past the bound is given up on (false) — the bound caps
        // the wait. We use a stop flag so the thread does not leak past the test.
        let park = Arc::new(AtomicBool::new(false));
        let park_thread = Arc::clone(&park);
        let running = std::thread::spawn(move || {
            while !park_thread.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let started = std::time::Instant::now();
        let joined = bounded_join(running, Duration::from_millis(100));
        let elapsed = started.elapsed();
        assert!(
            !joined,
            "a still-running thread must time out (reported false)"
        );
        assert!(
            elapsed >= Duration::from_millis(100) && elapsed < Duration::from_secs(1),
            "the bound must cap the wait, took {elapsed:?}"
        );
        // Release the parked thread so it exits cleanly.
        park.store(true, Ordering::SeqCst);
    }

    /// Double-spawn fix #2: starting a supervisor after a previous one was left in the
    /// slot must SUPERSEDE it (set its stop flag, bounded-join) and leave EXACTLY ONE
    /// live, non-stopped supervisor — never two concurrently (re)spawning. This is the
    /// stop→start sequence that, before the fix, ran two supervisor threads at once and
    /// double-spawned the resident server.
    #[test]
    fn start_supervisor_after_stop_leaves_a_single_live_supervisor() {
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Clean slate: no supervisor in the slot.
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        // Start the first supervisor and capture its stop handle.
        start_supervisor();
        let first_stop = {
            let slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
            let sup = slot.as_ref().expect("first supervisor must be in the slot");
            assert!(
                !sup.stop.load(Ordering::SeqCst),
                "a freshly started supervisor must not be stopped"
            );
            Arc::clone(&sup.stop)
        };

        // New semantics: `stop_supervisor` (the on_lock path) SIGNALS stop only and
        // LEAVES the supervisor in the slot — it does NOT take() it. So after a stop
        // the slot still holds the (now stop-set) previous supervisor, and the next
        // `start_supervisor` (the re-unlock path) finds it, joins it, and replaces it.
        stop_supervisor();
        {
            let slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
            let sup = slot
                .as_ref()
                .expect("stop_supervisor must LEAVE the supervisor in the slot (signal only)");
            assert!(
                sup.stop.load(Ordering::SeqCst),
                "stop_supervisor must have set the retained supervisor's stop flag"
            );
            assert!(
                Arc::ptr_eq(&first_stop, &sup.stop),
                "stop_supervisor must retain the SAME supervisor, not replace it"
            );
        }

        start_supervisor();

        // The replacement must be a DIFFERENT, live, non-stopped supervisor.
        let second_stop = {
            let slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
            let sup = slot
                .as_ref()
                .expect("replacement supervisor must be in the slot");
            assert!(
                !sup.stop.load(Ordering::SeqCst),
                "the replacement supervisor must be live (not stopped)"
            );
            Arc::clone(&sup.stop)
        };
        assert!(
            !Arc::ptr_eq(&first_stop, &second_stop),
            "the replacement must be a distinct supervisor, not the superseded one"
        );
        // The superseded one stays stopped (it was signalled + bounded-joined).
        assert!(
            first_stop.load(Ordering::SeqCst),
            "the superseded supervisor must remain stopped"
        );

        // Cleanup: signal stop AND take the retained live supervisor out of the slot so
        // no thread leaks past the test (stop_supervisor now only signals + retains).
        stop_supervisor();
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    /// Fix #1 + #3: `start_supervisor` must perform its bounded join OUTSIDE the slot
    /// mutex, so that `on_lock` / `stop_supervisor` (the lock path) can run WITHOUT
    /// stalling on the slot mutex while a join is in progress. We plant a previous
    /// supervisor whose thread parks (does not exit until released) and is stop-set, run
    /// `start_supervisor` on a worker thread (it will sit in `bounded_join` for the full
    /// `SUPERVISOR_JOIN_TIMEOUT`), and assert that `stop_supervisor` — which only needs
    /// the slot mutex briefly — returns near-instantly meanwhile (never blocked for the
    /// join bound). Also asserts the join is genuinely bounded (the parked thread is
    /// given up on) and that exactly one live supervisor remains afterwards.
    #[test]
    fn start_supervisor_join_does_not_hold_the_slot_mutex() {
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Clean slate.
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        // Plant a previous supervisor whose thread PARKS until released, so the bounded
        // join in start_supervisor cannot complete early — it must run the full bound.
        // Its stop flag is pre-set, so start_supervisor classifies it as "supersede".
        let park = Arc::new(AtomicBool::new(false));
        let park_thread = Arc::clone(&park);
        let planted_stop = Arc::new(AtomicBool::new(true));
        let planted_handle = std::thread::Builder::new()
            .name("oracle-supervisor-test-parked".into())
            .spawn(move || {
                while !park_thread.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .expect("spawn parked supervisor thread");
        *supervisor_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(Supervisor {
            stop: Arc::clone(&planted_stop),
            handle: planted_handle,
        });

        // Run start_supervisor on a worker — it will be inside bounded_join (outside the
        // slot lock) for the whole SUPERVISOR_JOIN_TIMEOUT because the parked thread
        // never exits.
        let worker = std::thread::spawn(start_supervisor);

        // Give the worker a beat to take the previous supervisor out and enter the join.
        std::thread::sleep(Duration::from_millis(50));

        // Concurrently exercise the lock path: stop_supervisor only needs the slot mutex
        // briefly and MUST NOT block on the in-flight join. (At this instant the slot is
        // in its brief empty window OR already holds the replacement; either way the
        // call only locks the slot, never joins.)
        let lock_started = std::time::Instant::now();
        stop_supervisor();
        let lock_elapsed = lock_started.elapsed();
        assert!(
            lock_elapsed < Duration::from_millis(500),
            "stop_supervisor must not stall on the slot mutex during a join, took {lock_elapsed:?}"
        );

        // Release the parked thread and let the worker finish (its bounded join either
        // already timed out — proving boundedness — or reaps the now-exiting thread).
        park.store(true, Ordering::SeqCst);
        worker.join().expect("start_supervisor worker must finish");

        // Exactly one supervisor remains in the slot (the replacement).
        {
            let slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
            assert!(
                slot.is_some(),
                "exactly one (replacement) supervisor must remain after the join"
            );
        }

        // Cleanup: signal + take so no thread leaks past the test.
        stop_supervisor();
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    /// Double-spawn fix #2 (responsiveness): on_unlock runs on the unlock command
    /// thread and MUST return promptly — the bounded supervisor join (≤2s) plus the
    /// thread spawn is the only blocking work. With no configured workspace the
    /// supervisor loop is a cheap sleep-loop (reconcile_once early-returns), so this
    /// must complete well under the join bound.
    #[test]
    fn on_unlock_returns_promptly() {
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Clean slate.
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        let started = std::time::Instant::now();
        on_unlock();
        let elapsed = started.elapsed();
        assert!(
            elapsed < SUPERVISOR_JOIN_TIMEOUT + Duration::from_secs(1),
            "on_unlock must not block (bounded join + spawn only), took {elapsed:?}"
        );

        // A second on_unlock (idempotent re-entry) must also return promptly: the
        // existing live, unstopped supervisor is left in place.
        let started = std::time::Instant::now();
        on_unlock();
        assert!(
            started.elapsed() < SUPERVISOR_JOIN_TIMEOUT + Duration::from_secs(1),
            "a re-entrant on_unlock must also return promptly"
        );

        // Cleanup: signal stop AND take the retained supervisor out of the slot
        // (stop_supervisor now only signals + retains) so no thread leaks past the test.
        stop_supervisor();
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        EXITING.store(false, Ordering::SeqCst);
    }

    #[test]
    fn on_lock_does_not_tear_down_the_oracle_server_lifecycle() {
        // LIFECYCLE: a vault lock must NOT touch the Oracle server lifecycle —
        // the supervisor, the discovery file, and the watcher one-shot all survive
        // a lock so agents keep querying and any in-flight index keeps running.
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Clean slate, then plant a live supervisor + an "already watching" flag +
        // a pending LLM restart, and publish a discovery file.
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        EXITING.store(false, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "aspis-oracle-discovery-onlock-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        init(dir.clone());
        let target = dir.join(DISCOVERY_FILENAME);
        std::fs::write(&target, b"{\"live\":true}").unwrap();

        start_supervisor();
        let live_stop = {
            let slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(&slot.as_ref().expect("supervisor planted").stop)
        };
        WATCHER_STARTED.store(true, Ordering::SeqCst);
        LLM_RESTART_REQUESTED.store(true, Ordering::SeqCst);

        on_lock();

        // The supervisor must NOT have been signalled to stop.
        assert!(
            !live_stop.load(Ordering::SeqCst),
            "on_lock must NOT stop the supervisor (server is app-process-scoped)"
        );
        // The discovery file must SURVIVE the lock.
        assert!(
            target.exists(),
            "on_lock must NOT delete the discovery file (agents keep querying)"
        );
        // The watcher one-shot must STAY armed (the watcher survives with its server).
        assert!(
            WATCHER_STARTED.load(Ordering::SeqCst),
            "on_lock must NOT re-arm the watcher one-shot (watcher survives a lock)"
        );
        // A pending LLM restart must STILL apply to the still-running server.
        assert!(
            LLM_RESTART_REQUESTED.load(Ordering::SeqCst),
            "on_lock must NOT clear a pending LLM-restart (server still running)"
        );
        // The EXITING gate must stay clear: a lock is not an exit.
        assert!(
            !EXITING.load(Ordering::SeqCst),
            "on_lock must NOT set the EXITING gate (publishing stays enabled)"
        );

        // Cleanup: stop + take the supervisor, clear flags, remove the temp dir.
        stop_supervisor();
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        WATCHER_STARTED.store(false, Ordering::SeqCst);
        LLM_RESTART_REQUESTED.store(false, Ordering::SeqCst);
        *projects_dir_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `on_app_exit` is the SINGLE teardown point now: it sets the EXITING gate,
    /// stops the supervisor (signal-only), and deletes the discovery file. (The
    /// server-child kill needs a live child and is exercised in the e2e test; here
    /// we assert the no-network state transitions.)
    #[test]
    fn on_app_exit_tears_down_supervisor_and_discovery() {
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Clean slate, plant a live supervisor + a discovery file.
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        EXITING.store(false, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "aspis-oracle-discovery-exit-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        init(dir.clone());
        let target = dir.join(DISCOVERY_FILENAME);
        std::fs::write(&target, b"{\"live\":true}").unwrap();

        start_supervisor();
        let live_stop = {
            let slot = supervisor_slot().lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(&slot.as_ref().expect("supervisor planted").stop)
        };

        on_app_exit();

        // EXITING gate set, supervisor signalled to stop, discovery file deleted.
        assert!(
            EXITING.load(Ordering::SeqCst),
            "on_app_exit must set the EXITING gate (suppress any in-flight publish)"
        );
        assert!(
            live_stop.load(Ordering::SeqCst),
            "on_app_exit must signal the supervisor to stop"
        );
        assert!(
            !target.exists(),
            "on_app_exit must delete the discovery file (no stale AGENT token)"
        );
        // Idempotent: a second call must not panic.
        on_app_exit();

        // Cleanup.
        let _ = supervisor_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        EXITING.store(false, Ordering::SeqCst);
        *projects_dir_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The save-LLM-settings command must NOT block on server teardown. It sets a
    /// lightweight restart-request flag that the supervisor observes; the supervisor
    /// CLAIMS it atomically (single-winner compare_exchange) and clears it, so the
    /// flag is one-shot. `on_lock` no longer clears it (the server survives a lock,
    /// so a pending restart still applies). We exercise the flag transitions
    /// directly (the actual teardown needs a live child).
    #[test]
    fn llm_restart_request_is_set_and_claimed_once_and_survives_lock() {
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Start clean.
        LLM_RESTART_REQUESTED.store(false, Ordering::SeqCst);

        // The save command sets it (non-blocking; returns immediately).
        request_llm_restart();
        assert!(
            LLM_RESTART_REQUESTED.load(Ordering::SeqCst),
            "request_llm_restart must set the flag"
        );

        // The supervisor claims it atomically: exactly one winner.
        let first = LLM_RESTART_REQUESTED
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        let second = LLM_RESTART_REQUESTED
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        assert!(first, "the first supervisor claim must win");
        assert!(!second, "a second claim on the now-cleared flag must lose");

        // A vault lock must NOT clear a pending request (the server keeps running,
        // so the restart should still apply to it).
        request_llm_restart();
        on_lock();
        assert!(
            LLM_RESTART_REQUESTED.load(Ordering::SeqCst),
            "on_lock must NOT clear a pending LLM-restart (server survives a lock)"
        );

        // Reset shared state so later tests see a clean slate.
        LLM_RESTART_REQUESTED.store(false, Ordering::SeqCst);
        EXITING.store(false, Ordering::SeqCst);
    }

    /// Fix 1: a pending LLM-restart request must wake the supervisor's inter-tick
    /// wait EARLY (within ~one slice) instead of after the full ~10s interval, so
    /// the save→respawn-with-new-key latency drops from ~10s to ~1-2s. We use a
    /// LONG interval and a PRE-SET flag so the early break is what returns — if the
    /// flag were ignored the test would block for the (long) interval. No real long
    /// sleep occurs because we break on the first slice.
    #[test]
    fn restart_flag_wakes_wait_early() {
        use std::time::Instant;
        let stop = AtomicBool::new(false);
        let restart = AtomicBool::new(true); // request already pending

        let started = Instant::now();
        let wake = wait_for_next_tick(&stop, &restart, Duration::from_secs(3600));
        let elapsed = started.elapsed();

        assert_eq!(
            wake,
            TickWake::RestartRequested,
            "a pending restart must break the wait early"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "early break must not wait out the interval, took {elapsed:?}"
        );
    }

    /// The stop signal also wins the early break (unchanged behavior), and it takes
    /// precedence over a restart in the same slice (we exit rather than reconcile).
    #[test]
    fn stop_flag_wins_the_wait_and_takes_precedence_over_restart() {
        use std::time::Instant;
        let stop = AtomicBool::new(true);
        let restart = AtomicBool::new(true);

        let started = Instant::now();
        let wake = wait_for_next_tick(&stop, &restart, Duration::from_secs(3600));
        assert_eq!(wake, TickWake::Stop, "stop must win and exit the loop");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "stop must break immediately"
        );
    }

    /// With neither flag set, a SHORT interval elapses normally and reports the
    /// regular tick cadence — proving the loop is not a busy-spin and still ticks.
    #[test]
    fn no_signal_elapses_the_interval_normally() {
        let stop = AtomicBool::new(false);
        let restart = AtomicBool::new(false);
        // Short interval (< one 250ms slice) so this returns promptly without any
        // real long sleep while still exercising the elapsed path.
        let wake = wait_for_next_tick(&stop, &restart, Duration::from_millis(1));
        assert_eq!(wake, TickWake::TickElapsed);
    }

    #[test]
    fn reset_watcher_armed_rearms_without_tearing_down() {
        let _serial = GLOBAL_STATE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Simulate "watcher already started this session" with the app not exiting.
        EXITING.store(false, Ordering::SeqCst);
        WATCHER_STARTED.store(true, Ordering::SeqCst);

        reset_watcher_armed();

        assert!(
            !WATCHER_STARTED.load(Ordering::SeqCst),
            "reset_watcher_armed must re-arm the one-shot so the supervisor can \
             re-start the watcher after a manual stop"
        );
        // FIX 4 invariant: a manual watcher stop must NOT tear anything down — only
        // the watcher one-shot is re-armed; the EXITING gate stays clear.
        assert!(
            !EXITING.load(Ordering::SeqCst),
            "reset_watcher_armed must not flip the EXITING gate (no teardown)"
        );
    }

    #[test]
    fn delete_discovery_is_idempotent_on_missing_file() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-oracle-discovery-del-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join(DISCOVERY_FILENAME);
        assert!(!target.exists());

        // Deleting a missing file must not panic / must be a no-op.
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("unexpected error deleting missing file: {e}"),
        }

        // And the production delete path tolerates a present-then-absent file.
        std::fs::write(&target, b"{}").unwrap();
        std::fs::remove_file(&target).unwrap();
        assert!(!target.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sibling_with_suffix_appends_to_filename() {
        let p = std::path::Path::new("/tmp/dir/.oracle-server.json");
        assert_eq!(
            sibling_with_suffix(p, ".bak").file_name().unwrap(),
            std::ffi::OsStr::new(".oracle-server.json.bak")
        );
    }

    /// Atomic publish round-trip WITHOUT exercising the OS-level restriction
    /// (icacls/0o600), which is environment-dependent and covered by the
    /// `#[ignore]` end-to-end test. This locks the serialization + temp+rename
    /// contract: the published file deserializes to the exact discovery shape.
    #[test]
    fn atomic_replace_round_trip_preserves_discovery_json() {
        let dir = std::env::temp_dir().join(format!(
            "aspis-oracle-discovery-rt-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let payload = DiscoveryFile {
            base_url: "http://127.0.0.1:21000".to_string(),
            auth_token: oracle_agent_token().to_string(),
            index_root: dir.to_string_lossy().to_string(),
            pid: std::process::id(),
            updated_at: "2026-06-01T00:00:00+00:00".to_string(),
        };
        let json = serde_json::to_string_pretty(&payload).unwrap();

        // Plain temp write (the restriction is integration-tested), then the same
        // atomic replace the production publisher uses.
        let temp = dir.join(".oracle-server-test.tmp");
        std::fs::write(&temp, &json).unwrap();
        let target = dir.join(DISCOVERY_FILENAME);
        let backup = sibling_with_suffix(&target, ".bak");
        crate::backend::fs_replace::replace_file_with_backup(
            &temp,
            &target,
            &backup,
            "Oracle discovery file",
        )
        .expect("atomic replace");

        assert!(target.exists());
        assert!(!temp.exists(), "temp should be renamed away");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(value["authToken"].as_str().unwrap(), oracle_agent_token());
        assert!(value["baseUrl"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Heavy, real-process integration test. Requires the bundled `oracle/`
    /// package + a built `oracle-data/` index next to the workspace AND a usable
    /// Oracle venv. Spawns the resident server, asserts readiness, publishes +
    /// reads back the discovery file, kills the child, and asserts the supervisor
    /// predicate would restart.
    ///
    /// Run with:
    ///   cargo test --lib oracle_service::tests::resident_server_end_to_end -- --ignored --nocapture
    #[test]
    #[ignore]
    fn resident_server_end_to_end() {
        let Some(root) = index_root() else {
            eprintln!("skipping: no built Oracle index available");
            return;
        };
        // Publish under a private temp projects dir for the test.
        let dir =
            std::env::temp_dir().join(format!("aspis-oracle-discovery-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        init(dir.clone());

        ensure_oracle_server(&root, &AtomicBool::new(false)).expect("resident server start");
        assert!(oracle_server_ready(&root), "server must be ready");

        publish_discovery(&root).expect("publish discovery");
        let target = dir.join(DISCOVERY_FILENAME);
        let raw = std::fs::read_to_string(&target).expect("read discovery");
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(value["baseUrl"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"));
        assert_eq!(value["authToken"].as_str().unwrap(), oracle_agent_token());

        // Kill the child; the predicate must then say "restart".
        stop_python_oracle_runtime().expect("stop server");
        // Give the OS a moment to drop the port.
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            should_restart(
                true,
                true,
                install_in_progress(),
                oracle_server_ready(&root)
            ),
            "after kill, supervisor should want a restart"
        );

        delete_discovery();
        assert!(!target.exists(), "discovery file must be deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── should_kick_commit_index ──────────────────────────────────────────────

    #[test]
    fn should_kick_commit_index_truth_table() {
        // commit + ready → kick
        assert!(should_kick_commit_index(Some("commit"), true));
        // commit + not ready → do not kick (server unreachable)
        assert!(!should_kick_commit_index(Some("commit"), false));
        // watch mode + ready → do not kick
        assert!(!should_kick_commit_index(Some("watch"), true));
        // None (default) + ready → do not kick
        assert!(!should_kick_commit_index(None, true));
        // None + not ready → do not kick
        assert!(!should_kick_commit_index(None, false));
    }

    // ── WARNING 4: commit-kick debounce ───────────────────────────────────────

    #[test]
    fn should_debounce_commit_kick_truth_table() {
        let window = Duration::from_secs(10);
        let now = std::time::Instant::now();

        // First-ever kick (None) → never debounce (kick).
        assert!(
            !should_debounce_commit_kick(None, now, window),
            "first-ever kick is never debounced"
        );

        // Last kick WITHIN the window → debounce (skip). Simulate a 'last' 2s ago.
        let recent = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
        assert!(
            should_debounce_commit_kick(Some(recent), now, window),
            "a kick 2s ago (within 10s window) is debounced"
        );

        // Last kick PAST the window → do not debounce (kick). 'last' 11s ago.
        let old = now.checked_sub(Duration::from_secs(11)).unwrap_or(now);
        assert!(
            !should_debounce_commit_kick(Some(old), now, window),
            "a kick 11s ago (past 10s window) kicks normally"
        );

        // Exactly at the window boundary → NOT debounced (strict `<`).
        let boundary = now.checked_sub(window).unwrap_or(now);
        assert!(
            !should_debounce_commit_kick(Some(boundary), now, window),
            "exactly at the window boundary kicks (strict less-than)"
        );
    }

    // ── build_watch_start_url ─────────────────────────────────────────────────

    #[test]
    fn build_watch_start_url_appends_mode_commit_only_for_commit_pref() {
        let root_q = "root=%2Fsome%2Fpath";
        // commit mode → &mode=commit appended
        let url = build_watch_start_url(root_q, Some("commit"));
        assert_eq!(url, "/index/watch/start?root=%2Fsome%2Fpath&mode=commit");

        // watch mode → no mode param
        let url = build_watch_start_url(root_q, Some("watch"));
        assert_eq!(url, "/index/watch/start?root=%2Fsome%2Fpath");

        // None (absent) → no mode param
        let url = build_watch_start_url(root_q, None);
        assert_eq!(url, "/index/watch/start?root=%2Fsome%2Fpath");
    }

    #[test]
    fn build_watch_start_url_never_emits_mode_for_unknown_values() {
        // Unknown values must NOT propagate to the Python server; sanitize_oracle_index_preferences
        // coerces them to None, but build_watch_start_url must be safe even if called directly.
        let url = build_watch_start_url("root=x", Some("garbage"));
        assert!(!url.contains("mode="), "garbage mode must not appear in URL: {url}");
    }

    // ── commit_root_in_index_scope (WARNING 5) ────────────────────────────────

    #[test]
    fn commit_root_in_index_scope_truth_table() {
        let tmp = std::env::temp_dir().join(format!(
            "oracle_scope_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let index_root = tmp.join("index");
        let inside = index_root.join("sub").join("proj");
        let outside = tmp.join("other").join("proj");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // inside index_root → true
        assert!(
            commit_root_in_index_scope(&inside, &index_root),
            "a project nested under index_root must be in scope"
        );
        // equal to index_root → true (starts_with is reflexive)
        assert!(
            commit_root_in_index_scope(&index_root, &index_root),
            "index_root itself must be in scope"
        );
        // outside index_root → false
        assert!(
            !commit_root_in_index_scope(&outside, &index_root),
            "a project outside index_root must NOT be in scope"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn commit_root_in_index_scope_canonicalize_miss_falls_back_lexically() {
        // Neither path exists → canonicalize() fails for both → the lexical
        // fallback must still compute the correct starts_with relationship.
        let base = std::env::temp_dir().join("oracle_nonexistent_scope_xyz");
        let index_root = base.join("index");
        let inside = index_root.join("a").join("b");
        let outside = base.join("elsewhere");

        // A leading "./" component must be normalized away by the fallback so it
        // does not break the starts_with compare.
        let inside_with_curdir = Path::new(".").join(&inside);

        assert!(
            commit_root_in_index_scope(&inside, &index_root),
            "in-scope path must match via lexical fallback when canonicalize misses"
        );
        assert!(
            commit_root_in_index_scope(&inside_with_curdir, &Path::new(".").join(&index_root)),
            "a CurDir component must be normalized in the lexical fallback"
        );
        assert!(
            !commit_root_in_index_scope(&outside, &index_root),
            "out-of-scope path must NOT match via lexical fallback"
        );
    }

    // ── COMMIT_KICK_IN_FLIGHT coalescing (NIT 8) ──────────────────────────────

    #[test]
    fn commit_kick_in_flight_guard_coalesces_second_call() {
        // The pure compare_exchange logic the kick uses: the FIRST claimer wins
        // (false→true succeeds); a SECOND claim while the flag is set is a no-op
        // (compare_exchange fails). Use a local flag so the test is isolated from
        // the process-wide static and any concurrent kick.
        let flag = AtomicBool::new(false);

        // First claim wins.
        assert!(
            flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "first kick must claim the in-flight slot"
        );
        // Second claim while set → no-op.
        assert!(
            flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err(),
            "a second kick while one is in flight must be skipped"
        );
        // After the running kick clears the flag, a new kick may claim again.
        flag.store(false, Ordering::Release);
        assert!(
            flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "after the in-flight kick clears the flag a new kick may claim it"
        );
    }

    #[test]
    fn notify_local_commit_returns_immediately() {
        // BLOCKER 2 structural regression: notify_local_commit must NOT run the
        // server_ready probe / blocking POST on the calling thread — it only
        // spawns. We cannot easily inject the probe here, but we CAN assert the
        // call returns far faster than the 5s blocking probe would take. The
        // entire heavy body lives in run_commit_index_kick on the spawned thread.
        let start = std::time::Instant::now();
        // A path guaranteed outside any real index root; even on the spawned
        // thread this short-circuits, but timing is measured on THIS thread.
        notify_local_commit(Path::new("/__oracle_notify_immediate_test__"));
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "notify_local_commit must return immediately (spawn only); took {elapsed:?}"
        );
        // Give the spawned thread a moment to clear the in-flight flag so it does
        // not leak into a subsequent test that inspects the static.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

static ORACLE_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_oracle_enabled_flag(enabled: bool) {
    // Release pairs with the Acquire load so a thread observing `true` also sees
    // every side-effect (config write, etc.) that preceded the store.
    ORACLE_ENABLED.store(enabled, std::sync::atomic::Ordering::Release);
}

pub(crate) fn oracle_is_enabled() -> bool {
    // Acquire pairs with the Release store so we see the latest value and all
    // preceding side-effects.  Relaxed was incorrect — a reader could see a
    // stale `true` after another thread already stored `false`.
    ORACLE_ENABLED.load(std::sync::atomic::Ordering::Acquire)
}

pub fn oracle_enabled_from_value(v: &serde_json::Value) -> bool {
    v.get("oracle").and_then(|o| o.get("enabled")).and_then(|e| e.as_bool()).unwrap_or(true)
}

pub fn read_oracle_enabled(app: &tauri::AppHandle) -> bool {
    let Some(path) = crate::backend::projects::locate_config_path(app) else { return true; };
    let Ok(content) = std::fs::read_to_string(&path) else { return true; };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else { return true; };
    oracle_enabled_from_value(&value)
}

#[tauri::command]
pub fn get_oracle_enabled(app: tauri::AppHandle) -> bool {
    read_oracle_enabled(&app)
}

#[tauri::command]
pub fn set_oracle_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::backend::state::BackendState>,
    enabled: bool,
) -> Result<bool, String> {
    state.ensure_unlocked()?;
    let _lock = crate::backend::projects::config_write_lock().lock().map_err(|e| format!("config write lock poisoned: {e}"))?;
    let path = crate::backend::projects::locate_config_path(&app).ok_or_else(|| "config.json not found".to_string())?;
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) if !path.exists() => "{}".to_string(),
        Err(e) => return Err(format!("Could not read config.json: {e}")),
    };
    let mut value: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
    if !value.is_object() { value = serde_json::json!({}); }
    let obj = value.as_object_mut().unwrap();
    let oracle = obj.entry("oracle").or_insert_with(|| serde_json::json!({}));
    if !oracle.is_object() { *oracle = serde_json::json!({}); }
    oracle.as_object_mut().unwrap().insert("enabled".to_string(), serde_json::json!(enabled));
    let serialized = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    let timestamp = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let suffix = format!("{}-{}", std::process::id(), timestamp);
    let temp = path.with_extension(format!("json.{suffix}.tmp"));
    let backup = path.with_extension(format!("json.{suffix}.bak"));
    std::fs::write(&temp, serialized).map_err(|e| e.to_string())?;
    crate::backend::fs_replace::replace_file_with_backup(&temp, &path, &backup, "config.json")?;
    set_oracle_enabled_flag(enabled);
    Ok(enabled)
}

#[cfg(test)]
mod oracle_toggle_tests {
    use super::oracle_enabled_from_value;
    use serde_json::json;

    #[test]
    fn default_true_when_absent() {
        assert!(oracle_enabled_from_value(&json!({})));
        assert!(oracle_enabled_from_value(&json!({"oracle": {}})));
    }

    #[test]
    fn reads_explicit_bool() {
        assert!(!oracle_enabled_from_value(&json!({"oracle": {"enabled": false}})));
        assert!(oracle_enabled_from_value(&json!({"oracle": {"enabled": true}})));
    }

    /// MUTATES PROCESS-GLOBAL STATE: toggles the `ORACLE_ENABLED` AtomicBool
    /// through true → false → true to prove the flag is runtime-mutable (not
    /// stuck behind OnceLock's set-once semantics). Restores `true` at the end
    /// so other tests in the same binary see the default.
    #[test]
    fn oracle_enabled_flag_is_mutable_at_runtime() {
        super::set_oracle_enabled_flag(true);
        assert!(super::oracle_is_enabled());
        super::set_oracle_enabled_flag(false);
        assert!(!super::oracle_is_enabled());
        super::set_oracle_enabled_flag(true);
        assert!(super::oracle_is_enabled());
    }
}
