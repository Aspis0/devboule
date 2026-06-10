//! Censor — Tauri command surface + the managed `CensorState`.
//!
//! All commands are gated by `BackendState::ensure_unlocked()` (they read/write
//! arbitrary local files + spawn linters, exactly the posture of the Polis watch
//! commands and `generate_city_state`).
//!
//! WATCH MODEL: SINGLE-ACTIVE. Censor watches ONE project root at a time — the
//! currently-selected project's working tree. `censor_start_watch` replaces any
//! existing watcher (a different project, or a re-start on the same root is an
//! idempotent no-op). This mirrors the Projects page UX (you work one project at a
//! time) and bounds resource use to a single notify watcher + worker, rather than
//! one per known project. The handle is keyed by `project_id` so the frontend can
//! confirm which project is live. Phase C's board chip uses the lock-free
//! `censor_count_open` read (no watcher required).
//!
//! LIFECYCLE: the state map lock is NEVER held across blocking IO. Commands clone
//! out the root/handle, release the lock, then do the subprocess/shard work
//! (mirrors `agent_pty.rs`). Teardown is the watcher's non-blocking
//! signal-then-detached-reaper (`CensorWatchHandle::stop`/`Drop`), and `lib.rs`
//! reaps the active watcher on `RunEvent::Exit` so quit never orphans a thread or
//! an in-flight tool subprocess.

use super::gemma;
use super::ledger;
use super::orchestrator::{self, GemmaCtx};
use super::schema::{CensorShard, Disposition, Finding};
use super::watch::{self, CensorWatchHandle};
use crate::backend::state::BackendState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

/// BLOCKER B: the error returned by the tool-running commands when the project is
/// not trusted for Censor. A stable, content-free message the frontend can detect
/// to render the "Trust this project to run Censor" prompt.
const CENSOR_UNTRUSTED_MSG: &str =
    "Censor is disabled for this project. Trust the project to run Censor.";

/// Managed Censor state: the single active watch handle (if any). Guarded by its
/// own mutex so start/stop are atomic and the watcher cannot be double-installed.
/// `None` when no project is being watched.
pub struct CensorState {
    watch: Mutex<Option<CensorWatchHandle>>,
    /// IDENTITY-KEYED cache of the Gemma availability probe: `None` = not yet probed
    /// this session; `Some((cache_identity, available))` = the last probe's FULL client
    /// IDENTITY (`client.cache_identity()` — `"{provider}|{base}|{model}"`) and its result.
    /// The probe is a loopback HTTP round-trip — we pay it ONCE per identity (the first
    /// `censor_start_watch` / one-shot for that identity) and reuse the answer for every
    /// fine pass + the UI, rather than re-probing per file. KEYING ON THE FULL IDENTITY
    /// (not just the provider) is what fixes the stale-cache bug: a warm answer must NOT be
    /// reused for a client whose provider OR base OR model differs — changing the oMLX base
    /// (same provider) is a cache MISS and re-probes (a stale answer for the OLD endpoint
    /// would otherwise wrongly enable/disable the tier for the new one). The identity
    /// string is opaque + in-memory only and (unlike the provider/model labels) carries the
    /// base, so it MUST NEVER be logged. The mutex (replacing the old separate `AtomicU8` +
    /// gate) BOTH stores the result AND serializes the one-time probe so concurrent starts
    /// cannot double-probe / double-log (WARNING 1 — probe TOCTOU). It is held only across
    /// the (short, ≤5s) probe; NEVER across watcher install or any shard IO. Phase E reads
    /// it via [`CensorState::gemma_status`] (a brief lock, not on the per-file path).
    gemma_probe: Mutex<Option<(String, bool)>>,
    /// WARNING F: shared "keep running" flag for the DETACHED `censor_review_now`
    /// one-shot fallback (the no-live-watcher case). That worker runs runners + Gemma
    /// off the command thread and was previously untracked, so app exit could leave
    /// it (and an in-flight tool subprocess) running. The worker passes a CLONE of
    /// this `Arc` straight to the orchestrator as its `running` stop-gate (true =
    /// keep going); the orchestrator re-reads it between passes/runners/before emit,
    /// so flipping it to `false` in `kill_all_on_exit` aborts any in-flight one-shot
    /// at the next checkpoint (no orphan thread / no stale event). Shared (not
    /// per-thread) because a single exit signal must abandon ALL in-flight one-shots,
    /// and there are at most a handful at once.
    oneshot_running: Arc<AtomicBool>,
}

impl Default for CensorState {
    fn default() -> Self {
        Self::new()
    }
}

impl CensorState {
    pub fn new() -> Self {
        Self {
            watch: Mutex::new(None),
            gemma_probe: Mutex::new(None),
            oneshot_running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// WARNING F: a clone of the shared "keep running" flag for a detached
    /// `censor_review_now` fallback worker to hand to the orchestrator as its
    /// `running` stop-gate. Flipping it to `false` at exit aborts the worker at its
    /// next checkpoint.
    fn oneshot_running_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.oneshot_running)
    }

    /// WARNING F: signal every in-flight one-shot review to stop (called at app exit).
    fn signal_oneshots_stop(&self) {
        self.oneshot_running.store(false, Ordering::SeqCst);
    }

    /// Resolve the cached Gemma availability for `client`'s provider, probing exactly
    /// ONCE PER PROVIDER (the first call for a provider probes via `client` and stores
    /// `(provider, result)`; later calls for the SAME provider reuse it). On a fresh
    /// probe we log ONCE (not per file). Returns `true` iff the model is available.
    ///
    /// IDENTITY-KEYED (max-recall FIX 4): the cache is keyed on `client.cache_identity()`
    /// — the FULL `"{provider}|{base}|{model}"` identity, not just the provider. A warm
    /// cache for one identity is NOT reused for a client whose provider OR base OR model
    /// differs — any mismatch is a cache MISS and re-probed, then the new identity+result is
    /// stored. This prevents a stale availability (for the OLD endpoint) silently driving
    /// (or disabling) the tier after a base/model change within the SAME provider (the bug
    /// a provider-only key missed). The same-identity fast path still hits the cache (no
    /// re-probe, no re-log). The probe is a cheap loopback metadata read; the mutex is held
    /// only across it, never on the per-file path.
    fn ensure_gemma_probed(&self, client: &dyn gemma::GemmaClient) -> bool {
        let identity = client.cache_identity();
        // Serialize the one-time-per-identity probe so concurrent callers cannot both
        // round-trip + log (WARNING 1). A poisoned lock (a prior panic mid-probe) is
        // recovered — the probe is idempotent and side-effect-free beyond the log, so
        // proceeding is safe.
        let mut cache = self.gemma_probe.lock().unwrap_or_else(|p| p.into_inner());
        // CACHE HIT only when the stored entry is for the SAME full identity. A different
        // provider/base/model is a MISS → fall through and re-probe.
        if let Some((cached_identity, available)) = cache.as_ref() {
            if cached_identity == &identity {
                return *available;
            }
        }
        let available = gemma::probe_available(client);
        // Log ONCE at the probe boundary (never per file, never twice for one identity).
        // Identity only — the PROVIDER identity ("ollama"/"omlx") + the model tag ACTUALLY
        // in use (from the live client, NOT the hardcoded GEMMA_MODEL constant); NEVER the
        // base URL (which IS in `identity` — that string stays in-memory, never logged),
        // file content, or any path (privacy header).
        let provider = client.provider_label();
        let model = client.model_label();
        if available {
            eprintln!("censor gemma: model {model} available (local {provider})");
        } else {
            eprintln!(
                "censor gemma: model {model} unavailable via {provider} — local-AI review tier disabled (deterministic linters still run)"
            );
        }
        *cache = Some((identity, available));
        available
    }

    /// PHASE E read API: the current cached Gemma availability as a tri-state token
    /// the UI can render — `"available"`, `"offline"`, or `"unknown"` (not yet
    /// probed, i.e. no watch has started this session). Exposed so Phase E can show
    /// "Gemma layer offline" without itself probing Ollama. Reflects the MOST RECENT
    /// probe regardless of provider (a brief lock, not on the per-file path).
    #[allow(dead_code)] // first caller is Phase E (the Censor UI "Gemma offline" state).
    pub fn gemma_status(&self) -> &'static str {
        match self
            .gemma_probe
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            Some((_, true)) => "available",
            Some((_, false)) => "offline",
            None => "unknown",
        }
    }

    /// Take the active handle out (used by app-exit teardown), leaving `None`.
    pub fn take_handle(&self) -> Option<CensorWatchHandle> {
        self.watch.lock().ok().and_then(|mut g| g.take())
    }

    /// BLOCKER (trust-revoke): if the active watcher is for `project_id`, take it out
    /// of the slot UNDER THE LOCK (a cheap lock+swap, no IO) and return it so the
    /// caller can run the non-blocking `stop()` reaper OUTSIDE the lock. Returns
    /// `None` (a no-op) when there is no active watcher or it belongs to a DIFFERENT
    /// project, so revoking trust on project Y never disturbs project X's watcher.
    ///
    /// This is what makes `set_censor_trusted(id, false)` IMMEDIATELY inert: the
    /// guard on `censor_start_watch`/`censor_review_now` only blocks FUTURE entries,
    /// but an already-installed watcher keeps spawning repo-controlled
    /// linters/Gemma on every file change until its handle is dropped/stopped — which
    /// this performs atomically the moment trust is withdrawn.
    fn take_handle_if_project(&self, project_id: &str) -> Option<CensorWatchHandle> {
        let mut guard = self.watch.lock().ok()?;
        match guard.as_ref() {
            Some(h) if h.project_id() == project_id => guard.take(),
            _ => None,
        }
    }

    /// If the active watcher is for `project_id` whose `root` matches, enqueue an
    /// on-demand review onto ITS serialized worker queue (so the review runs in
    /// order with the watcher's passes — never concurrently). Returns `true` if it
    /// was enqueued. The send happens UNDER the lock, but it is a non-blocking
    /// channel send (the worker does the IO), so the lock is never held across
    /// blocking IO. Returns `false` if there is no matching live watcher, so the
    /// caller can fall back to a one-shot run.
    fn enqueue_review_if_active(
        &self,
        project_id: &str,
        root: &Path,
        file: Option<String>,
    ) -> bool {
        let guard = match self.watch.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        match guard.as_ref() {
            Some(h) if h.project_id() == project_id && h.root() == root => h.enqueue_review(file),
            _ => false,
        }
    }
}

/// Resolve + validate a project root supplied by the frontend. The root must be an
/// existing directory; everything downstream (shard paths, runner cwd) assumes a
/// real dir.
///
/// CONTRACT: the returned path is CANONICALIZED (`fs::canonicalize`) so a symlinked
/// root resolves to its real target and the watcher/shard paths are consistent
/// across calls (a symlink and its target both reduce to one canonical root, so two
/// `censor_start_watch` calls naming the same tree the two different ways are
/// correctly recognized as the same root by the idempotency check). This is not a
/// security boundary (the user selects their own project), just consistency. If
/// canonicalization fails (e.g. a permission quirk) we fall back to the raw path so
/// a watch still starts rather than hard-failing.
fn resolve_root(root: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(root);
    if !path.is_dir() {
        return Err(format!(
            "Project root is not a directory: {}",
            path.display()
        ));
    }
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

/// BLOCKER 2 + WARNING 3 — confine every censor command to a TRUSTED project root.
///
/// `resolve_root` alone only checks "is a directory", so an authenticated-but-
/// malicious caller (or a webview XSS) could pass `root=C:\Users\<me>` +
/// `file=NTUSER.DAT` and read/open/dispose against arbitrary files. This validator
/// canonicalizes `root` and verifies it equals a configured project `root_path`
/// from the trusted project list (`backend::projects::list_projects`, the SAME
/// resolution `polis::commands::project_root_map` uses), so the censor surface can
/// only ever operate inside a real, declared project tree.
///
/// `expected_project_id`:
///   - `None`  → the root must match SOME project (used by the board's project-less
///     reads: `censor_get_findings` / `censor_count_open` / `censor_status`);
///   - `Some(id)` → the root must match THAT SPECIFIC project's configured root
///     (used by the project-scoped commands: dispose / open / watch / review), so a
///     valid-but-wrong root cannot be paired with a foreign project id.
///
/// Returns the canonical root on success. Both the caller's root and each project's
/// declared root are canonicalized before comparison so a symlink/`.`/`..`-laden
/// but legitimate path still matches. A project whose root fails to canonicalize
/// (e.g. it was deleted) simply does not match.
fn validate_censor_root(
    app: &AppHandle,
    backend_state: &State<'_, BackendState>,
    root: &str,
    expected_project_id: Option<&str>,
) -> Result<PathBuf, String> {
    let canonical = resolve_root(root)?;
    let projects = crate::backend::projects::list_projects(app.clone(), backend_state.clone())
        .map_err(|e| format!("Could not load the project list to validate the Censor root: {e}"))?;
    // (id, declared_root) pairs from the trusted list; the pure matcher does the
    // canonicalize-and-compare so it is unit-testable without a Tauri app/state.
    let declared: Vec<(String, String)> = projects
        .into_iter()
        .filter_map(|p| p.root_path.map(|r| (p.id, r)))
        .collect();
    match_censor_root(&canonical, &declared, expected_project_id)
}

/// Pure core of [`validate_censor_root`]: given the caller's CANONICAL root and the
/// trusted `(project_id, declared_root)` pairs, return the canonical root iff it
/// matches an allowed declared root (canonicalized for comparison). `expected`
/// restricts the match to one project id (mismatch → reject). Factored out so the
/// confinement logic is testable without constructing a Tauri `AppHandle`/`State`.
fn match_censor_root(
    canonical: &Path,
    declared: &[(String, String)],
    expected: Option<&str>,
) -> Result<PathBuf, String> {
    for (id, root) in declared {
        if let Some(expected) = expected {
            if id != expected {
                continue;
            }
        }
        // Canonicalize the declared root the same way `resolve_root` does so the two
        // canonical forms are comparable; skip a declared root that no longer exists.
        let declared_canonical = match std::fs::canonicalize(PathBuf::from(root)) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if declared_canonical == *canonical {
            return Ok(canonical.to_path_buf());
        }
    }
    Err(match expected {
        Some(id) => format!(
            "Censor root does not match the configured root for project {id}; refusing access."
        ),
        None => "Censor root is not a known project root; refusing access.".to_string(),
    })
}

/// Start (or replace) the single active Censor watcher on `root` for `project_id`.
/// Idempotent on the SAME root (no-op). Starting on a DIFFERENT root stops the
/// previous watcher cleanly (non-blocking) and installs the new one.
///
/// CONCURRENCY (BLOCKER 3+4 — TOCTOU): the idempotency decision AND the install are
/// done atomically under the `CensorState` lock. The new watcher is BUILT before
/// taking the lock (notify setup is blocking IO and the lock must never be held
/// across blocking IO), so two concurrent starts may both build a watcher — but the
/// lock then serializes the install so AT MOST ONE handle is ever active:
///   - the loser of an idempotent same-root race stops its just-built watcher;
///   - a replace SIGNALS the outgoing watcher's stop flag UNDER THE LOCK (a cheap
///     atomic store, no IO) BEFORE storing the new handle, so two workers can never
///     run for one project; the detached reap happens outside the lock.
#[tauri::command]
pub fn censor_start_watch(
    project_id: String,
    root: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    censor: State<'_, CensorState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    // Confine to THIS project's configured root (rejects a foreign/arbitrary root).
    let path = validate_censor_root(&app, &backend_state, &root, Some(&project_id))?;

    // BLOCKER B (untrusted-repo tool-config RCE): running Censor executes the
    // project's OWN tool configs from its root (eslint plugins, cargo build scripts
    // via clippy/check, custom semgrep rules). Refuse to install a watcher — i.e.
    // refuse to spawn ANY deterministic runner OR Gemma — for a project the user has
    // not explicitly trusted. The engine stays fully inert until opt-in; the
    // frontend reads `censor_status.trusted` and prompts the user to trust.
    if !crate::backend::projects::project_censor_trusted(&app, &project_id)? {
        eprintln!(
            "censor: project {project_id} is not trusted — watcher NOT started (no runner/Gemma spawn). Trust the project to enable Censor."
        );
        return Err(CENSOR_UNTRUSTED_MSG.to_string());
    }

    // Resolve the local-AI provider config ONCE for this start, then build the client
    // from it (Ollama default OR oMLX). The SAME `local_ai` snapshot is handed to the
    // watcher below so the probe and the worker that follows can never split-brain (probe
    // on one provider, worker on another). `read_censor_local_ai` is fail-safe: a missing
    // or invalid config resolves to the Ollama default (byte-identical to before).
    let local_ai = crate::backend::projects::read_censor_local_ai(&app);

    // Probe the OPTIONAL Gemma tier ONCE (cached in CensorState; reused for every
    // fine pass and read by Phase E). A loopback round-trip — never on the per-file
    // path. If unavailable the tier is disabled and the engine degrades to
    // deterministic-only. Done BEFORE taking the watch lock (it is blocking IO).
    let probe_client = gemma::build_gemma_client(&local_ai);
    let gemma_available = censor.ensure_gemma_probed(&*probe_client);

    // Build the new watcher BEFORE taking the lock (notify setup is blocking IO).
    // A setup failure leaves any existing watcher intact and returns a clear error.
    // The watcher's worker builds its OWN client from the SAME `local_ai` snapshot, so
    // probe + worker agree on one provider/base/model for this session.
    let handle = watch::start_watch(app, project_id, path.clone(), gemma_available, local_ai)?;

    // Atomic check-and-install under the lock. We do NO blocking IO here: the
    // idempotency check, the signal-stop of the outgoing handle, and the store are
    // all in-memory. The (just-built or outgoing) handle to reap is returned and
    // torn down OUTSIDE the lock.
    let to_reap: Option<CensorWatchHandle> = {
        let mut guard = censor
            .watch
            .lock()
            .map_err(|_| "Censor watch state lock poisoned".to_string())?;
        install_handle(&mut guard, handle, path.as_path())
    };
    if let Some(old) = to_reap {
        old.stop();
    }
    Ok(())
}

/// Atomic install decision for `censor_start_watch`, factored out so the
/// one-active-handle invariant is unit-testable without a real watcher. Operates on
/// the locked `Option<CensorWatchHandle>` slot:
///   - if the current handle already watches `new_root` → idempotent no-op: return
///     the just-built `incoming` so the caller reaps it (no second watcher installed);
///   - otherwise install `incoming`, SIGNAL-STOP any previous handle UNDER THE LOCK
///     (so its worker can never overlap the new one — BLOCKER 3+4), and return the
///     previous handle to reap outside the lock.
///
/// In all cases the slot ends holding AT MOST ONE handle, and any outgoing handle is
/// already stop-signaled before this returns.
fn install_handle(
    slot: &mut Option<CensorWatchHandle>,
    incoming: CensorWatchHandle,
    new_root: &Path,
) -> Option<CensorWatchHandle> {
    match slot.as_ref() {
        Some(existing) if existing.root() == new_root => Some(incoming),
        _ => {
            if let Some(prev) = slot.as_ref() {
                prev.signal_stop();
            }
            slot.replace(incoming)
        }
    }
}

/// Stop the active Censor watcher cleanly (non-blocking). Idempotent: stopping when
/// not watching (or when watching a DIFFERENT project) is a successful no-op for
/// the requested `project_id` — we only tear down if the active handle matches.
#[tauri::command]
pub fn censor_stop_watch(project_id: String, censor: State<'_, CensorState>) -> Result<(), String> {
    let handle = {
        let mut guard = censor
            .watch
            .lock()
            .map_err(|_| "Censor watch state lock poisoned".to_string())?;
        match guard.as_ref() {
            // Only stop if the active watcher is THIS project's (avoid a stale
            // stop racing a freshly-started watcher for a different project).
            Some(h) if h.project_id() == project_id => guard.take(),
            _ => None,
        }
    };
    if let Some(h) = handle {
        h.stop();
    }
    Ok(())
}

/// On-demand review pass bypassing debounce. `file = Some(rel)` rechecks one file
/// (its FINE runners); `file = None` runs the whole-project COARSE sweep. Emits
/// `censor://findings-updated`.
///
/// SERIALIZATION (MAJOR race fix): if a live watcher is active for this project +
/// root, the review is ENQUEUED onto that watcher's single serialized worker so it
/// runs in order with the watcher's fine/coarse passes — eliminating the concurrent
/// read-modify-write on shards (and the two-cargo-invocations race) that an inline
/// command-thread review caused. In that case the call returns immediately (the
/// worker does the work off-thread and emits the event when done). If there is NO
/// live watcher for this project, there is no concurrent worker to race, so we run
/// a one-shot pass inline on the command thread (the runners have their own
/// timeouts so it cannot hang indefinitely).
#[tauri::command]
pub fn censor_review_now(
    project_id: String,
    root: String,
    file: Option<String>,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    censor: State<'_, CensorState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    // Confine to THIS project's configured root before any runner/shard IO.
    let path = validate_censor_root(&app, &backend_state, &root, Some(&project_id))?;
    // BLOCKER B: an on-demand review runs the project's OWN tool configs (RCE
    // surface). Refuse for an untrusted project so NO deterministic runner OR Gemma
    // is ever spawned for it. (Reading/disposing existing shards stays allowed —
    // those commands do not run tools.)
    if !crate::backend::projects::project_censor_trusted(&app, &project_id)? {
        return Err(CENSOR_UNTRUSTED_MSG.to_string());
    }
    // Validate a supplied file path (reject traversal / argv-injection) BEFORE any
    // runner sees it.
    if let Some(ref rel) = file {
        ledger::validate_rel_path(rel).map_err(|e| e.to_string())?;
    }
    // Prefer the active watcher's serialized worker (no shard race). Falls back to a
    // one-shot inline run only when no live watcher exists for this project.
    if censor.enqueue_review_if_active(&project_id, &path, file.clone()) {
        return Ok(());
    }
    // No live watcher → no concurrent worker to race. Run the one-shot pass on a
    // DETACHED worker thread and return immediately (WARNING 4): the fallback can take
    // up to probe(5s)+generate(60s)+linter time, which must NEVER block the Tauri
    // command thread. The frontend learns the results via the `censor://findings-
    // updated` event (same as the live-watcher path), so a fire-and-forget run is the
    // correct contract here. Mirrors the watcher's off-thread worker pattern.
    let worker_app = app.clone();
    // WARNING F: hand the detached worker a clone of the shared "keep running" flag
    // so app exit (`kill_all_on_exit`) can flip it and abort the worker between
    // runners/passes rather than leaving an orphan thread + in-flight tool subprocess.
    let running = censor.oneshot_running_flag();
    let spawned = std::thread::Builder::new()
        .name("censor-review-now-oneshot".into())
        .spawn(move || {
            run_review_now_oneshot(&worker_app, project_id, path, file, running);
        });
    if let Err(e) = spawned {
        // A thread-spawn failure is a hard resource error; surface it rather than
        // silently dropping the requested review.
        return Err(format!("Failed to spawn Censor review worker: {e}"));
    }
    Ok(())
}

/// The detached one-shot `censor_review_now` fallback body (no live watcher case).
/// Runs OFF the Tauri command thread (WARNING 4) so a slow Gemma probe/generate +
/// linters never block the IPC caller.
///
/// The Gemma tier uses the process-cached probe via the managed [`CensorState`]
/// (re-resolved from `app` here — we cannot move a `State` borrow across threads):
/// it reuses a prior watch's answer, or probes exactly once (gated) if this is the
/// first Gemma touch this session, so an on-demand recheck gets the same additive
/// Gemma layer as the live watcher's fine passes. If the managed state is somehow
/// absent (teardown race) the tier is simply disabled (deterministic-only).
fn run_review_now_oneshot(
    app: &AppHandle,
    project_id: String,
    path: PathBuf,
    file: Option<String>,
    running: Arc<AtomicBool>,
) {
    use tauri::Manager;
    // Resolve the provider config ONCE and build the client from it (Ollama default OR
    // oMLX). One snapshot per one-shot run = probe and generate use the SAME provider.
    // `read_censor_local_ai` is fail-safe (missing/invalid ⇒ Ollama default).
    let local_ai = crate::backend::projects::read_censor_local_ai(app);
    let gemma_client = gemma::build_gemma_client(&local_ai);
    let gemma_available = match app.try_state::<CensorState>() {
        Some(state) => state.ensure_gemma_probed(&*gemma_client),
        None => false,
    };
    let gemma_ctx = Some(GemmaCtx {
        client: &*gemma_client as &dyn gemma::GemmaClient,
        available: gemma_available,
    });
    // WARNING F: pass the SHARED "keep running" flag straight through as the
    // orchestrator's stop-gate. The orchestrator re-reads it between passes/runners
    // and before each emit, so `kill_all_on_exit` flipping it to `false` aborts an
    // in-flight one-shot at its next checkpoint (no orphan thread, no stale event).
    // `Arc<AtomicBool>` derefs to `&AtomicBool` for the call.
    orchestrator::run_review_now(
        app,
        &project_id,
        &path,
        file.as_deref(),
        gemma_ctx,
        &running,
    );
}

/// Read OPEN findings for the board/panel. `file = Some(rel)` returns that file's
/// shard's open findings; `file = None` returns every shard's open findings across
/// the project. "Open" = `disposition == Open` (judged findings are hidden from the
/// active list but preserved on disk as the audit trail). Lock-free read.
#[tauri::command]
pub fn censor_get_findings(
    root: String,
    file: Option<String>,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<Vec<Finding>, String> {
    backend_state.ensure_unlocked()?;
    // Board read: the root must match SOME trusted project (rejects arbitrary dirs).
    let path = validate_censor_root(&app, &backend_state, &root, None)?;

    let shards: Vec<CensorShard> = match file {
        Some(rel) => {
            ledger::validate_rel_path(&rel).map_err(|e| e.to_string())?;
            ledger::read_shard(&path, &rel)
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect()
        }
        None => ledger::list_shards(&path).map_err(|e| e.to_string())?,
    };

    let mut open: Vec<Finding> = Vec::new();
    for shard in shards {
        for f in shard.findings {
            if f.disposition == Disposition::Open {
                open.push(f);
            }
        }
    }
    Ok(open)
}

/// Cheap count of OPEN findings across the project, for the board chip (Phase C).
/// Lock-free; reads the shard dir and sums open findings. A missing dir → 0.
#[tauri::command]
pub fn censor_count_open(
    root: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<u32, String> {
    backend_state.ensure_unlocked()?;
    // Board chip read: the root must match SOME trusted project.
    let path = validate_censor_root(&app, &backend_state, &root, None)?;
    let shards = ledger::list_shards(&path).map_err(|e| e.to_string())?;
    let count: usize = shards
        .iter()
        .flat_map(|s| s.findings.iter())
        .filter(|f| f.disposition == Disposition::Open)
        .count();
    Ok(count as u32)
}

/// One detected/absent linter for the Censor status payload. `available` reflects
/// a `command_exists` probe of the tool's executable; the UI uses an absent tool
/// to show a "tool not installed — that layer is skipped" hint without implying an
/// error.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CensorToolStatus {
    /// The runner's executable name (e.g. "clippy" surfaces as "cargo", "eslint").
    pub name: String,
    pub available: bool,
}

/// The Censor status payload for the UI: the cached Gemma availability plus the
/// detected/absent deterministic linters relevant to this project's kinds.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CensorStatus {
    /// `"available" | "offline" | "unknown"` from [`CensorState::gemma_status`].
    pub gemma_status: String,
    /// Linters relevant to this project (deduped by executable), each with a
    /// presence flag. Empty when the root is not a recognized project kind.
    pub tools: Vec<CensorToolStatus>,
    /// BLOCKER B: whether the user has trusted this project to RUN Censor. The
    /// panel uses this to show a "Trust this project to run Censor" prompt instead
    /// of (silently) running the repo's tool configs. `false` when no `project_id`
    /// was supplied (a board-level status read) or the project is untrusted.
    pub trusted: bool,
}

/// Pure: the deduped, order-stable list of linter executables relevant to a set of
/// detected project kinds, mapped through `probe` to a presence flag. Factored out
/// so the tool-detection logic is unit-testable without spawning `where.exe`/`sh`.
/// One entry per distinct EXECUTABLE (clippy/cargo-check/cargo-audit all share the
/// `cargo` program, so they collapse to a single `cargo` row).
fn detect_tools_with(
    kinds: &std::collections::HashSet<super::detect::ProjectKind>,
    probe: impl Fn(&str) -> bool,
) -> Vec<CensorToolStatus> {
    use super::detect::FileLang;
    use super::runners::applicable_runners;
    // Union of the runners that could apply across the languages of the detected
    // kinds, plus the cross-cutting set (always present via any lang). We ask for
    // each kind's representative language so kind-specific runners are included.
    let mut programs: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for lang in [FileLang::Rust, FileLang::Ts, FileLang::Py, FileLang::Other] {
        for runner in applicable_runners(kinds, lang) {
            let prog = runner.program().to_string();
            if seen.insert(prog.clone()) {
                programs.push(prog);
            }
        }
    }
    programs
        .into_iter()
        .map(|name| {
            let available = probe(&name);
            CensorToolStatus { name, available }
        })
        .collect()
}

/// UI status read: the cached Gemma availability + which linters are present for
/// this project. Lock-free + cheap (a handful of `command_exists` probes). Used by
/// the Censor panel to render "Gemma layer offline" and optional tool-absent hints.
/// Never starts a watcher or probes Ollama (it reuses the CACHED Gemma tri-state,
/// so before any watch has started this session it is `"unknown"`).
#[tauri::command]
pub fn censor_status(
    root: String,
    project_id: Option<String>,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    censor: State<'_, CensorState>,
) -> Result<CensorStatus, String> {
    backend_state.ensure_unlocked()?;
    // Status read: the root must match SOME trusted project. When a `project_id` is
    // supplied, confine to THAT project's root so the trust flag we report is for
    // the same project the panel is rendering.
    let path = validate_censor_root(&app, &backend_state, &root, project_id.as_deref())?;
    let kinds = super::detect::detect_project_kinds(&path);
    let tools = detect_tools_with(&kinds, |name| {
        crate::backend::projects::command_exists(name)
    });
    // BLOCKER B: surface the trust flag so the panel can prompt to trust rather than
    // run the repo's tool configs. Only resolvable with a project id; a board-level
    // status read (no id) reports `false` (the panel shows the prompt / no run).
    let trusted = match project_id.as_deref() {
        Some(id) => crate::backend::projects::project_censor_trusted(&app, id)?,
        None => false,
    };
    Ok(CensorStatus {
        gemma_status: censor.gemma_status().to_string(),
        tools,
        trusted,
    })
}

/// BLOCKER B: set (or clear) a project's Censor trust flag. Trusting a project
/// authorizes Censor to RUN the project's OWN tool configs from its root (eslint
/// plugins, cargo build scripts via clippy/check, custom semgrep rules) — i.e. to
/// execute repo-controlled code — so it must be an explicit user action. Until set,
/// `censor_start_watch`/`censor_review_now` stay inert. Persisted via the locked
/// project write path; NO-CHURN (the frontmatter line is omitted when false).
///
/// REVOKE-STOPS-THE-WATCHER (adversarial-verify BLOCKER): the entry guards on
/// `censor_start_watch`/`censor_review_now` only block FUTURE invocations. An
/// already-installed watcher would keep spawning the repo's linters/Gemma on every
/// file change after trust is withdrawn. So when this sets `trusted = false`, we
/// ATOMICALLY tear down the active Censor watcher IF it belongs to this project —
/// reusing the same non-blocking signal-then-detached-reaper teardown as
/// `censor_stop_watch` (handle taken under the lock, `stop()` run outside it). The
/// stop lives here in the backend command so EVERY caller of `set_censor_trusted`
/// (any frontend "Untrust" affordance, the MCP surface, tests) is covered.
/// Revoking trust on a DIFFERENT project leaves this project's watcher running.
/// Setting `trusted = true` does NOT auto-start a watch — the frontend explicitly
/// calls `censor_start_watch` after trusting.
#[tauri::command]
pub fn set_censor_trusted(
    project_id: String,
    trusted: bool,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    censor: State<'_, CensorState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    crate::backend::projects::set_project_censor_trusted(&app, &project_id, trusted)?;
    // Revoking trust must make the engine inert IMMEDIATELY: stop any running watcher
    // for THIS project so its worker loop stops spawning runners/Gemma. The handle is
    // taken under the lock; the non-blocking reaper runs outside it (no hang on quit).
    if !trusted {
        if let Some(handle) = censor.take_handle_if_project(&project_id) {
            handle.stop();
        }
    }
    Ok(())
}

/// Set a finding's disposition (e.g. mark a false positive) and append a
/// provenance entry, via the A1 LOCKED write path so a concurrent review pass /
/// the Python MCP writer cannot clobber it. Locates the finding by `id` within the
/// file's shard; if absent, returns an error (the UI passes the file the finding
/// belongs to).
#[tauri::command]
pub fn censor_dispose_finding(
    project_id: String,
    root: String,
    file: String,
    id: String,
    disposition: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    // Confine to THIS project's configured root (rejects a foreign/arbitrary root).
    let path = validate_censor_root(&app, &backend_state, &root, Some(&project_id))?;
    ledger::validate_rel_path(&file).map_err(|e| e.to_string())?;
    let new_disposition = parse_disposition(&disposition)?;

    ledger::dispose_finding(
        &path,
        &file,
        &id,
        new_disposition,
        &project_id,
        &super::now_stamp(),
    )
    .map_err(|e| e.to_string())
}

/// Parse a disposition token from the IPC boundary into the enum, rejecting an
/// unknown value (never silently default — a typo'd disposition must surface).
fn parse_disposition(token: &str) -> Result<Disposition, String> {
    match token {
        "open" => Ok(Disposition::Open),
        "fixed" => Ok(Disposition::Fixed),
        "fp" => Ok(Disposition::Fp),
        "wontfix" => Ok(Disposition::Wontfix),
        other => Err(format!("Unknown disposition: {other}")),
    }
}

/// Open a finding's source file in a chosen editor from the Censor panel's
/// clickable `file:line`. REUSES the Polis editor primitives verbatim: the file is
/// validated to be a real, regular file INSIDE the project root via
/// `polis::commands::resolve_editor_target` (canonicalize root + target, reject
/// `..`/absolute/symlink-escape, containment check) and launched through the same
/// fixed editor allowlist + no-shell `launch_editor`. Unlike Polis (which resolves
/// against the last-scanned map root), this takes the project `root` explicitly so
/// the open works whether or not Polis has scanned this project.
///
/// SECURITY: `resolve_editor_target` is the SAME root-containment guard Polis uses;
/// a `file` outside `root` (traversal / symlink) is rejected before any launch.
#[tauri::command]
pub fn censor_open_in_editor(
    project_id: String,
    root: String,
    file: String,
    editor: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<(), String> {
    // Posture match: launches a process against a local file → require unlock.
    backend_state.ensure_unlocked()?;
    let editor = editor.trim();
    if !crate::polis::commands::is_supported_editor(editor) {
        return Err("Unsupported editor".into());
    }
    // WARNING D: confine to THIS project's configured root (not just SOME project's)
    // before resolving the editor target, so a caller cannot pair a valid root for
    // project A with project B's id and open a file in a foreign project tree. An
    // arbitrary `root` + sibling `file` would otherwise pass `resolve_editor_target`'s
    // own containment check while still being outside the intended project.
    let path = validate_censor_root(&app, &backend_state, &root, Some(&project_id))?;
    // Defense in depth: reject the censor rel-path shapes (argv-injection / `..`)
    // BEFORE the editor-target validation also re-checks containment.
    ledger::validate_rel_path(&file).map_err(|e| e.to_string())?;
    let abs_path = crate::polis::commands::resolve_editor_target(&path, &file)?;
    crate::polis::commands::launch_editor(editor, &abs_path)
}

/// APP-EXIT teardown: reap the active Censor watcher (and its worker) so quit /
/// dev Ctrl-C never orphans the watcher thread or an in-flight tool subprocess.
/// Called from `lib.rs` `RunEvent::Exit`/`ExitRequested` next to the agent_pty +
/// Polis teardown. Idempotent + bounded: `take_handle()` is a cheap lock+swap and
/// `stop()` is the non-blocking signal-then-detached-reaper (no blocking join), so
/// it is safe to run on both ExitRequested and Exit. A missing managed state (e.g.
/// a teardown before setup) is a no-op.
pub fn kill_all_on_exit(app: &AppHandle) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<CensorState>() {
        // WARNING F: signal any in-flight detached one-shot review to stop FIRST, so
        // it aborts at its next orchestrator checkpoint instead of orphaning a thread
        // / tool subprocess past exit. Cheap atomic store; no blocking join (the
        // worker is fire-and-forget and bounded by the runners' own timeouts).
        state.signal_oneshots_stop();
        if let Some(handle) = state.take_handle() {
            handle.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU8;

    #[test]
    fn parse_disposition_accepts_known_rejects_unknown() {
        assert_eq!(parse_disposition("open").unwrap(), Disposition::Open);
        assert_eq!(parse_disposition("fp").unwrap(), Disposition::Fp);
        assert_eq!(parse_disposition("fixed").unwrap(), Disposition::Fixed);
        assert_eq!(parse_disposition("wontfix").unwrap(), Disposition::Wontfix);
        assert!(parse_disposition("bogus").is_err());
        assert!(parse_disposition("").is_err());
    }

    #[test]
    fn censor_state_take_handle_empty_is_none() {
        let st = CensorState::new();
        assert!(st.take_handle().is_none());
    }

    // ---- BLOCKER 2 + WARNING 3: censor root confinement (pure matcher) ----

    /// A real, canonicalizable dir under the temp tree (so `canonicalize` succeeds
    /// for both the caller root and the declared root in the matcher).
    fn real_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aspis-censor-root-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn match_censor_root_rejects_root_not_in_project_list() {
        let outside = real_dir("outside");
        let known = real_dir("known");
        let declared = vec![("p1".to_string(), known.to_string_lossy().into_owned())];
        // A root that is a real dir but NOT a declared project root is refused.
        let err = match_censor_root(&outside, &declared, None).unwrap_err();
        assert!(err.contains("not a known project root"), "got: {err}");
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&known);
    }

    #[test]
    fn match_censor_root_rejects_root_project_id_mismatch() {
        // The root is a valid project root, but it belongs to p1 while the caller
        // claims p2 → reject (a valid-but-wrong root cannot be paired with a foreign
        // project id).
        let root_p1 = real_dir("mismatch-p1");
        let root_p2 = real_dir("mismatch-p2");
        let declared = vec![
            ("p1".to_string(), root_p1.to_string_lossy().into_owned()),
            ("p2".to_string(), root_p2.to_string_lossy().into_owned()),
        ];
        let err = match_censor_root(&root_p1, &declared, Some("p2")).unwrap_err();
        assert!(
            err.contains("does not match the configured root for project p2"),
            "got: {err}"
        );
        let _ = std::fs::remove_dir_all(&root_p1);
        let _ = std::fs::remove_dir_all(&root_p2);
    }

    #[test]
    fn match_censor_root_accepts_valid_project_root() {
        let root = real_dir("valid");
        let declared = vec![("p1".to_string(), root.to_string_lossy().into_owned())];
        // Project-less (board) check: any declared root passes.
        assert_eq!(match_censor_root(&root, &declared, None).unwrap(), root);
        // Project-scoped check: the matching id passes.
        assert_eq!(
            match_censor_root(&root, &declared, Some("p1")).unwrap(),
            root
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn match_censor_root_skips_declared_root_that_no_longer_exists() {
        // A declared root that cannot canonicalize (deleted) is skipped, not matched.
        let real = real_dir("exists");
        let gone = std::env::temp_dir().join(format!("aspis-censor-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&gone);
        let declared = vec![
            ("p-gone".to_string(), gone.to_string_lossy().into_owned()),
            ("p-real".to_string(), real.to_string_lossy().into_owned()),
        ];
        // The caller root matches the REAL project, not the gone one.
        assert_eq!(match_censor_root(&real, &declared, None).unwrap(), real);
        // And the gone project id can never match (its root is unresolvable).
        assert!(match_censor_root(&real, &declared, Some("p-gone")).is_err());
        let _ = std::fs::remove_dir_all(&real);
    }

    #[test]
    fn open_in_editor_root_must_match_its_own_project() {
        // WARNING D: `censor_open_in_editor` now passes `Some(project_id)` to the
        // matcher, so a root that is valid for project A cannot be opened under
        // project B's id. (The matcher is the same one the command uses.)
        let root_a = real_dir("open-a");
        let root_b = real_dir("open-b");
        let declared = vec![
            ("a".to_string(), root_a.to_string_lossy().into_owned()),
            ("b".to_string(), root_b.to_string_lossy().into_owned()),
        ];
        // Project A's root paired with project B's id → rejected.
        assert!(match_censor_root(&root_a, &declared, Some("b")).is_err());
        // Project A's root paired with its OWN id → accepted.
        assert_eq!(
            match_censor_root(&root_a, &declared, Some("a")).unwrap(),
            root_a
        );
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    #[test]
    fn censor_state_signal_oneshots_stop_flips_running_flag() {
        // WARNING F: the shared one-shot flag starts "running" (true) and exit flips
        // it to false so any in-flight detached review aborts at its next checkpoint.
        let st = CensorState::new();
        let flag = st.oneshot_running_flag();
        assert!(flag.load(Ordering::SeqCst), "one-shot flag starts running");
        st.signal_oneshots_stop();
        assert!(
            !flag.load(Ordering::SeqCst),
            "exit must clear the running flag"
        );
    }

    // ---- PHASE E: censor_status tool detection is pure + deduped by executable ----

    #[test]
    fn detect_tools_dedupes_cargo_and_includes_cross_cutting() {
        use super::super::detect::ProjectKind;
        let mut kinds = std::collections::HashSet::new();
        kinds.insert(ProjectKind::Rust);
        // Probe reports everything present so we test the SET + dedup, not presence.
        let tools = detect_tools_with(&kinds, |_| true);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // clippy/cargo-check/cargo-audit all share the `cargo` executable → ONE row.
        assert_eq!(names.iter().filter(|n| **n == "cargo").count(), 1);
        // Cross-cutting tools always present for any kind.
        for cross in ["gitleaks", "jscpd", "lizard", "semgrep"] {
            assert!(names.contains(&cross), "missing cross-cutting tool {cross}");
        }
        assert!(tools.iter().all(|t| t.available));
    }

    #[test]
    fn detect_tools_reflects_absent_probe() {
        use super::super::detect::ProjectKind;
        let mut kinds = std::collections::HashSet::new();
        kinds.insert(ProjectKind::Node);
        // Only `eslint` is "installed"; everything else is absent.
        let tools = detect_tools_with(&kinds, |name| name == "eslint");
        let eslint = tools
            .iter()
            .find(|t| t.name == "eslint")
            .expect("eslint row");
        assert!(eslint.available, "eslint probed present");
        let gitleaks = tools
            .iter()
            .find(|t| t.name == "gitleaks")
            .expect("gitleaks row");
        assert!(!gitleaks.available, "gitleaks probed absent");
        // tsc is a Node runner and must appear (absent here).
        assert!(tools.iter().any(|t| t.name == "tsc" && !t.available));
    }

    #[test]
    fn detect_tools_empty_kinds_is_cross_cutting_only() {
        let kinds = std::collections::HashSet::new();
        let tools = detect_tools_with(&kinds, |_| true);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // No project kind → no kind-specific runners, only the cross-cutting four.
        assert_eq!(names, vec!["gitleaks", "jscpd", "lizard", "semgrep"]);
    }

    // ---- WARNING 1 / N3: the one-time Gemma probe runs ONCE under concurrency ----

    /// A probe-counting stub client (no network). `probe()` increments a shared
    /// counter so the test can assert it ran exactly once even under contention.
    struct CountingProbeClient {
        result: bool,
        probes: std::sync::Arc<AtomicU8>,
    }
    impl gemma::GemmaClient for CountingProbeClient {
        fn probe(&self) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            // A tiny sleep widens the race window so two threads genuinely contend on
            // the gate (without it both might serialize trivially on the OS scheduler).
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.result
        }
        fn generate(&self, _prompt: &str) -> Result<String, gemma::GemmaError> {
            Ok(String::new())
        }
        fn provider_label(&self) -> &'static str {
            "stub"
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
    }

    #[test]
    fn ensure_gemma_probed_probes_exactly_once_under_concurrency() {
        let st = std::sync::Arc::new(CensorState::new());
        let probes = std::sync::Arc::new(AtomicU8::new(0));

        // Spawn several threads that all hit the cold cache simultaneously. Each owns
        // its own client (the real call site builds one per thread too), all sharing
        // the SAME probe counter, so we count TOTAL probe round-trips across threads.
        let mut handles = Vec::new();
        for _ in 0..8 {
            let st = st.clone();
            let probes = probes.clone();
            handles.push(std::thread::spawn(move || {
                let client = CountingProbeClient {
                    result: true,
                    probes,
                };
                st.ensure_gemma_probed(&client)
            }));
        }
        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Exactly ONE probe happened despite 8 concurrent callers (the gate + the
        // double-check serialize it), and every caller agrees on the answer.
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "the probe must run exactly once"
        );
        assert!(
            results.iter().all(|&r| r),
            "all callers see the cached available=true"
        );
        assert_eq!(st.gemma_status(), "available");

        // A subsequent call uses the fast path — still no new probe.
        let client = CountingProbeClient {
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&client));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "a later call hits the cache, no re-probe"
        );
    }

    #[test]
    fn gemma_status_unknown_before_probe_then_reflects_result() {
        let st = CensorState::new();
        assert_eq!(st.gemma_status(), "unknown", "no probe yet → unknown");
        let probes = std::sync::Arc::new(AtomicU8::new(0));
        let client = CountingProbeClient {
            result: false,
            probes,
        };
        assert!(!st.ensure_gemma_probed(&client));
        assert_eq!(
            st.gemma_status(),
            "offline",
            "an unavailable probe → offline"
        );
    }

    /// F2: a probe-counting stub whose `provider_label()` is configurable, so a test can
    /// drive the SAME `CensorState` cache with two different providers and assert it
    /// re-probes on a provider switch (the cache is provider-keyed).
    struct ProviderStub {
        provider: &'static str,
        result: bool,
        probes: std::sync::Arc<AtomicU8>,
    }
    impl gemma::GemmaClient for ProviderStub {
        fn probe(&self) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.result
        }
        fn generate(&self, _prompt: &str) -> Result<String, gemma::GemmaError> {
            Ok(String::new())
        }
        fn provider_label(&self) -> &'static str {
            self.provider
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
        // Fold the provider into the identity (the model is fixed), so this stub still
        // exercises the provider-switch cache miss exactly as before.
        fn cache_identity(&self) -> String {
            format!("{}|stub-base|stub-model", self.provider)
        }
    }

    /// max-recall FIX 4: a probe-counting stub whose `cache_identity()` varies by BASE
    /// (same provider + model), so a test can prove the cache re-probes when only the
    /// oMLX base changes — the bug a provider-only key missed.
    struct BaseStub {
        base: &'static str,
        result: bool,
        probes: std::sync::Arc<AtomicU8>,
    }
    impl gemma::GemmaClient for BaseStub {
        fn probe(&self) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.result
        }
        fn generate(&self, _prompt: &str) -> Result<String, gemma::GemmaError> {
            Ok(String::new())
        }
        fn provider_label(&self) -> &'static str {
            "omlx"
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
        fn cache_identity(&self) -> String {
            format!("omlx|{}|stub-model", self.base)
        }
    }

    #[test]
    fn ensure_gemma_probed_reprobes_on_base_change_same_provider() {
        // max-recall FIX 4 (stale-cache fix): the cache is keyed on the FULL identity, so
        // changing ONLY the oMLX base (same provider + model) is a cache MISS and re-probes
        // — a stale answer for the OLD base must NOT drive the tier for the NEW base.
        let st = CensorState::new();
        let probes = std::sync::Arc::new(AtomicU8::new(0));

        // 1) Warm the cache for base A (probe #1, available).
        let a = BaseStub {
            base: "http://localhost:8000/v1",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&a));
        assert_eq!(probes.load(Ordering::SeqCst), 1, "base A probed once");

        // 2) A second call on the SAME base hits the cache — NO new probe.
        let a2 = BaseStub {
            base: "http://localhost:8000/v1",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&a2));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "same identity reuses the cache (no re-probe)"
        );

        // 3) Change ONLY the base (same provider+model): must re-probe (#2). Base B is
        //    DOWN (result=false) — a stale reuse of base A's available=true would be the
        //    exact bug (tier driven against a different, down endpoint).
        let b = BaseStub {
            base: "http://127.0.0.1:9000/v1",
            result: false,
            probes: probes.clone(),
        };
        assert!(
            !st.ensure_gemma_probed(&b),
            "a base change must re-probe, not reuse base A's answer"
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "a base change (same provider) re-probes (cache miss on identity mismatch)"
        );
        assert_eq!(st.gemma_status(), "offline", "status reflects the latest (base B) probe");
    }

    #[test]
    fn ensure_gemma_probed_is_provider_keyed_reprobes_on_switch() {
        // F2 (stale-cache fix): a warm cache for one provider must NOT be reused by a
        // client of a DIFFERENT provider; a provider switch re-probes (and stores the new
        // provider's answer), while a SAME-provider call still hits the cache.
        let st = CensorState::new();
        let probes = std::sync::Arc::new(AtomicU8::new(0));

        // 1) Warm the cache for ollama (probe #1).
        let ollama = ProviderStub {
            provider: "ollama",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&ollama));
        assert_eq!(probes.load(Ordering::SeqCst), 1, "ollama probed once");

        // 2) A second ollama call hits the cache — NO new probe.
        let ollama2 = ProviderStub {
            provider: "ollama",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&ollama2));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "same-provider call reuses the cache (no re-probe)"
        );

        // 3) Switch to omlx: the warm ollama answer must NOT be reused — re-probe (#2).
        //    The omlx server is DOWN here (result=false), so a stale reuse of ollama's
        //    available=true would be the exact bug (tier driven against a down daemon).
        let omlx = ProviderStub {
            provider: "omlx",
            result: false,
            probes: probes.clone(),
        };
        assert!(
            !st.ensure_gemma_probed(&omlx),
            "omlx must re-probe, not reuse the ollama answer"
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "a provider switch re-probes (cache miss on provider mismatch)"
        );
        assert_eq!(st.gemma_status(), "offline", "status reflects the latest (omlx) probe");

        // 4) A second omlx call now hits the (re-keyed) cache — NO new probe.
        let omlx2 = ProviderStub {
            provider: "omlx",
            result: false,
            probes: probes.clone(),
        };
        assert!(!st.ensure_gemma_probed(&omlx2));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "the cache is now keyed to omlx — same-provider call reuses it"
        );
    }

    fn test_root(tag: &str) -> PathBuf {
        // A distinct, existing dir per tag (canonicalization-agnostic — we only need
        // path equality between handle.root() and the stored slot).
        let dir =
            std::env::temp_dir().join(format!("aspis-censor-cmd-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    // ---- BLOCKER 3+4: atomic install — second start same root is a no-op; a
    //                   different root cleanly replaces (one active handle). ----

    #[test]
    fn install_handle_same_root_is_idempotent_noop() {
        let root = test_root("idem");
        let first = CensorWatchHandle::for_test("p1", root.clone());
        let mut slot: Option<CensorWatchHandle> = Some(first);

        // A second start for the SAME root: the incoming watcher is returned to be
        // reaped, the originally-installed handle stays put and is NOT signaled.
        let incoming = CensorWatchHandle::for_test("p1", root.clone());
        let reaped = install_handle(&mut slot, incoming, root.as_path());

        let reaped = reaped.expect("idempotent start returns the just-built watcher to reap");
        assert!(
            reaped.is_running(),
            "the discarded incoming is not the active one"
        );
        let active = slot
            .as_ref()
            .expect("the original handle is still installed");
        assert!(
            active.is_running(),
            "the original active handle is untouched"
        );
        assert_eq!(active.project_id(), "p1");
        // Exactly one active handle remains in the slot.
        reaped.stop();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_handle_different_root_replaces_and_signals_old() {
        let root_a = test_root("repl-a");
        let root_b = test_root("repl-b");
        let old = CensorWatchHandle::for_test("p-old", root_a.clone());
        let mut slot: Option<CensorWatchHandle> = Some(old);

        // Start for a DIFFERENT root: the new handle is installed, the old one is
        // returned to reap AND was stop-signaled UNDER the lock (so two workers can
        // never run for one project).
        let incoming = CensorWatchHandle::for_test("p-new", root_b.clone());
        let reaped = install_handle(&mut slot, incoming, root_b.as_path());

        let old = reaped.expect("a replace returns the previous handle");
        assert!(
            !old.is_running(),
            "the outgoing handle must be stop-signaled before the swap"
        );
        // Exactly ONE active handle, and it is the new one.
        let active = slot.as_ref().expect("the new handle is installed");
        assert!(active.is_running());
        assert_eq!(active.project_id(), "p-new");
        assert_eq!(active.root(), root_b.as_path());

        old.stop();
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    // ---- MAJOR race: review_now routes through the active watcher's serialized
    //                  worker (enqueue) rather than running inline. ----

    #[test]
    fn enqueue_review_if_active_routes_to_matching_watcher_else_falls_back() {
        let root = test_root("enq");
        let st = CensorState::new();

        // No watcher → not enqueued (caller runs inline one-shot).
        assert!(!st.enqueue_review_if_active("p1", root.as_path(), None));

        // Install a watcher for p1@root.
        {
            let mut g = st.watch.lock().unwrap();
            *g = Some(CensorWatchHandle::for_test("p1", root.clone()));
        }
        // Matching project + root → enqueued onto its serialized worker.
        assert!(st.enqueue_review_if_active("p1", root.as_path(), Some("src/a.ts".into())));
        // Wrong project → not enqueued (falls back to inline).
        assert!(!st.enqueue_review_if_active("other", root.as_path(), None));
        // Wrong root → not enqueued.
        let other_root = test_root("enq-other");
        assert!(!st.enqueue_review_if_active("p1", other_root.as_path(), None));

        // Cleanup (stop the watcher).
        if let Some(h) = st.take_handle() {
            h.stop();
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other_root);
    }

    // ---- BLOCKER (trust-revoke): set_censor_trusted(X, false) stops X's running
    //      watcher; revoking a DIFFERENT project leaves X's watcher running. The
    //      atomic teardown core is `take_handle_if_project` (the same factoring as
    //      `install_handle`/`match_censor_root` — testable without a Tauri app). ----

    #[test]
    fn untrust_stops_running_watcher_for_that_project() {
        let root = test_root("untrust-x");
        let st = CensorState::new();
        // Install a live watcher for project X.
        {
            let mut g = st.watch.lock().unwrap();
            *g = Some(CensorWatchHandle::for_test("X", root.clone()));
        }
        // Revoking trust for X takes its handle out under the lock (the command then
        // runs the non-blocking stop on it). The slot is left empty → watcher gone.
        let taken = st
            .take_handle_if_project("X")
            .expect("X's watcher handle is returned for teardown");
        assert!(
            taken.is_running(),
            "the returned handle is X's live watcher"
        );
        assert!(
            st.watch.lock().unwrap().is_none(),
            "after untrust X has no active watcher in the slot"
        );
        taken.stop();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn untrust_different_project_leaves_active_watcher_running() {
        let root = test_root("untrust-y");
        let st = CensorState::new();
        // X is the active watcher.
        {
            let mut g = st.watch.lock().unwrap();
            *g = Some(CensorWatchHandle::for_test("X", root.clone()));
        }
        // Revoking trust for a DIFFERENT project Y is a no-op for X's watcher.
        assert!(
            st.take_handle_if_project("Y").is_none(),
            "untrusting Y must not take X's handle"
        );
        let guard = st.watch.lock().unwrap();
        let active = guard.as_ref().expect("X's watcher is still installed");
        assert_eq!(
            active.project_id(),
            "X",
            "X keeps watching after Y is untrusted"
        );
        assert!(active.is_running(), "X's watcher is still running");
        drop(guard);
        if let Some(h) = st.take_handle() {
            h.stop();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn untrust_with_no_active_watcher_is_noop() {
        // Revoking trust when nothing is being watched must not panic or fabricate a
        // handle (the command then simply skips the stop).
        let st = CensorState::new();
        assert!(st.take_handle_if_project("anyone").is_none());
    }
}
