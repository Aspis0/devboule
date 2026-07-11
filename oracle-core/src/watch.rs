//! File-system and git-ref watcher for Oracle's auto-reindex mode.
//!
//! Port of `oracle/watcher/git_watcher.py` (312 LOC) and the
//! `index_jobs.py::start_watcher` fs-watcher arming path. Both watcher kinds
//! return a uniform [`WatcherHandle`] that exposes `stop()` / `join()` so the
//! job manager can tear down either kind identically.
//!
//! ## Design choices
//!
//! * **Debounce**: hand-rolled generation-counter pattern with `std::thread`.
//!   `notify-debouncer-mini` was considered but rejected — the three-phase
//!   teardown and per-repo git watcher need fine-grained cancellation control
//!   that a library debouncer does not expose.
//! * **notify crate v7**: cross-platform FS notifications.
//! * **Git watcher**: bounded BFS repo discovery (depth ≤ 3, ≤ 64 repos,
//!   skip heavy/irrelevant trees), watch `.git` (non-recursive) +
//!   `.git/refs/heads` (recursive), event filter identical to Python's
//!   `_is_commit_event`, 3 s debounce burst → single `on_commit` callback.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::Event;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

// ═══════════════════════════════════════════════════════════════════════════
// Constants (mirror Python config.py values)
// ═══════════════════════════════════════════════════════════════════════════

/// Debounce window for the git-ref watcher (seconds). A single `git commit`
/// touches HEAD/packed-refs/refs several times; 3 s coalesces that burst.
const GIT_WATCH_DEBOUNCE_SECONDS: f64 = 3.0;

/// Default debounce for the recursive FS watcher (seconds).
/// Matches `ORACLE_WATCH_DEBOUNCE` default in `oracle/config.py`.
const FS_WATCH_DEBOUNCE_SECONDS: u64 = 30;

/// Bound repo discovery so pathological workspaces cannot schedule unbounded
/// OS watches.
const DEFAULT_MAX_REPOS: usize = 64;
const DEFAULT_MAX_DEPTH: usize = 3;

/// Directories we never descend into when discovering repos.
const SKIP_DIRS: &[&str] = &["node_modules", ".venv", "venv", "dist", "target"];

/// Subdirectories of the workspace root watched by the FS watcher.
/// Matches `oracle/config.py::WATCH_DIRS`.
const FS_WATCH_DIRS: &[&str] = &[
    "workers",
    "containers",
    "src",
    "src-tauri/src",
    "oracle",
    "projects",
];

/// File extensions that trigger re-indexing in FS watch mode.
/// Matches `oracle/config.py::WATCH_EXTENSIONS`.
const FS_WATCH_EXTENSIONS: &[&str] = &[
    ".js", ".ts", ".tsx", ".py", ".rs", ".go", ".json", ".yaml", ".yml", ".md",
];

// ═══════════════════════════════════════════════════════════════════════════
// Debouncer — generation-counter pattern
// ═══════════════════════════════════════════════════════════════════════════

struct DebouncerInner {
    generation: u64,
    handle: Option<JoinHandle<()>>,
}

/// Cancel-and-reschedule debounce timer.
///
/// Each call to `trigger()` increments the generation counter and spawns a new
/// timer thread. Only the most-recent generation actually fires the callback.
/// `cancel()` invalidates any pending timer and joins its thread.
struct Debouncer {
    state: Arc<Mutex<DebouncerInner>>,
    callback: Arc<dyn Fn() + Send + Sync>,
    duration: Duration,
}

impl Debouncer {
    fn new(callback: Arc<dyn Fn() + Send + Sync>, duration: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(DebouncerInner {
                generation: 0,
                handle: None,
            })),
            callback,
            duration,
        }
    }

    fn trigger(&self) {
        // NEVER join a timer thread while holding `state`: the timer needs
        // that lock to read the generation, so join-under-lock deadlocks the
        // event thread AND every pending timer (found by the P6b end-to-end
        // test). Superseded timers are detached — they wake, see a newer
        // generation, and exit on their own.
        let (gen, superseded) = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.generation += 1;
            (s.generation, s.handle.take())
        };
        drop(superseded); // detach

        let state = Arc::clone(&self.state);
        let callback = Arc::clone(&self.callback);
        let duration = self.duration;
        let handle = thread::spawn(move || {
            thread::sleep(duration);
            let current = state.lock().unwrap_or_else(|e| e.into_inner()).generation;
            if current == gen {
                callback();
            }
        });
        self.state.lock().unwrap_or_else(|e| e.into_inner()).handle = Some(handle);
    }

    fn cancel(&self) {
        // Bump the generation and TAKE the handle under the lock, then join
        // OUTSIDE it so the timer can acquire the lock and exit.
        let pending = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.generation = s.generation.wrapping_add(1); // invalidate pending
            s.handle.take()
        };
        if let Some(h) = pending {
            let _ = h.join();
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Git commit event filter
// ═══════════════════════════════════════════════════════════════════════════

/// Returns `true` iff a changed path under `.git` represents a commit/ref
/// change. Port of `git_watcher.py::_is_commit_event`.
///
/// Reacts to `HEAD`, `packed-refs`, and anything under `refs/heads/`.
/// Ignores `index`, `index.lock`, `COMMIT_EDITMSG`, `ORIG_HEAD`, `logs/*`,
/// `objects/*`, and lock files.
pub fn is_commit_event(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let norm = path.replace('\\', "/");
    let parts: Vec<&str> = norm.split('/').collect();
    let name = parts.last().copied().unwrap_or("");

    // refs/heads/<branch> — canonical commit signal. The reflog mirror
    // (.git/logs/refs/heads/...) is explicitly NOT one: a `refs` preceded by
    // `logs` never counts (a branch literally NAMED "logs" still matches
    // because it appears AFTER refs/heads/).
    for i in 0..parts.len().saturating_sub(1) {
        if parts[i] == "refs" && parts[i + 1] == "heads" {
            if parts[..i].iter().any(|p| *p == "logs") {
                return false;
            }
            return i + 2 < parts.len(); // need at least one segment after "heads"
        }
    }

    if name == "HEAD" {
        return !parts.iter().any(|p| *p == "logs");
    }

    name == "packed-refs"
}

// ═══════════════════════════════════════════════════════════════════════════
// Bounded BFS repo discovery
// ═══════════════════════════════════════════════════════════════════════════

/// Find git repos (dirs containing `.git`) under `root`.
///
/// Bounded BFS: depth ≤ `max_depth` (`root` itself = depth 0), skips
/// `SKIP_DIRS`, never descends into `.git`. Caps result at `max_repos`.
/// Returns `(repo_paths, truncated)`.
pub fn discover_git_repos(root: &Path, max_depth: usize, max_repos: usize) -> (Vec<PathBuf>, bool) {
    let mut repos = Vec::new();
    let mut truncated = false;
    let skip: HashSet<&str> = SKIP_DIRS.iter().copied().collect();

    let mut frontier: VecDeque<(PathBuf, usize)> = VecDeque::new();
    frontier.push_back((root.to_path_buf(), 0));

    while let Some((current, depth)) = frontier.pop_front() {
        let entries: Vec<_> = match std::fs::read_dir(&current) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => continue,
        };

        let has_git = entries.iter().any(|e| {
            e.file_name() == ".git" && e.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
        });

        if has_git {
            repos.push(current.clone());
            if repos.len() >= max_repos {
                if !frontier.is_empty() || depth < max_depth {
                    truncated = true;
                }
                return (repos, truncated);
            }
        }

        if depth >= max_depth {
            continue;
        }

        for entry in &entries {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == ".git" || skip.contains(name_str.as_ref()) {
                continue;
            }
            frontier.push_back((entry.path(), depth + 1));
        }
    }

    (repos, truncated)
}

// ═══════════════════════════════════════════════════════════════════════════
// WatcherHandle — uniform stop/join surface
// ═══════════════════════════════════════════════════════════════════════════

/// Handle returned by [`start_git_watching`] and [`start_watching`].
///
/// Exposes `.stop()` + `.join()` compatible with the job manager's uniform
/// watcher teardown pattern.
pub struct WatcherHandle {
    watchers: Mutex<Vec<RecommendedWatcher>>,
    debouncer: Debouncer,
    stop_flag: Arc<AtomicBool>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl WatcherHandle {
    fn new(
        watchers: Vec<RecommendedWatcher>,
        debouncer: Debouncer,
        threads: Vec<JoinHandle<()>>,
    ) -> Self {
        Self {
            watchers: Mutex::new(watchers),
            debouncer,
            stop_flag: Arc::new(AtomicBool::new(false)),
            threads: Mutex::new(threads),
        }
    }

    /// Stop the watcher: invalidate pending debounced callbacks, drop the
    /// notify watchers (which closes event channels → receiver threads exit),
    /// and join all threads.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.debouncer.cancel();
        // Dropping RecommendedWatcher stops the OS-level watch and closes
        // the channel, causing receiver threads to exit their loops.
        self.watchers.lock().unwrap().clear();
        let mut threads = self.threads.lock().unwrap();
        for h in threads.drain(..) {
            let _ = h.join();
        }
    }

    /// Join event-processing threads with a best-effort deadline (seconds).
    ///
    /// Since `stop()` already closed the watchers, threads exit quickly.
    /// `JoinHandle::join` has no native timeout in std, but with `stop()`
    /// called first the join is near-instant. If `stop()` was not called,
    /// threads are left as detached after the deadline.
    pub fn join(&self, timeout_secs: f64) {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_secs);
        let mut threads = self.threads.lock().unwrap();
        for h in threads.drain(..) {
            if Instant::now() < deadline {
                let _ = h.join();
            }
            // else: thread is detached; it will exit when the channel closes
        }
    }

    /// Whether the stop flag has been set.
    pub fn is_stopped(&self) -> bool {
        self.stop_flag.load(Ordering::SeqCst)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Git watcher
// ═══════════════════════════════════════════════════════════════════════════

/// Arm a lightweight git-ref watcher under `root` and return its handle.
///
/// Discovers the bounded set of git repos, schedules a `notify` watcher per
/// repo on `.git` (non-recursive) + `.git/refs/heads` (recursive), filters
/// events with [`is_commit_event`], and debounces to a single `on_commit`
/// callback. Port of `git_watcher.py::start_git_watching`.
pub fn start_git_watching(
    on_commit: Arc<dyn Fn() + Send + Sync + 'static>,
    root: &Path,
) -> WatcherHandle {
    let (repos, truncated) = discover_git_repos(root, DEFAULT_MAX_DEPTH, DEFAULT_MAX_REPOS);
    if truncated {
        eprintln!(
            "[watch] git watcher: repo discovery truncated at cap={} under {}; \
             some nested repos will not be watched",
            DEFAULT_MAX_REPOS,
            root.display()
        );
    }

    let debouncer = Debouncer::new(
        on_commit,
        Duration::from_secs_f64(GIT_WATCH_DEBOUNCE_SECONDS),
    );

    let mut watchers = Vec::new();
    let mut threads = Vec::new();

    for repo in &repos {
        let git_dir = repo.join(".git");
        if !git_dir.is_dir() {
            continue;
        }

        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(w) => w,
            Err(e) => {
                eprintln!(
                    "[watch] git watcher: failed to create watcher for {}: {}",
                    repo.display(),
                    e
                );
                continue;
            }
        };

        // Non-recursive on .git itself: catches HEAD / packed-refs without
        // subscribing to the whole objects/ + logs/ churn.
        if let Err(e) = watcher.watch(&git_dir, RecursiveMode::NonRecursive) {
            eprintln!(
                "[watch] git watcher: failed to watch {}/.git: {}",
                repo.display(),
                e
            );
            continue;
        }

        // Recursive on refs/heads for nested branch namespaces.
        let heads_dir = git_dir.join("refs").join("heads");
        if heads_dir.is_dir() {
            if let Err(e) = watcher.watch(&heads_dir, RecursiveMode::Recursive) {
                eprintln!(
                    "[watch] git watcher: failed to watch {}/.git/refs/heads: {}",
                    repo.display(),
                    e
                );
            }
        }

        // Clone debouncer state for the event-processing thread.
        let deb_state = Arc::clone(&debouncer.state);
        let deb_callback = Arc::clone(&debouncer.callback);
        let deb_duration = debouncer.duration;

        let handle = thread::spawn(move || {
            for res in rx {
                let event = match res {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for path in &event.paths {
                    let path_str = path.to_string_lossy();
                    if std::env::var("ORACLE_WATCH_DEBUG").is_ok() {
                        eprintln!(
                            "[watch-debug] event path={path_str} commit={}",
                            is_commit_event(&path_str)
                        );
                    }
                    if is_commit_event(&path_str) {
                        let local_deb = Debouncer {
                            state: Arc::clone(&deb_state),
                            callback: Arc::clone(&deb_callback),
                            duration: deb_duration,
                        };
                        local_deb.trigger();
                        break;
                    }
                }
            }
        });

        watchers.push(watcher);
        threads.push(handle);
    }

    WatcherHandle::new(watchers, debouncer, threads)
}

// ═══════════════════════════════════════════════════════════════════════════
// FS watcher (recursive directory watch)
// ═══════════════════════════════════════════════════════════════════════════

/// Arm a recursive filesystem watcher under `root` and return its handle.
///
/// Watches the standard `FS_WATCH_DIRS` subdirectories, filters by extension
/// (`FS_WATCH_EXTENSIONS`), debounces, and fires `on_batch_ready` with the
/// sorted, deduplicated batch. Port of `file_watcher.py::start_watching` +
/// the debouncer from `index_jobs.py::start_watcher`.
pub fn start_watching(
    on_batch_ready: Arc<dyn Fn(Vec<String>) + Send + Sync + 'static>,
    root: &Path,
) -> WatcherHandle {
    let debounce_secs = std::env::var("ORACLE_WATCH_DEBOUNCE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(FS_WATCH_DEBOUNCE_SECONDS);

    let watch_exts: HashSet<String> = FS_WATCH_EXTENSIONS.iter().map(|s| s.to_string()).collect();

    // Shared queue of changed file paths (relative, POSIX-style).
    let queue: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Debouncer callback: drain the queue, filter by extension, deduplicate,
    // sort, and fire the batch callback.
    let q2 = Arc::clone(&queue);
    let exts = Arc::new(watch_exts);
    let batch_cb = on_batch_ready;
    let debouncer = Debouncer::new(
        Arc::new(move || {
            let batch: Vec<String> = {
                let mut q = q2.lock().unwrap();
                let mut paths: Vec<String> = q.drain(..).collect();
                paths.sort();
                paths.dedup();
                paths
                    .into_iter()
                    .filter(|p| {
                        Path::new(p)
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| exts.contains(&format!(".{}", e)))
                            .unwrap_or(false)
                    })
                    .collect()
            };
            if !batch.is_empty() {
                batch_cb(batch);
            }
        }),
        Duration::from_secs(debounce_secs),
    );

    // Set up the notify watcher on the FS_WATCH_DIRS.
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[watch] fs watcher: failed to create watcher: {}", e);
            return WatcherHandle::new(Vec::new(), debouncer, Vec::new());
        }
    };

    let mut watched_dirs = 0usize;
    for dir_name in FS_WATCH_DIRS {
        let target = root.join(dir_name);
        if target.is_dir() {
            match watcher.watch(&target, RecursiveMode::Recursive) {
                Ok(()) => watched_dirs += 1,
                Err(e) => {
                    eprintln!(
                        "[watch] fs watcher: failed to watch {}/{}: {}",
                        root.display(),
                        dir_name,
                        e
                    );
                }
            }
        }
    }

    if watched_dirs == 0 {
        eprintln!(
            "[watch] fs watcher: no watch directories found under {}",
            root.display()
        );
    }

    // Event-processing thread: enqueue changed paths, debouncer fires later.
    let root_clone = root.to_path_buf();
    let q4 = Arc::clone(&queue);
    let deb_state = Arc::clone(&debouncer.state);
    let deb_callback = Arc::clone(&debouncer.callback);
    let deb_duration = debouncer.duration;

    let handle = thread::spawn(move || {
        for res in rx {
            let event = match res {
                Ok(e) => e,
                Err(_) => continue,
            };
            // Only react to file modifications and creations (not removes,
            // not directory events).
            match event.kind {
                notify::event::EventKind::Create(_) | notify::event::EventKind::Modify(_) => {}
                _ => continue,
            }
            for path in &event.paths {
                // Skip directories
                if path.is_dir() {
                    continue;
                }
                // Only enqueue files under the root
                if let Ok(rel) = path.strip_prefix(&root_clone) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    q4.lock().unwrap().push_back(rel_str);
                }
            }
            // Trigger the debounce timer
            let local_deb = Debouncer {
                state: Arc::clone(&deb_state),
                callback: Arc::clone(&deb_callback),
                duration: deb_duration,
            };
            local_deb.trigger();
        }
    });

    WatcherHandle::new(vec![watcher], debouncer, vec![handle])
}
