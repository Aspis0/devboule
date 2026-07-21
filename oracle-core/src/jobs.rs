//! Oracle index job manager — single-flight background indexing with watcher
//! arming, job lifecycle, and self-healing status.
//!
//! Port of `oracle/server/index_jobs.py` (532 LOC).
//!
//! ## Threading model
//!
//! The job runner uses `std::thread` (not tokio) because the indexing pipeline
//! is sync/blocking by design. A `tokio::runtime::Runtime` is created inside
//! the worker thread for the async `LanceStore` operations (prune, index,
//! cluster refresh). The outer layer (`OracleIndexJobManager`) holds an
//! `Arc<Mutex<JobState>>` for thread-safe status reads from the server layer.
//!
//! ## Known divergences from Python
//!
//! 1. **`on_phase` callback**: the Python `index_file_chunks` accepts a
//!    structured `on_phase(phase, detail)` callback for live sub-state
//!    (GPU cooling, memory waiting). The Rust path maps string progress lines
//!    (`chunk-index start` / `batch begin` / `paused_low_memory` / …) onto
//!    job `phase` + `phase_message` instead.
//! 2. **Embed device detection**: `resolve_min_free_gb` uses `ORACLE_EMBED_DEVICE`
//!    or macOS→`"mps"`. CUDA and MPS share the low accelerator host floor.
//! 3. **CKG refresh**: ported — the Rust job manager now calls `_refresh_ckg_best_effort`
//!    which spawns a named thread running an injectable hook (`set_ckg_refresh_hook`).
//!    The app wires the closure to `crate::backend::ckg::build_ckg_graph` →
//!    `oracle_core::store::ckg::CkgStore::replace_all`. Unlike the Python path
//!    (which shells `<ASPIS_APP_BIN> ckg`), everything runs in-process here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cluster;
use crate::config::OracleDataPaths;
use crate::embed::CancelFlag;
use crate::ingest::indexer::{self, IndexStatus, IndexerConfig, TextEmbedder};
use crate::store::lance::LanceStore;
use crate::store::sqlite::SqliteStore;
use crate::watch::{self, WatcherHandle};

// ═══════════════════════════════════════════════════════════════════════════
// Configuration constants
// ═══════════════════════════════════════════════════════════════════════════

/// Accelerator min free *host* RAM floor (GB). Used on CUDA and MPS/Metal
/// where the embedding model lives on the GPU, not as a large host ORT
/// allocation. Matches `oracle/config.py::CHUNK_GPU_MIN_FREE_GB`.
const GPU_MIN_FREE_GB: f64 = 1.5;

/// CPU min free RAM floor (GB). Matches `oracle/config.py::CHUNK_MIN_FREE_GB`.
const CPU_MIN_FREE_GB: f64 = 5.0;

/// Idle min free RAM floor (GB). When `idle=True`, use the higher of this
/// and `CPU_MIN_FREE_GB`. Matches `max(CHUNK_MIN_FREE_GB, 8.0)`.
const IDLE_FLOOR_GB: f64 = 8.0;

// ═══════════════════════════════════════════════════════════════════════════
// Job status (snake_case, matching Python's status dict keys)
// ═══════════════════════════════════════════════════════════════════════════

/// Job status enum. Variant names match Python's status strings exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Idle,
    Queued,
    Running,
    Complete,
    Error,
    PausedLowMemory,
    PausedGpuTemperature,
    PausedBatchLimit,
    Interrupted,
    Cancelled,
}

/// Live job state exposed via `status()`. Field names are snake_case (the
/// HTTP layer camelizes later, matching Python's `camelize_index_status`).
#[derive(Debug, Clone, Serialize)]
pub struct JobState {
    pub status: JobStatus,
    pub root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_batches: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_temp_c: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_gb: Option<f64>,
}

impl Default for JobState {
    fn default() -> Self {
        Self {
            status: JobStatus::Idle,
            root: String::new(),
            message: None,
            started_at: None,
            finished_at: None,
            force: None,
            max_batches: None,
            idle: None,
            phase: None,
            phase_message: None,
            gpu_temp_c: None,
            free_gb: None,
        }
    }
}

/// Full status response from `OracleIndexJobManager::status()`.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub job: JobState,
    pub watcher_running: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper functions (port of top-level functions in index_jobs.py)
// ═══════════════════════════════════════════════════════════════════════════

/// UTC timestamp as ISO-8601 string. Mirrors `index_jobs.py::utc_now`.
pub fn utc_now() -> String {
    let now: DateTime<Utc> = Utc::now();
    now.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Normalize index-run parameters for manual vs auto/watch.
/// Port of `index_jobs.py::resolve_index_run_params`.
pub fn resolve_index_run_params(
    manual: bool,
    max_batches: Option<usize>,
    idle: bool,
) -> (bool, Option<usize>) {
    if manual {
        return (false, None);
    }
    (idle, max_batches)
}

/// Pick the between-batch free-system-RAM floor.
/// Port of `index_jobs.py::resolve_min_free_gb`.
///
/// - CUDA / MPS (Metal): model weights live on the accelerator, so the host
///   RAM floor matches `GPU_MIN_FREE_GB` (1.5). On macOS, sysinfo's
///   `available_memory` often under-reports (compressor / inactive pages), and
///   a 5 GB CPU floor falsely pauses with zero embeddings.
/// - CPU / unknown: 5 GB active, 8 GB idle.
pub fn resolve_min_free_gb(device: Option<&str>, idle: bool) -> f64 {
    match device {
        Some("cuda") | Some("mps") | Some("metal") => GPU_MIN_FREE_GB,
        _ => {
            if idle {
                CPU_MIN_FREE_GB.max(IDLE_FLOOR_GB)
            } else {
                CPU_MIN_FREE_GB
            }
        }
    }
}

/// Env override for the free-RAM floor. Takes precedence over
/// [`resolve_min_free_gb`] when set and parseable.
fn env_min_free_gb_override() -> Option<f64> {
    for key in ["ORACLE_CHUNK_MIN_FREE_GB", "ORACLE_CHUNK_MIN_FREE_RAM_GB"] {
        if let Ok(raw) = std::env::var(key) {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = trimmed.parse::<f64>() {
                if v.is_finite() && v >= 0.0 {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Short, human, PATH-FREE label for a live index sub-state.
/// Port of `index_jobs.py::phase_message`.
pub fn phase_message(phase: &str, detail: &serde_json::Value) -> String {
    match phase {
        "cooling_gpu" => {
            if let Some(temp) = detail.get("gpu_temp_c").and_then(|v| v.as_f64()) {
                format!("GPU cooling ({}°C), resuming…", temp as i32)
            } else {
                "GPU cooling, resuming…".to_string()
            }
        }
        "waiting_memory" => {
            if let Some(free) = detail.get("free_gb").and_then(|v| v.as_f64()) {
                format!("Waiting for memory ({:.1} GB free), resuming…", free)
            } else {
                "Waiting for memory, resuming…".to_string()
            }
        }
        _ => String::new(),
    }
}

/// Resolve the Oracle data paths for a workspace root.
pub fn default_index_root(root: Option<&Path>) -> PathBuf {
    if let Some(r) = root {
        return r.to_path_buf();
    }
    if let Ok(env_root) = std::env::var("ORACLE_INDEX_ROOT") {
        return PathBuf::from(env_root);
    }
    PathBuf::from(".")
}

/// Detect the embedding device for `resolve_min_free_gb`.
fn detect_device() -> Option<&'static str> {
    if let Ok(v) = std::env::var("ORACLE_EMBED_DEVICE") {
        let v = v.trim().to_lowercase();
        return match v.as_str() {
            "cuda" => Some("cuda"),
            "mps" => Some("mps"),
            "cpu" => Some("cpu"),
            _ => None,
        };
    }
    #[cfg(target_os = "macos")]
    {
        Some("mps")
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// OracleIndexJobManager
// ═══════════════════════════════════════════════════════════════════════════

/// Job statuses that mean "a worker is supposed to be running".
const ACTIVE_JOB_STATUSES: &[JobStatus] = &[JobStatus::Queued, JobStatus::Running];

/// Inner state protected by the mutex.
#[derive(Default)]
struct JobManagerInner {
    job: JobState,
    thread_handle: Option<JoinHandle<()>>,
    cancel_flag: Option<CancelFlag>,
    watcher: Option<Arc<WatcherHandle>>,
    watcher_mode: Option<WatcherMode>,
    /// Fire-and-forget cluster-refresh worker (Python daemon-thread parity);
    /// kept so tests and shutdown can wait for it deterministically.
    cluster_refresh_handle: Option<JoinHandle<()>>,
    /// Injected hook invoked by `_refresh_ckg_best_effort` on a named thread.
    /// Set by the app layer (src-tauri/src/oracle/rust_oracle.rs); None when
    /// no in-process CKG builder is wired (mirrors Python's ASPIS_APP_BIN
    /// no-op).
    ckg_refresh_hook: Option<Arc<dyn Fn(&Path) + Send + Sync>>,
    /// Handle for the most recent CKG-refresh thread, so tests and orderly
    /// shutdown can wait for it deterministically.
    ckg_refresh_handle: Option<JoinHandle<()>>,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherMode {
    Watch,
    Commit,
}

/// Oracle index job manager. The server layer owns this in an `Arc`.
pub struct OracleIndexJobManager {
    inner: Arc<Mutex<JobManagerInner>>,
}

impl OracleIndexJobManager {
    /// Create a new job manager (initially idle, no watcher).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(JobManagerInner::default())),
        }
    }

    // ── status() ─────────────────────────────────────────────────────────

    /// Return the current job status. Self-heals a stale running job (thread
    /// died) to `interrupted`. Mirrors `index_jobs.py::status()`.
    pub fn status(&self, root: Option<&Path>) -> StatusResponse {
        let index_root = default_index_root(root);

        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let thread_alive = inner
            .thread_handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false);

        // Self-heal: if status claims active but thread is dead → interrupted.
        if ACTIVE_JOB_STATUSES.contains(&inner.job.status) && !thread_alive {
            inner.job = JobState {
                status: JobStatus::Interrupted,
                root: if inner.job.root.is_empty() {
                    index_root.to_string_lossy().to_string()
                } else {
                    inner.job.root.clone()
                },
                message: Some(
                    "The previous index job stopped (server restart or interruption). \
                     Click Index now to resume."
                        .to_string(),
                ),
                ..Default::default()
            };
            if let Some(h) = inner.thread_handle.take() {
                let _ = h.join();
            }
        }

        let job = inner.job.clone();
        let watcher_running = inner.watcher.is_some();

        StatusResponse {
            job,
            watcher_running,
        }
    }

    // ── run_once ─────────────────────────────────────────────────────────

    /// Execute the full index pipeline synchronously. Mirrors
    /// `index_jobs.py::run_once`.
    pub fn run_once(
        &self,
        root: Option<&Path>,
        force: bool,
        max_batches: Option<usize>,
        idle: bool,
        embedder: &dyn TextEmbedder,
    ) -> Result<JobState, anyhow::Error> {
        let index_root = default_index_root(root);
        let paths = OracleDataPaths::from_root(&index_root);
        let sqlite = SqliteStore::new(&paths.metadata)?;
        let chunk_vectors = LanceStore::new(&paths.chunks);
        let file_vectors = LanceStore::new(&paths.file_vectors);

        self._set_job(JobStatus::Running, &index_root, force, max_batches, idle);

        let result = (|| -> Result<JobState, anyhow::Error> {
            // Env override wins over device-based floor so operators can tune
            // Metal/CPU hosts without a rebuild (IndexerConfig::default already
            // reads the same keys; jobs must not clobber that with resolve_*).
            let min_free_gb = env_min_free_gb_override()
                .unwrap_or_else(|| resolve_min_free_gb(detect_device(), idle));

            let inner_for_progress = std::sync::Arc::clone(&self.inner);
            let progress = move |line: &str| {
                // Map the indexer's progress lines to a live job phase. Pause lines
                // (memory / GPU cooldown) must override "embedding" so the UI stops
                // claiming it is embedding while it is actually waiting.
                let update: Option<(&'static str, Option<String>)> =
                    if line.starts_with("chunk-text-sync") {
                        Some(("scanning", Some(line.to_string())))
                    } else if line.starts_with("chunk-index low-memory retry")
                        || line.starts_with("chunk-index paused_low_memory")
                    {
                        // Prefer a concrete free_gb when the line carries it.
                        let free_gb = line
                            .split_whitespace()
                            .find_map(|tok| tok.strip_prefix("free_gb="))
                            .and_then(|v| v.parse::<f64>().ok());
                        let msg = match free_gb {
                            Some(g) => Some(format!(
                                "Waiting for memory ({g:.1} GB free), resuming…"
                            )),
                            None => Some("Waiting for memory, resuming…".to_string()),
                        };
                        Some(("waiting_memory", msg))
                    } else if line.starts_with("chunk-index gpu cooldown") {
                        Some(("cooling_gpu", None))
                    } else if line.starts_with("chunk-index start")
                        || line.starts_with("chunk-index low-memory proceeding")
                    {
                        // Start / soft pre-scan lines carry counts or free_gb —
                        // surface them so the UI is not stuck on a blank phase.
                        Some(("scanning", Some(line.to_string())))
                    } else if line.starts_with("chunk-index batch begin") {
                        Some(("embedding", Some(line.to_string())))
                    } else if let Some(rest) =
                        line.strip_prefix("chunk-index progress embedded_chunks=")
                    {
                        rest.trim().parse::<usize>().ok().map(|n| {
                            ("embedding", Some(format!("Embedding\u{2026} {n} chunks")))
                        })
                    } else {
                        None
                    };
                if let Some((phase, msg)) = update {
                    let mut inner = inner_for_progress.lock().unwrap_or_else(|e| e.into_inner());
                    inner.job.phase = Some(phase.to_string());
                    inner.job.phase_message = msg;
                }
            };

            let sync_result = indexer::sync_text_chunks(
                &index_root,
                &sqlite,
                &paths.manifest,
                100,
                force,
                Some(&progress),
            )?;
            eprintln!(
                "[jobs] sync complete: files={} chunks={} skipped={}",
                sync_result.files, sync_result.chunks, sync_result.skipped
            );

            {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(indexer::prune_excluded_chunks(
                    &index_root,
                    &sqlite,
                    &chunk_vectors,
                    &paths.manifest,
                    None,
                    None,
                ))?;
            }

            let index_result = if max_batches == Some(0) {
                indexer::IndexResult {
                    status: IndexStatus::Complete,
                    root: index_root.to_string_lossy().to_string(),
                    sqlite_path: paths.metadata.to_string_lossy().to_string(),
                    vector_path: paths.chunks.to_string_lossy().to_string(),
                    manifest_path: paths.manifest.to_string_lossy().to_string(),
                    scanned: 0,
                    pending: None,
                    processed: 0,
                    chunks: 0,
                    total_files: None,
                    total_chunks: None,
                    vector_records: None,
                    free_gb: Some(indexer::free_memory_gb()),
                    free_ram_gb: Some(indexer::free_memory_gb()),
                    gpu_temp_c: None,
                    max_gpu_temp_c: None,
                }
            } else {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(indexer::index_file_chunks(
                    &index_root,
                    &sqlite,
                    &chunk_vectors,
                    &paths.manifest,
                    embedder,
                    &self.cancel_flag(),
                    &IndexerConfig {
                        min_free_gb,
                        max_batches,
                        force,
                        ..Default::default()
                    },
                    Some(&progress),
                ))?
            };

            // Count actual Lance vectors before cluster/CKG. A "success" with
            // 0 vectors (e.g. pre-scan abort, embed never wrote) must not
            // rebuild CKG or refresh clusters as if the dense index exists.
            let vector_count = {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(chunk_vectors.count()).unwrap_or(0)
            };

            let zero_vectors_failure = max_batches != Some(0)
                && vector_count == 0
                && (index_result.processed == 0
                    || matches!(
                        index_result.status,
                        IndexStatus::Complete
                            | IndexStatus::PausedLowMemory
                            | IndexStatus::PausedGpuTemperature
                            | IndexStatus::PausedBatchLimit
                    ));

            if zero_vectors_failure {
                let zero_msg = "Dense index produced 0 vectors. Text sync may have run, \
                    but embeddings did not write. Retry Index now."
                    .to_string();
                // Keep pause statuses so the UI can show pause; Complete→Error.
                let status = match index_result.status {
                    IndexStatus::PausedLowMemory => JobStatus::PausedLowMemory,
                    IndexStatus::PausedGpuTemperature => JobStatus::PausedGpuTemperature,
                    IndexStatus::PausedBatchLimit => JobStatus::PausedBatchLimit,
                    IndexStatus::Complete => JobStatus::Error,
                };
                eprintln!(
                    "[jobs] dense index produced 0 vectors status={:?} processed={} free_gb={:?}",
                    index_result.status, index_result.processed, index_result.free_gb
                );
                let result_job = JobState {
                    status,
                    root: index_root.to_string_lossy().to_string(),
                    message: Some(zero_msg),
                    started_at: None,
                    finished_at: Some(utc_now()),
                    force: Some(force),
                    max_batches,
                    idle: Some(idle),
                    phase: None,
                    phase_message: None,
                    gpu_temp_c: index_result.gpu_temp_c,
                    free_gb: index_result.free_gb,
                };
                self._finish_job(result_job.clone());
                // No cluster refresh, no CKG rebuild — dense index is empty.
                return Ok(result_job);
            }

            let status = match index_result.status {
                IndexStatus::Complete => JobStatus::Complete,
                IndexStatus::PausedLowMemory => JobStatus::PausedLowMemory,
                IndexStatus::PausedGpuTemperature => JobStatus::PausedGpuTemperature,
                IndexStatus::PausedBatchLimit => JobStatus::PausedBatchLimit,
            };

            let result_job = JobState {
                status: status.clone(),
                root: index_root.to_string_lossy().to_string(),
                message: None,
                started_at: None,
                finished_at: Some(utc_now()),
                force: Some(force),
                max_batches,
                idle: Some(idle),
                phase: None,
                phase_message: None,
                gpu_temp_c: index_result.gpu_temp_c,
                free_gb: index_result.free_gb,
            };

            self._finish_job(result_job.clone());

            // Best-effort cluster refresh on a daemon thread.
            // Only when vectors exist (or text-only max_batches==0, handled above).
            self._refresh_clusters_best_effort(&index_root, &sqlite, &chunk_vectors, &file_vectors);

            // Best-effort CKG full-rebuild on a named thread. No-op when no hook
            // is wired (mirrors Python's ASPIS_APP_BIN no-op).
            self._refresh_ckg_best_effort(&index_root);

            Ok(result_job)
        })();

        match result {
            Ok(job) => Ok(job),
            Err(e) => {
                eprintln!(
                    "[jobs] index run failed root={}: {}",
                    index_root.display(),
                    e
                );
                let error_job = JobState {
                    status: JobStatus::Error,
                    root: index_root.to_string_lossy().to_string(),
                    message: Some(
                        "Oracle index job failed. Check the Oracle server log.".to_string(),
                    ),
                    finished_at: Some(utc_now()),
                    ..Default::default()
                };
                self._finish_job(error_job.clone());
                Err(e)
            }
        }
    }

    // ── start_background ─────────────────────────────────────────────────

    /// Start an index job in the background. Returns immediately with the
    /// job state. If a job is already running, returns its state (single-flight).
    /// Mirrors `index_jobs.py::start_background`.
    pub fn start_background<F>(
        &self,
        root: Option<&Path>,
        force: bool,
        max_batches: Option<usize>,
        idle: bool,
        embedder_factory: F,
    ) -> JobState
    where
        F: FnOnce() -> Arc<dyn TextEmbedder> + Send + 'static,
    {
        let index_root = default_index_root(root);

        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

            // Single-flight guard. Two conditions, BOTH under this one lock, so
            // there is no TOCTOU window between the check and claiming the slot:
            //   1. a live worker thread (handle not finished), OR
            //   2. an ACTIVE job status (Queued/Running/paused_*) — this covers
            //      the gap after we set Queued below but BEFORE thread_handle is
            //      stored (a second racing call would otherwise see handle==None
            //      and spawn a second concurrent indexer on the same stores).
            let thread_alive = inner
                .thread_handle
                .as_ref()
                .map(|h| !h.is_finished())
                .unwrap_or(false);
            if thread_alive || ACTIVE_JOB_STATUSES.contains(&inner.job.status) {
                return inner.job.clone();
            }
            // A finished handle from a prior run — reap it.
            if let Some(h) = inner.thread_handle.take() {
                let _ = h.join();
            }

            // Claim the slot: set Queued NOW so any racing caller bounces on the
            // status check above until this run finishes.
            inner.job = JobState {
                status: JobStatus::Queued,
                root: index_root.to_string_lossy().to_string(),
                started_at: Some(utc_now()),
                ..Default::default()
            };
            inner.cancel_flag = Some(CancelFlag::new());
        }

        let job_snapshot = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.job.clone()
        };

        // Clone the Arc<Mutex> for the background thread.
        let inner_arc = Arc::clone(&self.inner);
        let root_owned = index_root.to_path_buf();

        let handle = thread::spawn(move || {
            let embedder_arc = embedder_factory();
            let mgr = OracleIndexJobManager { inner: inner_arc };
            match mgr.run_once(
                Some(&root_owned),
                force,
                max_batches,
                idle,
                embedder_arc.as_ref(),
            ) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("[jobs] background job failed: {}", e);
                }
            }
        });

        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.thread_handle = Some(handle);
        }

        job_snapshot
    }

    // ── cancel ───────────────────────────────────────────────────────────

    /// Cancel a running job. Sets the `CancelFlag`, which the indexer checks
    /// between batches.
    pub fn cancel(&self) -> JobState {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref flag) = inner.cancel_flag {
            flag.cancel();
        }
        if ACTIVE_JOB_STATUSES.contains(&inner.job.status) {
            inner.job.status = JobStatus::Interrupted;
            inner.job.finished_at = Some(utc_now());
            inner.job.message = Some("Job cancelled by user.".to_string());
        }
        inner.job.clone()
    }

    /// Get a reference to the current cancel flag (if any).
    fn cancel_flag(&self) -> CancelFlag {
        self.inner
            .lock()
            .unwrap()
            .cancel_flag
            .clone()
            .unwrap_or_default()
    }

    // ── keepalive_active / indexing_in_progress ──────────────────────────

    /// True if a watcher is armed or a job thread is alive.
    pub fn keepalive_active(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.watcher.is_some()
            || inner
                .thread_handle
                .as_ref()
                .map(|h| !h.is_finished())
                .unwrap_or(false)
    }

    /// True only while a background index job is ACTIVELY running.
    pub fn indexing_in_progress(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .thread_handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    // ── Watcher management ───────────────────────────────────────────────

    /// Arm the auto-reindex watcher. Port of `index_jobs.py::start_watcher`.
    ///
    /// Three-phase teardown: snapshot+clear under lock → stop/join outside
    /// lock → arm new under lock.
    pub fn start_watcher(
        &self,
        root: Option<&Path>,
        mode: Option<&str>,
        on_commit: Arc<dyn Fn() + Send + Sync + 'static>,
        on_batch_ready: Arc<dyn Fn(Vec<String>) + Send + Sync + 'static>,
    ) -> serde_json::Value {
        let index_root = default_index_root(root);
        let kind = if mode == Some("commit") {
            WatcherMode::Commit
        } else {
            WatcherMode::Watch
        };

        // Phase 1: under lock, snapshot + clear old watcher.
        let old_handle = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.watcher.is_some() && inner.watcher_mode == Some(kind) {
                return serde_json::json!({
                    "status": "watching",
                    "mode": format!("{:?}", kind).to_lowercase(),
                    "root": index_root.to_string_lossy(),
                });
            }
            let old = inner.watcher.take();
            inner.watcher_mode = None;
            old
        };

        // Phase 2: stop + join old watcher OUTSIDE the lock.
        if let Some(ref handle) = old_handle {
            handle.stop();
            handle.join(5.0);
        }

        // Phase 3: under lock, arm new watcher.
        let watcher_handle = match kind {
            WatcherMode::Commit => watch::start_git_watching(on_commit, &index_root),
            WatcherMode::Watch => watch::start_watching(on_batch_ready, &index_root),
        };

        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.watcher = Some(Arc::new(watcher_handle));
            inner.watcher_mode = Some(kind);
        }

        serde_json::json!({
            "status": "watching",
            "mode": format!("{:?}", kind).to_lowercase(),
            "root": index_root.to_string_lossy(),
        })
    }

    /// Stop the watcher. Port of `index_jobs.py::stop_watcher`.
    pub fn stop_watcher(&self) -> serde_json::Value {
        let old_handle = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let old = inner.watcher.take();
            inner.watcher_mode = None;
            old
        };
        if let Some(ref handle) = old_handle {
            handle.stop();
            handle.join(5.0);
        }
        serde_json::json!({"status": "stopped"})
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    fn _set_job(
        &self,
        status: JobStatus,
        root: &Path,
        force: bool,
        max_batches: Option<usize>,
        idle: bool,
    ) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.job = JobState {
            status,
            root: root.to_string_lossy().to_string(),
            message: None,
            started_at: inner.job.started_at.clone().or_else(|| Some(utc_now())),
            finished_at: None,
            force: Some(force),
            max_batches,
            idle: Some(idle),
            phase: None,
            phase_message: None,
            gpu_temp_c: None,
            free_gb: None,
        };
    }

    fn _finish_job(&self, job: JobState) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.job = job;
        // Do NOT join the worker handle here: _finish_job runs ON the worker
        // thread — self-join is EDEADLK and poisons the mutex. The finished
        // handle is reaped by the next start_background()/status() call.
        inner.cancel_flag = None;
    }

    /// Best-effort cluster refresh on a daemon thread.
    fn _refresh_clusters_best_effort(
        &self,
        index_root: &Path,
        _sqlite: &SqliteStore,
        _chunk_vectors: &LanceStore,
        _file_vectors: &LanceStore,
    ) {
        let root = index_root.to_path_buf();
        let paths = OracleDataPaths::from_root(index_root);
        let sqlite_path = paths.metadata.clone();
        let chunk_vectors_path = paths.chunks.clone();
        let file_vectors_path = paths.file_vectors.clone();

        let handle = thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("[jobs] cluster refresh: failed to create runtime: {}", e);
                    return;
                }
            };
            rt.block_on(async {
                let sqlite = match SqliteStore::new(&sqlite_path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[jobs] cluster refresh: sqlite open failed: {}", e);
                        return;
                    }
                };
                let chunk_vectors = LanceStore::new(&chunk_vectors_path);
                let file_vectors = LanceStore::new(&file_vectors_path);
                if let Err(e) =
                    cluster::refresh_clusters(&root, &sqlite, &chunk_vectors, &file_vectors).await
                {
                    eprintln!("[jobs] cluster refresh failed: {}", e);
                }
            });
        });
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Reap a previous refresh worker before replacing the handle.
        if let Some(prev) = inner.cluster_refresh_handle.take() {
            if prev.is_finished() {
                let _ = prev.join();
            } else {
                // Still running: detach it (Python daemon-thread behavior).
                drop(prev);
            }
        }
        inner.cluster_refresh_handle = Some(handle);
    }

    /// Block until the most recent best-effort cluster refresh finishes.
    /// Used by tests and orderly shutdown; Python's daemon threads have no
    /// equivalent wait (they die with the process).
    pub fn wait_for_cluster_refresh(&self) {
        let handle = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.cluster_refresh_handle.take()
        };
        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    /// Inject the CKG-refresh hook. Called once by the app layer (rust_oracle.rs)
    /// to wire the in-process `crate::backend::ckg::build_ckg_graph` → CkgStore
    /// writer. After this, `_refresh_ckg_best_effort` will fire on a named thread
    /// after every successful index run.
    pub fn set_ckg_refresh_hook(&self, hook: Arc<dyn Fn(&Path) + Send + Sync>) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.ckg_refresh_hook = Some(hook);
    }

    /// Best-effort full CKG rebuild on a named thread, right after a successful
    /// index run. Mirrors `index_jobs.py::_refresh_ckg_best_effort`.
    ///
    /// No-op when no hook is wired (mirrors Python's `ASPIS_APP_BIN` no-op).
    /// The hook itself is responsible for catching its own errors; we ALSO wrap
    /// the call in `catch_unwind` so a panic in the hook cannot kill the
    /// process — the CKG is auxiliary and must NEVER break the vector index.
    fn _refresh_ckg_best_effort(&self, index_root: &Path) {
        let hook = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.ckg_refresh_hook.clone()
        };
        let Some(hook) = hook else {
            return;
        };
        let root = index_root.to_path_buf();
        let handle = thread::spawn(move || {
            // Named thread so it shows up in thread dumps / debug output.
            if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                hook(&root)
            })) {
                eprintln!("[jobs] CKG refresh panicked: {:?}", e);
            }
        });
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Reap a previous refresh worker before replacing the handle.
        if let Some(prev) = inner.ckg_refresh_handle.take() {
            if prev.is_finished() {
                let _ = prev.join();
            } else {
                // Still running: detach it (Python daemon-thread behavior).
                drop(prev);
            }
        }
        inner.ckg_refresh_handle = Some(handle);
    }

    /// Block until the most recent best-effort CKG refresh finishes.
    /// Used by tests and orderly shutdown.
    pub fn wait_for_ckg_refresh(&self) {
        let handle = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.ckg_refresh_handle.take()
        };
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Drive `_refresh_ckg_best_effort` directly with an injected hook that
    /// records the root it was called with. Verifies:
    ///   - the hook fires with the right root
    ///   - `wait_for_ckg_refresh` joins it
    #[test]
    fn test_ckg_refresh_hook_fires_and_joins() {
        let mgr = OracleIndexJobManager::new();
        let called_with = Arc::new(AtomicUsize::new(0));
        let hook_root = Arc::new(Mutex::new(PathBuf::new()));

        let hook = {
            let called = Arc::clone(&called_with);
            let root = Arc::clone(&hook_root);
            move |p: &Path| {
                called.fetch_add(1, Ordering::SeqCst);
                *root.lock().unwrap() = p.to_path_buf();
            }
        };

        mgr.set_ckg_refresh_hook(Arc::new(hook));

        let test_root = PathBuf::from("/tmp/ckg-test-root");
        mgr._refresh_ckg_best_effort(&test_root);

        // The hook should not have run synchronously (it's on a thread).
        assert_eq!(called_with.load(Ordering::SeqCst), 0);

        // Wait for the refresh to finish.
        mgr.wait_for_ckg_refresh();

        assert_eq!(called_with.load(Ordering::SeqCst), 1);
        assert_eq!(*hook_root.lock().unwrap(), test_root);
    }
}
