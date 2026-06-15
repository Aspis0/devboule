//! Censor filesystem watcher — one handle per WATCHED PROJECT ROOT.
//!
//! Modeled on the Polis `WatchHandle` (`polis/watcher.rs`): a `notify` recursive
//! watcher owned by a dedicated DEBOUNCE thread, plus a non-blocking
//! signal-then-detached-reaper teardown shared by `stop()` and `Drop` (NEVER a
//! blocking join on quit). Differences from Polis:
//!
//!   - TWO debounce windows fed off the SAME watcher events, bucketed by the
//!     changed file's language:
//!       * FINE (~400ms): TS/Python files — cheap per-file linters; the settled
//!         set of changed fine files is handed to `orchestrator::run_fine_batch`.
//!       * COARSE (~4s): Rust files, "Other" files (cross-cutting only), and a
//!         manual coarse trigger — slow crate/project tools; the settle fires
//!         `orchestrator::run_coarse_pass` once for the whole project.
//!   - The ignore set adds `.aspis-censor` (the shard dir) so our OWN ledger
//!     writes can never self-trigger a review (mirrors how Polis ignores
//!     `.aspis-meta.json`).
//!   - The actual review work (subprocess spawns + shard writes + the
//!     `censor://findings-updated` emit) runs on a SEPARATE single worker thread
//!     drained by the debounce thread, so a slow clippy/tsc pass never blocks the
//!     debounce loop from coalescing the next burst. The worker SERIALIZES all
//!     work for this project (coarse cargo tools must be serialized anyway, and
//!     serializing fine batches avoids shard-lock contention) — Censor is
//!     explicitly allowed to lag.
//!
//! Both extra threads (debounce + worker) are reaped on teardown: stop signals
//! the flag, the debounce loop exits, it closes the worker's queue, and a detached
//! reaper joins both. No orphan thread, no orphan tool subprocess (the runners
//! have their own timeouts/kill).

use super::gemma::{self, CensorLocalAi};
use super::orchestrator::{self, GemmaCtx, COARSE_DEBOUNCE_MS, FINE_DEBOUNCE_MS};
use crate::backend::censor::detect::FileLang;
use crate::backend::fs_watch::{is_excluded_path, DebounceState};
use crate::polis::scanner;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Directory names excluded from review (build/deps/VCS noise + OUR shard dir).
/// Reuses the scanner's excluded set so Censor and Polis stay aligned, and ADDS
/// `.aspis-censor` so writing a shard never re-triggers a review (self-guard).
///
/// Computed ONCE for the whole process (the set is identical for every watcher and
/// every event) rather than allocating a fresh `Vec` on each notify event — the
/// notify callback fires on every fs change, so a per-event allocation was pure
/// churn on the hot path (NITPICK).
fn excluded_dirs() -> &'static [&'static str] {
    static EXCLUDED: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    EXCLUDED.get_or_init(|| {
        let mut v: Vec<&'static str> = scanner::EXCLUDED_DIRS.to_vec();
        v.push(super::CENSOR_DIR); // ".aspis-censor"
        v
    })
}

/// Is a changed `path` (under `root`) one Censor should review? Excludes the
/// build/deps/VCS dirs + our own shard dir, and the Polis meta file (we never
/// review it). Beyond the location screen, ANY remaining file is potentially
/// reviewable (the cross-cutting runners apply to every file); the per-file
/// language decision happens later when bucketing fine vs coarse.
fn is_relevant(path: &Path, root: &Path) -> bool {
    // Ignore the Polis meta file too (it churns on Polis scans) and any dotfile-
    // style sidecar we don't want to review. The shard dir is in `excluded`.
    !is_excluded_path(path, root, excluded_dirs(), &[".aspis-meta.json"])
}

/// A relevant change reported by the notify callback to the debounce loop: the
/// changed file's project-relative path (forward-slash normalized) + its language
/// (so the loop buckets it onto the fine or coarse window).
#[derive(Debug, Clone)]
struct ChangeEvent {
    rel_path: String,
    lang: FileLang,
}

/// A unit of work the debounce loop (or `censor_review_now`) hands to the
/// serialized worker thread. One worker per project drains these in order, so ALL
/// shard-mutating passes for a project are serialized — no concurrent
/// read-modify-write, and the coarse cargo tools (which must be serialized anyway)
/// never overlap.
enum Work {
    /// Settled FINE batch: the accumulated changed-file set.
    Fine(Vec<String>),
    /// Settled COARSE project-wide pass.
    Coarse,
    /// On-demand `censor_review_now`. `Some(rel)` rechecks one file's FINE runners;
    /// `None` runs the whole-project COARSE sweep. Routed through the worker (not run
    /// inline on the command thread) so it cannot race the watcher's passes.
    ReviewNow(Option<String>),
}

/// Owns the running Censor watcher + its debounce thread (which in turn owns the
/// worker thread). Dropping it (or `stop`) signals the loop to exit and reaps both
/// threads via a detached reaper — never blocking the caller (mirrors Polis).
pub struct CensorWatchHandle {
    project_id: String,
    root: PathBuf,
    running: Arc<AtomicBool>,
    /// Sender to the debounce loop for a MANUAL coarse trigger (e.g. on a commit
    /// signal). Currently unused by commands (review-now bypasses the watcher),
    /// but wired for Phase D's commit/push coarse kick.
    manual_coarse_tx: mpsc::Sender<()>,
    /// Sender to the SERIALIZED worker queue. `censor_review_now` enqueues a
    /// `Work::ReviewNow` here so an on-demand review runs ON THE SAME single worker
    /// as the watcher's fine/coarse passes — eliminating the concurrent
    /// read-modify-write on shards that an inline command-thread review caused
    /// (MAJOR race). A send error means the worker is already gone (torn down).
    work_tx: mpsc::Sender<Work>,
    thread: Option<JoinHandle<()>>,
}

impl CensorWatchHandle {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Fire a manual COARSE pass (best-effort; a send error means the loop is
    /// already gone). Used by callers that know a project-wide change happened
    /// out of band (e.g. a commit) without waiting for a file event.
    #[allow(dead_code)] // first caller is Phase D (commit/push); wired now.
    pub fn trigger_coarse(&self) {
        let _ = self.manual_coarse_tx.send(());
    }

    /// Enqueue an on-demand review onto THIS watcher's serialized worker queue so it
    /// runs in order with the watcher's fine/coarse passes (never concurrently —
    /// the MAJOR race fix). `file = Some(rel)` rechecks one file's FINE runners;
    /// `file = None` runs the whole-project COARSE sweep. Returns `false` if the
    /// worker is already gone (a teardown raced the enqueue), so the caller can fall
    /// back to a one-shot run. Non-blocking: the worker does the IO, not the caller.
    pub fn enqueue_review(&self, file: Option<String>) -> bool {
        self.work_tx.send(Work::ReviewNow(file)).is_ok()
    }

    /// Signal-only stop: set the running flag to `false` WITHOUT spawning the
    /// reaper. Cheap (a single atomic store, no IO, no thread spawn), so it is safe
    /// to call while holding the `CensorState` lock. Used by `censor_start_watch` to
    /// signal the OUTGOING watcher to stop BEFORE the new handle is stored under the
    /// same lock — guaranteeing two workers never run for one project (BLOCKER 3+4).
    /// The actual (detached) reap then happens via `stop()` outside the lock.
    pub fn signal_stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Shared non-blocking teardown for `stop()` and `Drop`: signal the loop, then
    /// hand the join to a detached reaper so the caller never blocks on an in-flight
    /// review. The debounce loop observes `running`, drains/closes the worker queue,
    /// and returns; the reaper joins it (and it joins the worker) — cleanly reaped,
    /// never leaked. A reaper-spawn failure leaves the thread to self-terminate on
    /// the flag (unjoined but not leaked).
    fn signal_and_reap(running: &Arc<AtomicBool>, thread: Option<JoinHandle<()>>) {
        running.store(false, Ordering::SeqCst);
        if let Some(t) = thread {
            let spawned = std::thread::Builder::new()
                .name("censor-watcher-reaper".into())
                .spawn(move || {
                    let _ = t.join();
                });
            if let Err(e) = spawned {
                eprintln!(
                    "censor watcher: reaper spawn failed ({e}); worker left to self-terminate"
                );
            }
        }
    }

    /// Stop the watcher cleanly (non-blocking). The notify watcher lives inside the
    /// debounce thread, so it is dropped (unsubscribed) when that thread returns.
    pub fn stop(mut self) {
        Self::signal_and_reap(&self.running, self.thread.take());
    }
}

impl Drop for CensorWatchHandle {
    fn drop(&mut self) {
        Self::signal_and_reap(&self.running, self.thread.take());
    }
}

#[cfg(test)]
impl CensorWatchHandle {
    /// Build a thread-light handle for unit tests of the lifecycle/state invariants
    /// (no real notify watcher / `AppHandle` needed). It owns a tiny worker thread
    /// that polls the stop flag (mirroring the real worker's teardown), so `stop()`
    /// reaps cleanly and `enqueue_review`/`signal_stop` are exercisable.
    pub(crate) fn for_test(project_id: &str, root: PathBuf) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let (manual_coarse_tx, _manual_rx) = mpsc::channel::<()>();
        let (work_tx, work_rx) = mpsc::channel::<Work>();
        let worker_running = running.clone();
        let thread = std::thread::spawn(move || loop {
            if !worker_running.load(Ordering::SeqCst) {
                break;
            }
            match work_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        });
        Self {
            project_id: project_id.to_string(),
            root,
            running,
            manual_coarse_tx,
            work_tx,
            thread: Some(thread),
        }
    }

    /// Test accessor: is the watcher's stop flag still set (i.e. not yet signaled)?
    pub(crate) fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Start watching `root` recursively for project `project_id`, dispatching
/// debounced FINE/COARSE review passes and emitting `censor://findings-updated`.
/// Returns a handle the caller stores in `CensorState`; dropping/stopping it tears
/// everything down. A watch-setup failure is reported synchronously (clear error,
/// no silent dead thread).
///
/// `local_ai` is the RESOLVED tier-2 provider config snapshot (Ollama default OR oMLX)
/// that the caller already used to PROBE availability. The worker builds its own client
/// from this SAME snapshot via `gemma::build_gemma_client`, so probe + worker can never
/// split-brain (probe on one provider, worker on another). `gemma_available` is that
/// probe's cached result.
pub fn start_watch(
    app: AppHandle,
    project_id: String,
    root: PathBuf,
    gemma_available: bool,
    local_ai: CensorLocalAi,
) -> Result<CensorWatchHandle, String> {
    let running = Arc::new(AtomicBool::new(true));

    // notify → debounce-loop channel of relevant changes.
    let (change_tx, change_rx) = mpsc::channel::<ChangeEvent>();
    // Manual coarse trigger channel (kept on the handle).
    let (manual_coarse_tx, manual_coarse_rx) = mpsc::channel::<()>();
    // Serialized worker queue. Created HERE (not inside the loop) so the handle can
    // hold a `work_tx` clone for `enqueue_review`: an on-demand review runs on the
    // SAME single worker as the watcher's passes (the MAJOR race fix). The loop
    // takes the other clone for fine/coarse sends; the worker owns the rx.
    let (work_tx, work_rx) = mpsc::channel::<Work>();
    let loop_work_tx = work_tx.clone();

    // Build the watcher BEFORE moving into the thread so a setup failure is
    // reported synchronously.
    let cb_root = root.clone();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let event = match res {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("censor watcher: fs event error: {e}");
                    return;
                }
            };
            for p in &event.paths {
                if !is_relevant(p, &cb_root) {
                    continue;
                }
                let rel = p
                    .strip_prefix(&cb_root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel.is_empty() {
                    continue;
                }
                let lang = FileLang::from_path(Path::new(&rel));
                // Ignore send errors: a gone receiver means we're shutting down.
                let _ = change_tx.send(ChangeEvent {
                    rel_path: rel,
                    lang,
                });
            }
        })
        .map_err(|e| format!("Failed to create Censor filesystem watcher: {e}"))?;

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| format!("Failed to watch project root for Censor: {e}"))?;

    let thread_running = running.clone();
    let thread_root = root.clone();
    let thread_project = project_id.clone();
    let thread = std::thread::Builder::new()
        .name("censor-watcher".into())
        .spawn(move || {
            let _watcher = watcher; // keep alive for the thread's lifetime
            run_loop(
                app,
                thread_project,
                thread_root,
                change_rx,
                manual_coarse_rx,
                loop_work_tx,
                work_rx,
                gemma_available,
                local_ai,
                thread_running,
            );
        })
        .map_err(|e| format!("Failed to spawn Censor watcher thread: {e}"))?;

    Ok(CensorWatchHandle {
        project_id,
        root,
        running,
        manual_coarse_tx,
        work_tx,
        thread: Some(thread),
    })
}

/// The debounce + dispatch loop. Owns the notify watcher (kept alive by the
/// caller's `_watcher`), the change receiver, and the worker thread. Buckets each
/// change onto the FINE or COARSE window, and on a settled window enqueues the
/// matching `Work` onto the worker (off-thread so a slow pass never blocks
/// coalescing). Exits promptly on the stop flag, dropping its `work_tx` so — once
/// the handle's `work_tx` is also gone — the worker drains and returns.
///
/// The worker queue is created by `start_watch` (so the handle holds a `work_tx`
/// clone for `enqueue_review`); this loop receives `work_tx` (its send clone) and
/// `work_rx` (handed to the worker thread).
#[allow(clippy::too_many_arguments)]
fn run_loop(
    app: AppHandle,
    project_id: String,
    root: PathBuf,
    change_rx: mpsc::Receiver<ChangeEvent>,
    manual_coarse_rx: mpsc::Receiver<()>,
    work_tx: mpsc::Sender<Work>,
    work_rx: mpsc::Receiver<Work>,
    gemma_available: bool,
    local_ai: CensorLocalAi,
    running: Arc<AtomicBool>,
) {
    // Serialized worker: drains the Work queue and runs the orchestrator for each.
    // One worker per project = coarse tools serialized + no concurrent shard
    // contention + on-demand reviews serialized with watcher passes.
    //
    // TEARDOWN: two `work_tx` clones exist (this loop's + the handle's, for
    // `enqueue_review`), so a plain blocking `recv()` would not return Err until
    // BOTH drop — leaving the worker blocked (and the reaper join hanging) if a stop
    // lands while the worker is idle and the handle's clone hasn't dropped yet. We
    // therefore `recv_timeout` and poll the stop flag, so the worker exits promptly
    // on teardown regardless of which `work_tx` clones are still alive.
    let worker_app = app.clone();
    let worker_project = project_id.clone();
    let worker_root = root.clone();
    let worker_running = running.clone();
    let worker = std::thread::Builder::new()
        .name("censor-worker".into())
        .spawn(move || {
            // The Gemma client is owned by THIS single worker thread, so the optional
            // tier inherits the per-project serialization for free (one Gemma call at
            // a time — no second concurrency mechanism). It is built ONCE here from the
            // SAME `local_ai` config snapshot the probe used at `censor_start_watch`, so
            // probe + worker can never split-brain (probe on Ollama, worker on oMLX); it
            // is then reused for every fine pass. `gemma_available` is the cached probe
            // result (probed once at `censor_start_watch`). The client is built regardless
            // so the ctx is cheap to construct each pass; when `available` is false the
            // orchestrator never calls `generate` (see `run_gemma`).
            let gemma_client = match gemma::build_gemma_client(&local_ai) {
                Ok(client) => Some(client),
                Err(e) => {
                    eprintln!("censor gemma: {e}");
                    None
                }
            };
            let gemma_ctx: Option<GemmaCtx<'_>> = gemma_client.as_deref().map(|client| GemmaCtx {
                client,
                available: gemma_available,
            });
            loop {
                if !worker_running.load(Ordering::SeqCst) {
                    break;
                }
                let work = match work_rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(w) => w,
                    Err(RecvTimeoutError::Timeout) => continue, // re-check stop flag
                    Err(RecvTimeoutError::Disconnected) => break, // all senders gone
                };
                // Honor a stop that arrived while this item was queued: bail before
                // doing the (potentially long) subprocess work for a torn-down watch.
                if !worker_running.load(Ordering::SeqCst) {
                    break;
                }
                match work {
                    Work::Fine(files) => {
                        orchestrator::run_fine_batch(
                            &worker_app,
                            &worker_project,
                            &worker_root,
                            &files,
                            gemma_ctx,
                            &worker_running,
                        );
                    }
                    Work::Coarse => {
                        orchestrator::run_coarse_pass(
                            &worker_app,
                            &worker_project,
                            &worker_root,
                            &worker_running,
                        );
                    }
                    Work::ReviewNow(file) => {
                        orchestrator::run_review_now(
                            &worker_app,
                            &worker_project,
                            &worker_root,
                            file.as_deref(),
                            gemma_ctx,
                            &worker_running,
                        );
                    }
                }
            }
        });
    let worker = match worker {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("censor watcher: failed to spawn worker thread: {e}; watch disabled");
            None
        }
    };

    let mut fine = DebounceState::new(Duration::from_millis(FINE_DEBOUNCE_MS));
    let mut coarse = DebounceState::new(Duration::from_millis(COARSE_DEBOUNCE_MS));
    let mut fine_files: BTreeSet<String> = BTreeSet::new();
    let mut coarse_pending = false;

    while running.load(Ordering::SeqCst) {
        // Wake at the SOONER of the two pending debounce deadlines (or a bounded
        // idle timeout so the stop flag is observed promptly without spinning).
        let now = Instant::now();
        let timeout = [fine.time_until_quiet(now), coarse.time_until_quiet(now)]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or_else(|| Duration::from_millis(250));

        match change_rx.recv_timeout(timeout) {
            Ok(ev) => {
                bucket_event(
                    ev,
                    &mut fine,
                    &mut coarse,
                    &mut fine_files,
                    &mut coarse_pending,
                );
                // Drain any already-queued events so a burst collapses immediately.
                while let Ok(ev) = change_rx.try_recv() {
                    bucket_event(
                        ev,
                        &mut fine,
                        &mut coarse,
                        &mut fine_files,
                        &mut coarse_pending,
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break, // watcher gone
        }

        // Drain any manual coarse triggers (non-blocking).
        while manual_coarse_rx.try_recv().is_ok() {
            coarse.record(Instant::now());
            coarse_pending = true;
        }

        if !running.load(Ordering::SeqCst) {
            break;
        }

        let now = Instant::now();
        // FINE settle → enqueue the accumulated changed-file set, reset it.
        if fine.take_if_quiet(now) && !fine_files.is_empty() {
            let files: Vec<String> = std::mem::take(&mut fine_files).into_iter().collect();
            let _ = work_tx.send(Work::Fine(files));
        }
        // COARSE settle → enqueue a project-wide pass.
        if coarse.take_if_quiet(now) && coarse_pending {
            coarse_pending = false;
            let _ = work_tx.send(Work::Coarse);
        }
    }

    // Teardown: drop this loop's `work_tx` clone and join the worker. The worker
    // observes the stop flag via its `recv_timeout` poll and exits within one tick
    // even if the handle's `work_tx` clone is still alive, so this join completes
    // promptly. We are on the detached reaper's thread already (via the handle's
    // signal_and_reap), so this join does not block any user-facing call.
    drop(work_tx);
    if let Some(w) = worker {
        let _ = w.join();
    }
}

/// Route one change onto the correct debounce window + accumulator.
///
/// TS/Python/Go/C-C++ files go to the FINE window (cheap per-file linters need the
/// exact changed set: eslint/ruff/gofmt/cppcheck — cppcheck is a no-compile static
/// analyzer, cheap enough for the per-file path). Their COMPILE-BASED / project-wide
/// tools (tsc/knip for TS, go vet for Go) are COARSE and ride the coarse window like
/// every other coarse runner — never the hot per-file path (go vet COMPILES, so this is
/// the load-bearing reason it must not be fine-routed). Rust files and "Other" files
/// (cross-cutting only) go to the COARSE window — a Rust edit means clippy/cargo-check;
/// an "Other" edit (config, etc.) still warrants the project-wide gitleaks/jscpd sweep.
/// A file can only be one language, so it lands in exactly one bucket.
fn bucket_event(
    ev: ChangeEvent,
    fine: &mut DebounceState,
    coarse: &mut DebounceState,
    fine_files: &mut BTreeSet<String>,
    coarse_pending: &mut bool,
) {
    let now = Instant::now();
    match ev.lang {
        FileLang::Ts | FileLang::Py | FileLang::Go | FileLang::Cpp => {
            fine_files.insert(ev.rel_path);
            fine.record(now);
        }
        FileLang::Rust | FileLang::Other => {
            *coarse_pending = true;
            coarse.record(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ---- ignore set: .aspis-censor self-guard + meta + build dirs ----

    #[test]
    fn shard_dir_writes_never_self_trigger() {
        let r = root();
        // A shard write under .aspis-censor must be IRRELEVANT (self-trigger guard).
        assert!(!is_relevant(&under(&r, ".aspis-censor/deadbeef.json"), &r));
        assert!(!is_relevant(
            &under(&r, ".aspis-censor/deadbeef.json.lock"),
            &r
        ));
        // The Polis meta file is ignored too.
        assert!(!is_relevant(&under(&r, ".aspis-meta.json"), &r));
        // Build/deps/VCS noise ignored (delegated to the scanner excluded set).
        assert!(!is_relevant(&under(&r, "node_modules/x/index.ts"), &r));
        assert!(!is_relevant(&under(&r, "target/debug/foo.rs"), &r));
        assert!(!is_relevant(&under(&r, ".git/HEAD"), &r));
    }

    #[test]
    fn real_source_files_are_relevant() {
        let r = root();
        assert!(is_relevant(&under(&r, "src/main.rs"), &r));
        assert!(is_relevant(&under(&r, "src/app.ts"), &r));
        assert!(is_relevant(&under(&r, "pkg/mod.py"), &r));
        // An "Other" file (config) is still relevant (cross-cutting runners apply).
        assert!(is_relevant(&under(&r, "Cargo.toml"), &r));
    }

    // ---- bucket_event: fine vs coarse routing ----

    #[test]
    fn ts_and_py_go_to_fine_rust_and_other_to_coarse() {
        let mut fine = DebounceState::new(Duration::from_millis(FINE_DEBOUNCE_MS));
        let mut coarse = DebounceState::new(Duration::from_millis(COARSE_DEBOUNCE_MS));
        let mut fine_files = BTreeSet::new();
        let mut coarse_pending = false;

        bucket_event(
            ChangeEvent {
                rel_path: "src/a.ts".into(),
                lang: FileLang::Ts,
            },
            &mut fine,
            &mut coarse,
            &mut fine_files,
            &mut coarse_pending,
        );
        bucket_event(
            ChangeEvent {
                rel_path: "pkg/b.py".into(),
                lang: FileLang::Py,
            },
            &mut fine,
            &mut coarse,
            &mut fine_files,
            &mut coarse_pending,
        );
        // Fine bucket has both fine files; coarse not yet pending.
        assert!(fine.pending());
        assert!(fine_files.contains("src/a.ts"));
        assert!(fine_files.contains("pkg/b.py"));
        assert!(!coarse_pending);

        bucket_event(
            ChangeEvent {
                rel_path: "src/lib.rs".into(),
                lang: FileLang::Rust,
            },
            &mut fine,
            &mut coarse,
            &mut fine_files,
            &mut coarse_pending,
        );
        bucket_event(
            ChangeEvent {
                rel_path: "Cargo.toml".into(),
                lang: FileLang::Other,
            },
            &mut fine,
            &mut coarse,
            &mut fine_files,
            &mut coarse_pending,
        );
        // A Rust + an Other change set coarse pending; the rust/other files are NOT
        // added to the fine file set (they ride the coarse project-wide pass).
        assert!(coarse.pending());
        assert!(coarse_pending);
        assert_eq!(
            fine_files.len(),
            2,
            "rust/other files do not enter the fine set"
        );
    }

    #[test]
    fn go_file_goes_to_fine_bucket() {
        // A .go edit routes to FINE (gofmt runs per-file); it does NOT set the coarse
        // pending flag (go vet rides the coarse window like tsc/knip for TS).
        let mut fine = DebounceState::new(Duration::from_millis(FINE_DEBOUNCE_MS));
        let mut coarse = DebounceState::new(Duration::from_millis(COARSE_DEBOUNCE_MS));
        let mut fine_files = BTreeSet::new();
        let mut coarse_pending = false;
        bucket_event(
            ChangeEvent {
                rel_path: "cmd/main.go".into(),
                lang: FileLang::Go,
            },
            &mut fine,
            &mut coarse,
            &mut fine_files,
            &mut coarse_pending,
        );
        assert!(fine.pending());
        assert!(fine_files.contains("cmd/main.go"));
        assert!(!coarse_pending, "a go edit alone must not flip coarse pending");
    }

    #[test]
    fn cpp_file_goes_to_fine_bucket() {
        // A .cpp edit routes to FINE (cppcheck is a no-compile per-file analyzer); it
        // does NOT flip the coarse pending flag (no coarse C/C++ runner exists).
        let mut fine = DebounceState::new(Duration::from_millis(FINE_DEBOUNCE_MS));
        let mut coarse = DebounceState::new(Duration::from_millis(COARSE_DEBOUNCE_MS));
        let mut fine_files = BTreeSet::new();
        let mut coarse_pending = false;
        bucket_event(
            ChangeEvent {
                rel_path: "src/main.cpp".into(),
                lang: FileLang::Cpp,
            },
            &mut fine,
            &mut coarse,
            &mut fine_files,
            &mut coarse_pending,
        );
        assert!(fine.pending());
        assert!(fine_files.contains("src/main.cpp"));
        assert!(!coarse_pending, "a cpp edit alone must not flip coarse pending");
    }

    #[test]
    fn fine_burst_on_same_file_coalesces_to_one_entry() {
        let mut fine = DebounceState::new(Duration::from_millis(FINE_DEBOUNCE_MS));
        let mut coarse = DebounceState::new(Duration::from_millis(COARSE_DEBOUNCE_MS));
        let mut fine_files = BTreeSet::new();
        let mut coarse_pending = false;
        for _ in 0..5 {
            bucket_event(
                ChangeEvent {
                    rel_path: "src/a.ts".into(),
                    lang: FileLang::Ts,
                },
                &mut fine,
                &mut coarse,
                &mut fine_files,
                &mut coarse_pending,
            );
        }
        // A burst on one file collapses to ONE entry (the set dedups).
        assert_eq!(fine_files.len(), 1);
    }

    // ---- lifecycle: start→stop leaves no live thread (best-effort) ----

    #[test]
    fn handle_stop_reaps_threads() {
        // We can't spawn a real notify watcher deterministically in CI, so model
        // the handle's TERMINATION contract directly: a stub debounce thread that
        // spins on the same `running` flag the real loop polls, plus a worker it
        // owns. After stop(), the Arc<running> strong count must fall back to the
        // test's single reference (the threads released their clones → reaped).
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();
        let (_manual_tx, _manual_rx) = mpsc::channel::<()>();
        // The handle now holds a `work_tx` clone (for enqueue_review). Model that:
        // the loop and the handle each get a clone; the worker uses recv_timeout so
        // it exits on the stop flag even while the handle's clone is alive (matching
        // the real teardown).
        let (work_tx, work_rx) = mpsc::channel::<Work>();
        let loop_work_tx = work_tx.clone();
        let worker_running = running.clone();
        let thread = std::thread::Builder::new()
            .name("censor-watcher-test-stub".into())
            .spawn(move || {
                // Mirror the real loop: own a worker that polls the stop flag, spin
                // on the flag, then drop our work_tx and join on exit.
                let worker = std::thread::spawn(move || loop {
                    if !worker_running.load(Ordering::SeqCst) {
                        break;
                    }
                    match work_rx.recv_timeout(Duration::from_millis(10)) {
                        Ok(_) => {}
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                });
                while thread_running.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                drop(loop_work_tx);
                let _ = worker.join();
            })
            .expect("spawn stub debounce thread");

        let handle = CensorWatchHandle {
            project_id: "p".into(),
            root: root(),
            running: running.clone(),
            manual_coarse_tx: _manual_tx,
            work_tx,
            thread: Some(thread),
        };
        assert_eq!(handle.project_id(), "p");

        handle.stop();
        // Poll for the debounce thread to release its `running` clone (reaped).
        let mut spun = 0;
        while Arc::strong_count(&running) != 1 && spun < 500 {
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        assert_eq!(
            Arc::strong_count(&running),
            1,
            "after stop() the watcher thread's Arc<running> must be released (reaped)"
        );
    }
}
