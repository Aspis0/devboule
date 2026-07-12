//! In-process Rust Oracle server lifecycle (M2.3).
//!
//! When the operator sets `oracle.engine` to `"rust"`, the app starts an
//! `oracle-core` HTTP server on the same loopback session port that the Python
//! subprocess would have used. Everything downstream (reqwest client, Tauri
//! commands, readiness probe, discovery file) is unchanged — only the server
//! behind the port differs.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use oracle_core::config::OracleDataPaths;
use oracle_core::embed::{BackendChoice, EmbedderPool};
use oracle_core::jobs::OracleIndexJobManager;
use oracle_core::server::AppState;

use tokio::sync::watch;

// ---------------------------------------------------------------------------
// Process-wide singleton slot
// ---------------------------------------------------------------------------

struct RustServer {
    shutdown: watch::Sender<bool>,
    root: std::path::PathBuf,
}

fn slot() -> &'static Mutex<Option<RustServer>> {
    static S: OnceLock<Mutex<Option<RustServer>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start the Rust oracle-core server in-process on the session port.
///
/// If a server is already running in the slot, this skips the spawn and jumps
/// straight to the readiness wait. Returns `Ok(())` once the server answers
/// the health probe, or `Err` on abort / timeout.
pub(crate) fn ensure_rust_oracle_server(root: &Path, stop: &AtomicBool) -> Result<(), String> {
    // Fast abort for a stopping supervisor.
    if stop.load(Ordering::SeqCst) {
        return Err("oracle server start aborted".into());
    }

    // Already serving — nothing to do.
    if crate::oracle::python_oracle::oracle_server_ready(root) {
        return Ok(());
    }

    // Lock the slot. Recover from poison (defensive — a panic inside a prior
    // lock hold should not block the new supervisor forever).
    let mut guard = slot().lock().unwrap_or_else(|e| e.into_inner());

    // If a server is recorded for a DIFFERENT root, it can never satisfy this
    // request (its index/paths differ). Signal it down and clear the slot so we
    // respawn below for the current root. (Mirrors the Python path's WrongRoot
    // teardown.)
    if let Some(existing) = guard.as_ref() {
        if existing.root != root {
            if let Some(old) = guard.take() {
                let _ = old.shutdown.send(true);
            }
        }
    }

    // If the slot is `None` (either empty or just cleared), build state and spawn.
    if guard.is_none() {
        let paths = OracleDataPaths::from_root(root);
        let model_dir = oracle_core::model_download::model_dir(&paths.root);
        let pool = Arc::new(EmbedderPool::new(BackendChoice::Ort {
            model_dir,
            int8: false,
        }));
        let (_base_url, port) = crate::oracle::python_oracle::oracle_session_endpoint();

        let state = Arc::new(AppState {
            sqlite_path: paths.metadata.clone(),
            vectors_path: paths.vectors.clone(),
            chunk_vectors_path: paths.chunks.clone(),
            file_vectors_path: paths.file_vectors.clone(),
            job_manager: Arc::new(OracleIndexJobManager::new()),
            embedder_pool: pool,
            operator_token: crate::oracle::python_oracle::oracle_operator_token().to_string(),
            agent_token: crate::oracle::python_oracle::oracle_agent_token().to_string(),
            server_root: root.to_string_lossy().to_string(),
            index_root: root.to_path_buf(),
            query_embedder_hash: false,
        });

        let (tx, rx) = watch::channel(false);
        tauri::async_runtime::spawn(async move {
            if let Err(e) = oracle_core::server::serve(state, port, rx).await {
                eprintln!("[rust-oracle] serve exited: {e:#}");
            }
        });

        *guard = Some(RustServer {
            shutdown: tx,
            root: root.to_path_buf(),
        });
    }

    // Drop the slot lock so we don't hold it during the readiness wait.
    drop(guard);

    // Readiness wait: poll until the server answers, with ~200ms slices.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if crate::oracle::python_oracle::oracle_server_ready(root) {
            return Ok(());
        }
        if stop.load(Ordering::SeqCst) {
            // Do NOT tear down — leave the task alive. The stopping supervisor /
            // app-exit path owns shutdown.
            return Err("oracle server start aborted".into());
        }
        if std::time::Instant::now() >= deadline {
            // The server never came up (bind failure, crashed task, etc.). Clear the
            // slot so the next supervisor tick respawns instead of being wedged forever
            // by a dead entry.
            stop_rust_oracle_server();
            return Err("rust oracle server did not become ready".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Best-effort graceful shutdown of the in-process Rust server.
///
/// Signals the spawned task to stop via the watch channel. Does NOT join or
/// block — the task ends on its own once the channel closes / the server
/// returns. Idempotent and safe to call from `Drop` / app-exit.
pub(crate) fn stop_rust_oracle_server() {
    if let Some(server) = slot().lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = server.shutdown.send(true);
    }
}
