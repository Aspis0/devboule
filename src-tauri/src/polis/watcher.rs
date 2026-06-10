//! Polis Map — filesystem watcher (live mode).
//!
//! Watches the scanned project root RECURSIVELY and, on a DEBOUNCED relevant
//! change, re-runs the pure `scanner::generate_city_state` core, re-attaches the
//! real agents (exactly as the `generate_city_state` command does), stores the
//! new shared `CityState`, and EMITS a `polis://city-updated` Tauri event
//! carrying the full new snapshot. The frontend diffs that snapshot and animates
//! the city incrementally — no fragile partial deltas cross the wire.
//!
//! DESIGN (kept simple + robust):
//!   - notify (v6) `RecommendedWatcher` running on a dedicated thread that owns
//!     the receiver. The watcher only REACTS to files the scanner would keep
//!     (`is_relevant_change`, mirroring the scanner's include extensions +
//!     excluded dirs) — `.git`/`node_modules`/`target`/`dist`/`build` churn and
//!     `*.d.ts`/`*.test.*`/`*.spec.*` are ignored, so an editor's save burst on
//!     a real `.ts`/`.rs` triggers exactly one coalesced re-scan.
//!   - MANUAL DEBOUNCE (`DEBOUNCE_MS`): editors emit many events per save. We
//!     coalesce a burst by waiting `DEBOUNCE_MS` of QUIET after the last relevant
//!     event before scanning. Further events that arrive while a scan is queued
//!     simply reset the quiet timer (throttle: scans never pile up — only the
//!     latest matters).
//!   - PURITY/DETERMINISM: the re-scan is the same deterministic core; it reuses
//!     the stable meta-store coords + grid road routing, so the new `CityState`
//!     is consistent with the previous one and the frontend diff is non-jarring.
//!     The watcher only reacts to REAL file changes; it fabricates nothing.
//!   - ROBUSTNESS: a filesystem / scan / emit error logs (`eprintln!`) and
//!     CONTINUES — it never panics the app. Stopping drops the watcher and signals
//!     the thread to exit cleanly (no leak).
//!
//! The pure pieces (`is_relevant_change`, `DebounceState`) are unit-tested
//! without any real fs-watch.

use crate::backend::fs_watch::{self, DebounceState};
use crate::polis::meta_store::normalize_rel_path;
use crate::polis::scanner;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Tauri event name carrying the full new `CityState` snapshot.
pub const CITY_UPDATED_EVENT: &str = "polis://city-updated";

/// Debounce window: wait this long of QUIET after the last relevant fs event
/// before re-scanning, so an editor's multi-event save coalesces to one scan.
/// TUNING KNOB: raise for noisier editors / slower disks; lower for snappier
/// reaction. ~400ms balances responsiveness against burst coalescing.
pub const DEBOUNCE_MS: u64 = 400;

// NOTE: the watcher's excluded-dir set is `scanner::EXCLUDED_DIRS`, referenced
// directly (see `is_relevant_change`) so the scanner's walk filter and the
// watcher's relevance filter can never drift apart. `.git` churn, `node_modules`,
// build outputs and `target` never trigger a re-scan.

// ---------------------------------------------------------------------------
// Pure predicate: is this fs change one the scanner would care about?
// ---------------------------------------------------------------------------

/// `true` if a change at `path` (under the scanned `root`) is RELEVANT — i.e.
/// the scanner would keep this file, so the city could actually change.
///
/// Convenience wrapper over [`is_relevant_change_with`] using the scanner's
/// DEFAULT extension set. Kept for the unit tests / any caller that has no
/// per-workspace override. The live watcher MUST use `is_relevant_change_with`
/// with the active per-workspace set so its relevance filter and the scanner's
/// keep filter never drift (BLOCKER A).
pub fn is_relevant_change(path: &Path, root: &Path) -> bool {
    is_relevant_change_with(path, root, scanner::DEFAULT_KEPT_EXTENSIONS)
}

/// `true` if a change at `path` (under the scanned `root`) is RELEVANT given the
/// ACTIVE per-workspace extension set `allowed` (lowercase, no leading dot).
///
/// Mirrors the scanner's filters so we never re-scan on noise:
///   - any path component equal to an excluded dir -> ignored
///     (`node_modules`/`target`/`.git`/`dist`/`build`/`docs`);
///   - the meta store itself (`.aspis-meta.json`) -> ignored (we WRITE it during
///     every scan; reacting to it would loop);
///   - otherwise the file name must pass `scanner::should_keep_file_with(name,
///     allowed)` — the EXACT predicate the scan uses, so a user-ENABLED
///     non-default type fires live updates and a DISABLED one no longer wastes a
///     re-scan. Critical JSON is always kept; `*.d.ts`/`*.md`/test/spec rejected.
///
/// NOTE: a DELETED file no longer exists on disk, so we judge purely by the
/// path string (component + filename), never by stat — a deletion of a real
/// kept file is correctly relevant.
pub fn is_relevant_change_with(path: &Path, root: &Path, allowed: &[impl AsRef<str>]) -> bool {
    // Location/name screen (SHARED with Censor via `fs_watch::is_excluded_path`):
    // any excluded-dir component, or the meta store we write on every scan (which
    // would self-trigger a rescan loop), is irrelevant. Keeping this half in the
    // shared module means the two watchers can never drift on the exclusion logic.
    if fs_watch::is_excluded_path(
        path,
        root,
        scanner::EXCLUDED_DIRS,
        &[crate::polis::meta_store::META_FILE_NAME],
    ) {
        return false;
    }

    // File name screen for the keep decision.
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };

    // SAME predicate the scanner uses for the active extension set (no drift).
    scanner::should_keep_file_with(name, allowed)
}

// NOTE: the pure debounce state machine (`DebounceState`) now lives in the
// shared `crate::backend::fs_watch` module (imported above) so the Polis and
// Censor watchers cannot drift on the coalescing logic. It is used unchanged
// below (`DebounceState::new(...)` in `run_loop`).

// ---------------------------------------------------------------------------
// Live watcher handle (held in PolisState).
// ---------------------------------------------------------------------------

/// Owns the running watcher + its debounce/scan thread. Dropping it (or calling
/// `stop`) signals the thread to exit, drops the `notify` watcher, and joins the
/// thread — no leak.
pub struct WatchHandle {
    /// The watched root (idempotency: starting on the same root is a no-op).
    root: PathBuf,
    /// Flag the run loop polls; set on stop so it exits its receive loop.
    running: Arc<AtomicBool>,
    /// The ACTIVE per-workspace extension set the relevance filter uses (BLOCKER
    /// A). Loaded from the project's `.aspis-meta.json` at `start_watch` and
    /// shared (`Arc<Mutex<..>>`) with the notify callback. `polis_set_scan_extensions`
    /// refreshes it via [`set_allowed_extensions`] so a runtime change to the
    /// enabled types takes effect WITHOUT restarting the watcher (the frontend
    /// re-runs the scan on the same root, which does NOT re-start the watcher).
    allowed: Arc<std::sync::Mutex<Vec<String>>>,
    /// The dedicated debounce/scan thread.
    thread: Option<JoinHandle<()>>,
}

impl WatchHandle {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Replace the active per-workspace extension set the relevance filter uses.
    /// Called by `polis_set_scan_extensions` when this watcher is running on the
    /// affected root, so the live filter tracks the persisted override without a
    /// watcher restart. Cheap: swaps a small `Vec<String>` under a mutex.
    pub fn set_allowed_extensions(&self, exts: Vec<String>) {
        if let Ok(mut guard) = self.allowed.lock() {
            *guard = exts;
        }
    }

    /// Signal the worker to stop and hand the blocking `join` to a short-lived
    /// DETACHED reaper thread, so NO caller (neither `stop()` nor `Drop`) ever
    /// blocks for the full remaining scan duration.
    ///
    /// WARNING 3 / FIX 5: the worker may be mid-scan (`generate_city_state` is a
    /// single synchronous call we can't abort mid-walk). We SIGNAL stop (the
    /// worker bails before store/emit — see `rescan_and_emit`) and detach the
    /// `join` to a reaper. The worker still terminates cleanly and the reaper
    /// joins it (no detached/leaked worker, no zombie); the caller just doesn't
    /// wait on it. On a (rare) reaper-spawn failure the worker is left to
    /// self-terminate on the stop flag — unjoined but not leaked.
    ///
    /// This is the SINGLE shared teardown path used by BOTH `stop()` and `Drop`
    /// so they can never drift (FIX 5): dropping a handle without calling
    /// `stop()` is now also non-blocking.
    fn signal_and_reap(running: &Arc<AtomicBool>, thread: Option<JoinHandle<()>>) {
        running.store(false, Ordering::SeqCst);
        if let Some(t) = thread {
            // Hand the blocking join to a detached reaper so the caller returns
            // immediately. The worker observes `running` and exits promptly (it
            // bails before store/emit even mid-scan); the reaper then joins it ->
            // cleanly reaped, never leaked. If the reaper can't be spawned (rare:
            // OS thread limit), the worker is simply left unjoined — it still
            // exits on the flag and the OS reaps it; no memory is leaked, we only
            // forgo the explicit join.
            let spawned = std::thread::Builder::new()
                .name("polis-watcher-reaper".into())
                .spawn(move || {
                    // Best-effort; a poisoned/panicked worker must not crash us.
                    let _ = t.join();
                });
            if let Err(e) = spawned {
                eprintln!("polis watcher: reaper spawn failed ({e}); worker left to self-terminate on the stop flag");
            }
        }
    }

    /// Stop the watcher cleanly: signal the loop, then reap the worker thread via
    /// a detached reaper (non-blocking). The `notify` watcher lives INSIDE the
    /// thread, so it is dropped (unsubscribed) when the thread returns.
    pub fn stop(mut self) {
        Self::signal_and_reap(&self.running, self.thread.take());
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // FIX 5: use the SAME non-blocking signal-then-detached-reaper path as
        // `stop()` so dropping a handle without calling `stop()` (e.g. replacing
        // the handle in PolisState on a folder switch) never blocks the dropping
        // thread for the remaining scan duration. The worker exits on the flag
        // and the reaper joins it — no leak, no zombie, no inline join.
        WatchHandle::signal_and_reap(&self.running, self.thread.take());
    }
}

/// Start watching `root` recursively, emitting `polis://city-updated` on the
/// `app` whenever a debounced relevant change is observed. Returns a handle the
/// caller stores in `PolisState`; dropping/stopping it tears everything down.
///
/// `polis` is the shared state cloned for the scan thread so each re-scan writes
/// the new `CityState` into the same `Arc<Mutex<..>>` the commands read.
/// `project_roots` is the real project-id -> root map (for agent re-attach),
/// captured once at start (best-effort; a change there just isn't reflected
/// until the next start — agents are a thin overlay, not the city geometry).
pub fn start_watch(
    app: AppHandle,
    root: PathBuf,
    polis_city: Arc<std::sync::Mutex<crate::polis::model::CityState>>,
    attach: AttachAgents,
) -> Result<WatchHandle, String> {
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = running.clone();
    let thread_root = root.clone();

    // BLOCKER A: load the ACTIVE per-workspace extension set from the project's
    // `.aspis-meta.json` (the same override the scan reads), defaulting to the
    // built-in set when the workspace has no override. Shared with the notify
    // callback so its relevance filter uses the SAME predicate the scan uses,
    // and refreshable at runtime via `WatchHandle::set_allowed_extensions`.
    let allowed = Arc::new(std::sync::Mutex::new(
        crate::polis::meta_store::MetaStore::load(&root)
            .enabled_extensions()
            .cloned()
            .unwrap_or_else(scanner::default_extensions),
    ));

    // The notify event channel. We send only RELEVANT change notifications
    // (a single unit value) across it; coalescing happens in the loop.
    let (tx, rx) = mpsc::channel::<()>();

    // Build the watcher BEFORE moving into the thread so a watch-setup failure is
    // reported synchronously to the command (clear error, no silent dead thread).
    // The watcher's event callback filters to relevant changes and pings `tx`.
    let cb_root = root.clone();
    let cb_allowed = allowed.clone();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // An fs error inside the callback must never panic the app.
            let event = match res {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("polis watcher: fs event error: {e}");
                    return;
                }
            };
            // Snapshot the active extension set once per event (cheap clone of a
            // small Vec) so the relevance check uses the SAME predicate the scan
            // uses, honoring the live per-workspace override (BLOCKER A). A
            // poisoned lock falls back to the default set rather than panicking.
            let allowed_now: Vec<String> = cb_allowed
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| scanner::default_extensions());
            // Only ping when at least one affected path is relevant.
            let relevant = event
                .paths
                .iter()
                .any(|p| is_relevant_change_with(p, &cb_root, &allowed_now));
            if relevant {
                // Ignore send errors: if the receiver is gone the loop has
                // exited and we're shutting down.
                let _ = tx.send(());
            }
        })
        .map_err(|e| format!("Failed to create filesystem watcher: {e}"))?;

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| format!("Failed to watch project root: {e}"))?;

    // The dedicated debounce/scan thread OWNS the watcher (so dropping the thread
    // unsubscribes) and the receiver.
    let thread = std::thread::Builder::new()
        .name("polis-watcher".into())
        .spawn(move || {
            // Keep the watcher alive for the lifetime of the thread.
            let _watcher = watcher;
            run_loop(app, thread_root, rx, thread_running, polis_city, attach);
        })
        .map_err(|e| format!("Failed to spawn watcher thread: {e}"))?;

    Ok(WatchHandle {
        root,
        running,
        allowed,
        thread: Some(thread),
    })
}

/// How the run loop re-attaches real agents after a re-scan.
///
/// LIVE RE-READ (GAP A): the watcher must reflect agents that move/appear/
/// disappear, not only file changes. So instead of reusing a STALE one-time
/// snapshot, each rescan re-reads the real agent live-state FRESH via the same
/// `get_agent_live_state` path the Projects/Agents pages poll. We keep:
///   - `app`        — the `AppHandle` (Send) used to fetch the managed
///                     `BackendState` + re-read the live state / project roots;
///   - `live` / `project_roots` — the snapshot captured at `start_watch`, used
///                     as a best-effort FALLBACK when a fresh read fails (a
///                     transient FS/lock hiccup must never blank the agents).
///
/// A `None` fresh live state with no fallback leaves the map honestly agent-less.
pub struct AttachAgents {
    pub app: AppHandle,
    pub live: Option<crate::backend::model::AgentLiveState>,
    pub project_roots: std::collections::BTreeMap<String, PathBuf>,
}

impl AttachAgents {
    /// Re-read the REAL agent live-state + project-root map fresh for THIS
    /// rescan. Best-effort: a read failure (e.g. unlock lapsed, lock contention)
    /// falls back to the captured snapshot so a transient miss never blanks the
    /// agents and never panics. This is what makes Polis agents LIVE under the
    /// watcher — they reflect the current `.aspis-agents.json`, not a start-time
    /// copy. Reuses the exact `get_agent_live_state` command path (via the
    /// managed `BackendState`) so the watcher and the Agents page agree.
    fn fresh(
        &self,
    ) -> (
        Option<crate::backend::model::AgentLiveState>,
        std::collections::BTreeMap<String, PathBuf>,
    ) {
        use tauri::Manager;
        let backend_state = self.app.state::<crate::backend::state::BackendState>();

        // 1) Fresh agent live-state from the same gated command the UI polls.
        //    On any error (locked, lock contention) fall back to the snapshot.
        let live =
            crate::backend::agents::get_agent_live_state(self.app.clone(), backend_state.clone())
                .ok()
                .or_else(|| self.live.clone());

        // 2) Fresh project-root map; empty (a read failure) falls back too, so
        //    agents don't all drop off-map on a transient project-list miss.
        let project_roots = crate::polis::commands::fresh_project_roots(&self.app);
        let project_roots = if project_roots.is_empty() {
            self.project_roots.clone()
        } else {
            project_roots
        };

        (live, project_roots)
    }
}

/// The debounce + scan + emit loop. Blocks on the notify channel with a timeout
/// sized to the pending debounce deadline, so it wakes exactly when a burst has
/// gone quiet (no busy-poll). Robust: scan/emit failures log and continue.
fn run_loop(
    app: AppHandle,
    root: PathBuf,
    rx: mpsc::Receiver<()>,
    running: Arc<AtomicBool>,
    polis_city: Arc<std::sync::Mutex<crate::polis::model::CityState>>,
    attach: AttachAgents,
) {
    let mut debounce = DebounceState::new(Duration::from_millis(DEBOUNCE_MS));
    // SKIP-IF-UNCHANGED (anti re-emit storm): the content signature of the LAST
    // emitted city. When another process edits the workspace, the watcher fires,
    // we re-scan, and the resulting city is OFTEN byte-identical (the change was in
    // an excluded/irrelevant file, or didn't alter the kept structure). Re-emitting
    // an identical city makes the frontend re-diff 878+ buildings on every event;
    // hundreds of those climb the JS heap to an OOM. We emit ONLY when the city
    // actually changed (timestamp excluded — see `city_signature`).
    let mut last_sig: Option<u64> = None;

    while running.load(Ordering::SeqCst) {
        // Decide how long to block: if a scan is pending, only until the debounce
        // deadline; otherwise a bounded idle timeout so we can observe the stop
        // flag promptly without spinning.
        let timeout = debounce
            .time_until_quiet(Instant::now())
            .unwrap_or_else(|| Duration::from_millis(250));

        match rx.recv_timeout(timeout) {
            Ok(()) => {
                // A relevant change: (re)arm the debounce. Drain any other
                // already-queued pings so a burst collapses immediately.
                debounce.record(Instant::now());
                while rx.try_recv().is_ok() {
                    debounce.record(Instant::now());
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // No new event in the window — fall through to the quiet check.
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Sender dropped (watcher gone): nothing more will arrive. Exit.
                break;
            }
        }

        if !running.load(Ordering::SeqCst) {
            break;
        }

        // If the burst has gone quiet, scan exactly once (coalesced). Any events
        // that arrived during the scan re-arm `debounce` for the next pass, so we
        // always end up reflecting the LATEST state without flooding.
        if debounce.take_if_quiet(Instant::now()) {
            rescan_and_emit(&app, &root, &polis_city, &attach, &running, &mut last_sig);
        }
    }
}

/// Re-run the deterministic scan, re-attach agents, store the snapshot, and emit
/// it. Every fallible step logs + continues; the watcher never panics the app.
fn rescan_and_emit(
    app: &AppHandle,
    root: &Path,
    polis_city: &Arc<std::sync::Mutex<crate::polis::model::CityState>>,
    attach: &AttachAgents,
    running: &Arc<AtomicBool>,
    last_sig: &mut Option<u64>,
) {
    let mut city = match scanner::generate_city_state(root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("polis watcher: re-scan failed: {e}");
            return;
        }
    };

    // WARNING 3: a `stop()` may have arrived DURING the (potentially long) scan
    // above. `generate_city_state` is a single synchronous call we can't abort
    // mid-walk, but once it returns we observe the stop flag BEFORE doing the
    // agent re-attach + store + emit, so a stop is honored promptly and we don't
    // push a stale snapshot for a watcher that's being torn down.
    if !should_publish(running) {
        return;
    }

    // Re-attach real agents from a FRESH read of the live state (GAP A): the
    // watcher must reflect agents moving/appearing/disappearing, not just file
    // changes. `fresh()` re-reads `.aspis-agents.json` + the project roots each
    // rescan, falling back to the captured snapshot on a transient read miss.
    // Best-effort; never fabricates. A None live state leaves the map agent-less.
    let (live, project_roots) = attach.fresh();
    if let Some(ref live) = live {
        scanner::attach_agents(&mut city, live, root, &project_roots);
    }

    // Bug-investigation P3 — re-attach the open-bug investigative-smoke markers on a
    // live file-change re-scan too, so a saved file does NOT wipe the smoke until
    // the next 5s agent poll. Sourced from the live project files; FAIL-OPEN (a
    // read error → empty list → stale markers cleared). Mirrors the agent re-attach.
    let open_bug_suspects = crate::backend::projects::gather_open_bug_suspects(app);
    scanner::attach_suspect_cards(&mut city, &open_bug_suspects);

    // POLIS 5 — re-attach external cloud services from the ALREADY-SYNCED in-memory
    // provider inventory so a live re-scan keeps the harbour in sync (otherwise a
    // file change would emit a city with an empty `externalServices` and the cloud
    // outposts would vanish). PURE + OFFLINE: reads the cached snapshot via the
    // managed `BackendState` (resolved off the `AppHandle`, same posture as
    // `fresh_project_roots`) — NO network call, never blocks. The cache is cleared
    // on lock/idle-expiry, so a locked app honestly yields an empty harbour. Era
    // monuments are preserved by `attach_external_services`. Never fabricates.
    {
        use tauri::Manager;
        let inventories = app
            .state::<crate::backend::state::BackendState>()
            .cached_provider_inventories()
            .unwrap_or_default();
        crate::polis::cloud::attach_external_services(&mut city, &inventories);
    }

    // SKIP-IF-UNCHANGED: with this fully-attached city, compute its content
    // signature (timestamp excluded) and compare to the last one we emitted. If a
    // file-change event produced a city IDENTICAL to what the frontend already
    // shows (the change was in an excluded/irrelevant file, or didn't alter the
    // kept structure/agents), do NOT store or emit — re-emitting an identical city
    // only forces the frontend to re-diff every building, and a storm of those
    // (e.g. another process editing the workspace) climbs the JS heap to an OOM.
    let sig = city_signature(&city);
    if *last_sig == Some(sig) {
        return;
    }
    // NOTE: `last_sig` is advanced only at the emit below (after every
    // `should_publish` guard), so a stop-path bail can't leave it stamped with a
    // signature we never actually emitted.

    // Store as the new shared CityState (so subsequent commands see it).
    match polis_city.lock() {
        Ok(mut guard) => {
            // FIX 1 (a): re-check the stop flag AFTER acquiring the lock and
            // BEFORE writing the shared city. A `stop()` (folder switch / replace)
            // could have fired during the scan above OR while we blocked on this
            // lock. If we are no longer running, this scan is for a stopped/
            // replaced watcher targeting the OLD root — storing it would clobber
            // the shared `polis.city` (which the new root's watcher now owns) and
            // the emit below would push the OLD root's city to a frontend that is
            // showing a DIFFERENT root. Bail without storing or emitting.
            if !should_publish(running) {
                return;
            }
            *guard = city.clone();
        }
        Err(_) => {
            // Poisoned lock: don't crash; just skip storing (still emit so the
            // UI gets the fresh snapshot) — subject to the running re-check below.
            eprintln!("polis watcher: city lock poisoned; emitting without store");
        }
    }

    // FIX 1 (b): re-check the stop flag one last time IMMEDIATELY before the
    // emit. The store dropped the lock; a `stop()` may have raced in between.
    // Emitting the OLD root's snapshot now would repaint a frontend that has
    // already switched to a different root — so a late scan becomes a no-op.
    if !should_publish(running) {
        return;
    }

    // Commit the signature ONLY now, immediately before the emit and after every
    // `should_publish` guard — so we never record a city as "emitted" that a
    // stop-path bail actually suppressed.
    *last_sig = Some(sig);
    // Emit the full snapshot for the frontend to diff. An emit failure (e.g. the
    // window is gone) logs and is ignored.
    if let Err(e) = app.emit(CITY_UPDATED_EVENT, &city) {
        eprintln!("polis watcher: emit {CITY_UPDATED_EVENT} failed: {e}");
    }
}

/// Content signature of a city for the SKIP-IF-UNCHANGED guard (run_loop). EXCLUDES
/// the per-scan `generated_at` timestamp — it changes on every scan and would defeat
/// the guard — by clearing it on a throwaway clone before hashing the serialized
/// form. Cost (~1 MB serialize + hash) is trivial next to the spurious full re-diff
/// of every building a needless re-emit would cause on the frontend.
pub(crate) fn city_signature(city: &crate::polis::model::CityState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut probe = city.clone();
    // Zero out the two PER-SCAN-VOLATILE fields so a re-scan that produced the
    // SAME city is recognized as unchanged:
    //   - `generated_at`: a fresh now() stamp on every scan.
    //   - each building's `last_modified`: the file's mtime — a bare `touch`, a
    //     `git checkout`, or a formatter rewriting identical bytes changes the mtime
    //     without changing the building's structure. Including it here would make the
    //     signature differ on every such event and defeat the skip — i.e. the re-emit
    //     storm / OOM would return whenever a process bumps mtimes (exactly the case:
    //     another agent saving files in the workspace).
    probe.generated_at = String::new();
    for b in probe.buildings.iter_mut() {
        b.last_modified = String::new();
    }
    let bytes = serde_json::to_vec(&probe).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// FIX 1: the load-bearing publish predicate, factored out so the store/emit
/// decision is unit-testable without a real Tauri `AppHandle` or fs-watch.
///
/// A scan's result may be stored into the shared city + emitted to the frontend
/// ONLY while the watcher is still running. Once `stop()` has flipped the flag
/// (folder switch / handle replace), any in-flight scan is for the OLD root and
/// must become a no-op — never clobbering the shared city nor repainting the
/// frontend that has moved to a different root.
#[inline]
fn should_publish(running: &Arc<AtomicBool>) -> bool {
    running.load(Ordering::SeqCst)
}

/// Project-relative, normalized helper kept here for symmetry with the scanner's
/// path handling (used by tests + any future relative-path emit). Currently the
/// predicate works on absolute paths, but this keeps normalization consistent.
#[allow(dead_code)]
fn rel_norm(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    normalize_rel_path(&rel.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\proj" } else { "/proj" })
    }

    fn under(root: &Path, rel: &str) -> PathBuf {
        let mut p = root.to_path_buf();
        for seg in rel.split('/') {
            p.push(seg);
        }
        p
    }

    // ---- is_relevant_change: include real source, exclude noise ----

    #[test]
    fn accepts_real_source_files() {
        let r = root();
        assert!(is_relevant_change(&under(&r, "src/main.tsx"), &r));
        assert!(is_relevant_change(&under(&r, "src/store/cityStore.ts"), &r));
        assert!(is_relevant_change(&under(&r, "src-tauri/src/lib.rs"), &r));
        assert!(is_relevant_change(&under(&r, "Cargo.toml"), &r));
        assert!(is_relevant_change(&under(&r, "package.json"), &r));
        assert!(is_relevant_change(&under(&r, "tsconfig.json"), &r));
    }

    #[test]
    fn rejects_excluded_dirs() {
        let r = root();
        // A real .ts under node_modules / target / .git / dist / build is ignored.
        assert!(!is_relevant_change(
            &under(&r, "node_modules/react/index.ts"),
            &r
        ));
        assert!(!is_relevant_change(
            &under(&r, "src-tauri/target/debug/build/foo.rs"),
            &r
        ));
        assert!(!is_relevant_change(&under(&r, ".git/HEAD"), &r));
        assert!(!is_relevant_change(&under(&r, "dist/assets/app.ts"), &r));
        assert!(!is_relevant_change(&under(&r, "build/out.rs"), &r));
        assert!(!is_relevant_change(&under(&r, "target/x.rs"), &r));
    }

    #[test]
    fn rejects_non_kept_files() {
        let r = root();
        // Declaration / test / spec / markdown / non-critical json: ignored.
        assert!(!is_relevant_change(&under(&r, "src/types/global.d.ts"), &r));
        assert!(!is_relevant_change(&under(&r, "src/foo.test.ts"), &r));
        assert!(!is_relevant_change(&under(&r, "src/foo.spec.tsx"), &r));
        assert!(!is_relevant_change(&under(&r, "README.md"), &r));
        assert!(!is_relevant_change(&under(&r, "data/blob.json"), &r));
        assert!(!is_relevant_change(&under(&r, "image.png"), &r));
        // The meta store we write on every scan must never self-trigger.
        assert!(!is_relevant_change(&under(&r, ".aspis-meta.json"), &r));
    }

    #[test]
    fn deletion_of_real_source_is_relevant_by_path_only() {
        // A deleted file is gone from disk; the predicate judges by path string,
        // so deleting a real .ts is still relevant (drives a demolition).
        let r = root();
        let p = under(&r, "src/deleted/old.ts");
        assert!(is_relevant_change(&p, &r));
    }

    // ---- BLOCKER A: relevance honors the PER-WORKSPACE extension set ----

    /// The watcher must use the SAME active extension set as the scanner, NOT a
    /// hardcoded default. Given an explicit set, a user-ENABLED non-default type
    /// (`.lua` here — not in `DEFAULT_KEPT_EXTENSIONS`) must be relevant, and a
    /// type the user DISABLED (`.rs`, a default) must NOT be — proving the
    /// predicate is driven by `allowed`, not the hardcoded default list.
    #[test]
    fn relevance_honors_per_workspace_extension_set() {
        let r = root();
        // Workspace override: only `.lua` and `.ts` enabled; `.rs` removed.
        let allowed = ["lua".to_string(), "ts".to_string()];

        // Enabled non-default type fires (would NOT under the hardcoded default).
        assert!(
            is_relevant_change_with(&under(&r, "src/init.lua"), &r, &allowed),
            "a user-enabled non-default extension must be relevant"
        );
        // Still-enabled default type fires.
        assert!(is_relevant_change_with(
            &under(&r, "src/app.ts"),
            &r,
            &allowed
        ));
        // A DEFAULT type the user DISABLED must be ignored (no wasted re-scan).
        assert!(
            !is_relevant_change_with(&under(&r, "src-tauri/src/lib.rs"), &r, &allowed),
            "a disabled extension must NOT be relevant even though it is a default"
        );

        // Sanity: the hardcoded default rejects `.lua` and accepts `.rs` — proving
        // the two paths genuinely differ (the override is load-bearing).
        assert!(!is_relevant_change(&under(&r, "src/init.lua"), &r));
        assert!(is_relevant_change(&under(&r, "src-tauri/src/lib.rs"), &r));

        // Excluded dirs + meta store still win regardless of the active set.
        assert!(!is_relevant_change_with(
            &under(&r, "node_modules/x/init.lua"),
            &r,
            &allowed
        ));
        assert!(!is_relevant_change_with(
            &under(&r, ".aspis-meta.json"),
            &r,
            &allowed
        ));
    }

    // ---- DebounceState: coalescing ----

    /// A fake monotonic clock so the debounce logic is tested without sleeping.
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct FakeInstant(u64);
    impl std::ops::Sub for FakeInstant {
        type Output = Duration;
        fn sub(self, rhs: Self) -> Duration {
            Duration::from_millis(self.0.saturating_sub(rhs.0))
        }
    }

    #[test]
    fn debounce_coalesces_a_burst_into_one_scan() {
        let mut d = DebounceState::<FakeInstant>::new(Duration::from_millis(400));
        assert!(!d.pending());

        // Burst of events at t=0,100,200 — all within the window.
        d.record(FakeInstant(0));
        d.record(FakeInstant(100));
        d.record(FakeInstant(200));
        assert!(d.pending());

        // Still inside the window after the last event (t=200): NOT quiet yet.
        assert!(!d.take_if_quiet(FakeInstant(500))); // 500-200=300 < 400
        assert!(d.pending());

        // Window elapsed since the LAST event (t=200 + 400 = 600): scan once.
        assert!(d.take_if_quiet(FakeInstant(600)));
        assert!(!d.pending());

        // A second quiet check does nothing (already taken): no double scan.
        assert!(!d.take_if_quiet(FakeInstant(2000)));
    }

    #[test]
    fn debounce_time_until_quiet_shrinks_then_zeroes() {
        let mut d = DebounceState::<FakeInstant>::new(Duration::from_millis(400));
        assert_eq!(d.time_until_quiet(FakeInstant(0)), None); // nothing pending

        d.record(FakeInstant(0));
        assert_eq!(
            d.time_until_quiet(FakeInstant(100)),
            Some(Duration::from_millis(300))
        );
        assert_eq!(d.time_until_quiet(FakeInstant(400)), Some(Duration::ZERO));
        // Past the window also clamps to zero (never negative).
        assert_eq!(d.time_until_quiet(FakeInstant(9999)), Some(Duration::ZERO));
    }

    #[test]
    fn rel_norm_forward_slashes_under_root() {
        let r = root();
        let p = under(&r, "src/a/b.ts");
        assert_eq!(rel_norm(&r, &p), "src/a/b.ts");
    }

    // ---- FIX 2(a): self-trigger guard (meta write can't loop the watcher) ----

    /// The scan writes `.aspis-meta.json` on EVERY run. If that write were treated
    /// as a relevant change the watcher would re-scan -> re-write -> re-scan
    /// forever. Two INDEPENDENT guards must each reject the meta file:
    ///   1. `is_relevant_change` short-circuits on the meta filename, AND
    ///   2. `scanner::should_keep_file` (the scanner's own filter) rejects it.
    /// We assert BOTH hold for the EXACT path the scan writes
    /// (`MetaStore::path_in(root)`), so neither guard alone is load-bearing — the
    /// self-trigger loop is impossible even if one guard regresses.
    #[test]
    fn meta_write_cannot_self_trigger_rescan() {
        let r = root();
        // The real path the scan writes: <root>/.aspis-meta.json.
        let meta_path = crate::polis::meta_store::MetaStore::path_in(&r);
        assert_eq!(
            meta_path.file_name().and_then(|n| n.to_str()),
            Some(crate::polis::meta_store::META_FILE_NAME),
        );

        // Guard 1: the watcher's relevance filter ignores the meta write.
        assert!(
            !is_relevant_change(&meta_path, &r),
            "writing the meta store must NOT be a relevant change (would self-trigger a rescan loop)"
        );

        // Guard 2: the scanner's own keep-filter also rejects it, independently.
        let name = meta_path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("meta path has a file name");
        assert!(
            !scanner::should_keep_file(name),
            "scanner::should_keep_file must reject the meta file too (second independent guard)"
        );

        // Both guards hold -> a real scan's meta write can never loop the watcher.
    }

    // ---- FIX 1: should_publish gates the store/emit on the running flag ----

    /// The store+emit decision in `rescan_and_emit` is gated by `should_publish`,
    /// re-checked AFTER the lock is acquired and AGAIN before the emit. This test
    /// pins the load-bearing invariant: a scan whose watcher has been stopped
    /// (flag flipped to false — folder switch / handle replace) must NOT publish,
    /// so a late scan for the OLD/replaced root becomes a no-op (never clobbers
    /// the shared city, never repaints a frontend showing a different root).
    #[test]
    fn should_publish_is_false_once_stopped() {
        let running = Arc::new(AtomicBool::new(true));
        // While running: an in-flight scan may store + emit.
        assert!(
            should_publish(&running),
            "a running watcher must be allowed to publish its scan"
        );

        // stop()/Drop flips the flag (folder switch / replace).
        running.store(false, Ordering::SeqCst);

        // Now every post-lock and pre-emit re-check must refuse to publish, so the
        // OLD root's city is neither stored into the shared state nor emitted.
        assert!(
            !should_publish(&running),
            "a stopped/replaced watcher's late scan must NOT store or emit (stale OLD-root city)"
        );
    }

    // ---- FIX 2(b): WatchHandle lifecycle — stop()/Drop signal + join, no leak --
    // (`Arc`, `AtomicBool`, `Ordering`, `Duration`, `Instant` come via `super::*`.)

    /// Build a `WatchHandle` whose worker is a STUB thread (no real notify
    /// watcher, no Tauri `AppHandle` — those need integration infra). The stub
    /// faithfully models the real worker's TERMINATION contract: it polls the same
    /// `running` flag the real `run_loop` polls and returns as soon as it flips to
    /// `false`, setting `exited` on the way out so the test can observe that the
    /// thread body actually ran to completion (not just that `join` returned).
    ///
    /// This lets us prove the load-bearing invariant — `stop()`/`Drop` SIGNAL the
    /// worker (flip `running`) and JOIN it (no leaked/detached thread) — without a
    /// flaky real fs-watch event loop on Windows CI.
    fn stub_handle(exited: Arc<AtomicBool>) -> WatchHandle {
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();
        let thread = std::thread::Builder::new()
            .name("polis-watcher-test-stub".into())
            .spawn(move || {
                // Mirror the real loop: spin on the stop flag with a tiny bounded
                // sleep (never a busy-spin) until signaled to exit.
                while thread_running.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                exited.store(true, Ordering::SeqCst);
            })
            .expect("spawn stub worker");
        WatchHandle {
            root: root(),
            running,
            allowed: Arc::new(std::sync::Mutex::new(scanner::default_extensions())),
            thread: Some(thread),
        }
    }

    #[test]
    fn watch_handle_stop_signals_and_joins() {
        let exited = Arc::new(AtomicBool::new(false));
        let handle = stub_handle(exited.clone());
        // The worker holds a clone of `running`; before stop, strong count is 2.
        let running = handle.running.clone(); // now 3 (test + handle + worker)
        assert!(running.load(Ordering::SeqCst), "worker should be running");
        assert!(
            !exited.load(Ordering::SeqCst),
            "worker should not have exited yet"
        );

        let started = Instant::now();
        handle.stop(); // signals running=false; a DETACHED reaper joins the worker.
                       // `stop()` returns WITHOUT blocking on the join (WARNING 3): the worker is
                       // signaled and terminates promptly, then a detached reaper joins it. We
                       // poll for termination within a bounded window rather than asserting it
                       // synchronously (the whole point is that stop() no longer blocks).
        let mut spun = 0;
        while !exited.load(Ordering::SeqCst) && spun < 500 {
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        assert!(
            exited.load(Ordering::SeqCst),
            "after stop() the worker thread must run to completion (reaped, not leaked)"
        );
        // The worker dropped its `running` clone on exit; once the detached reaper
        // has joined it, only our test clone remains -> strong_count back to 1
        // proves the thread released its Arc (reaped, not detached/leaked).
        let mut spun = 0;
        while Arc::strong_count(&running) != 1 && spun < 500 {
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        assert_eq!(
            Arc::strong_count(&running),
            1,
            "worker's Arc<running> must be dropped -> thread truly terminated and reaped"
        );
        // Sanity: stop() + the worker's prompt self-termination is bounded.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stop() + bounded reap must complete within a bounded time"
        );
    }

    #[test]
    fn watch_handle_drop_signals_and_joins() {
        let exited = Arc::new(AtomicBool::new(false));
        let handle = stub_handle(exited.clone());
        let running = handle.running.clone();

        let started = Instant::now();
        drop(handle); // FIX 5: Drop now uses the SAME non-blocking signal-then-
                      // detached-reaper path as stop() (no inline join), so the
                      // worker terminates promptly via a reaper rather than
                      // blocking the dropping thread. We poll for termination
                      // within a bounded window, exactly like the stop() test.
        let mut spun = 0;
        while !exited.load(Ordering::SeqCst) && spun < 500 {
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        assert!(
            exited.load(Ordering::SeqCst),
            "Drop must signal the worker which then runs to completion (reaped, not leaked)"
        );
        let mut spun = 0;
        while Arc::strong_count(&running) != 1 && spun < 500 {
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        assert_eq!(
            Arc::strong_count(&running),
            1,
            "after Drop the worker's Arc<running> is gone -> thread terminated and reaped"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Drop must signal + bounded-reap within a bounded time"
        );
    }

    // ---- SKIP-IF-UNCHANGED: city_signature ignores the per-scan timestamp ----

    #[test]
    fn city_signature_ignores_generated_at_timestamp() {
        // Two scans of the SAME workspace differ only in `generated_at` (set to
        // now() each scan). The signature MUST treat them as identical, else the
        // skip-if-unchanged guard never fires and the re-emit storm / OOM returns.
        let mut a = crate::polis::model::CityState::empty("proj", "Alpha");
        a.generated_at = "2026-06-10T10:00:00Z".into();
        let mut b = crate::polis::model::CityState::empty("proj", "Alpha");
        b.generated_at = "2026-06-10T10:05:30Z".into(); // later scan, same content
        assert_eq!(
            city_signature(&a),
            city_signature(&b),
            "differing only by generated_at must yield the SAME signature"
        );
    }

    #[test]
    fn city_signature_changes_when_content_changes() {
        // A real structural change (a different project / a new building) MUST
        // change the signature so a genuine edit still emits + diffs.
        let base = crate::polis::model::CityState::empty("proj", "Alpha");
        let other_era = crate::polis::model::CityState::empty("proj", "Beta");
        assert_ne!(
            city_signature(&base),
            city_signature(&other_era),
            "a content change (era) must change the signature"
        );
    }
}
