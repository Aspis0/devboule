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

        // A just-torn-down server (root switch) may still hold the loopback
        // socket for a moment; wait for the port to free before rebinding, or
        // serve() fails with EADDRINUSE and the readiness loop times out.
        if let Err(e) = crate::oracle::python_oracle::wait_for_oracle_port_free(stop) {
            return Err(e);
        }

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

#[cfg(test)]
mod live_test {
    //! Operator-run live test of the M2.3 seam (`--ignored`): drives the REAL
    //! production `ensure_rust_oracle_server` against the repo's REAL oracle-data
    //! index + the REAL ONNX model, then hits the session port with an HTTP
    //! client carrying the operator token — exactly as the app's reqwest client
    //! does. Requires the model symlinked/copied at `<repo>/oracle-data/models/
    //! qwen3-onnx/`. Run:
    //!   cargo test -p aspis-management --lib rust_oracle::live_test -- --ignored --nocapture

    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    #[ignore]
    fn ensure_rust_server_serves_real_index_over_http() {
        // Repo root is one level up from src-tauri.
        let root = std::path::Path::new("..")
            .canonicalize()
            .expect("canonicalize repo root");
        let model = root.join("oracle-data/models/qwen3-onnx/onnx/model.onnx");
        if !model.exists() {
            eprintln!("skipping: model missing at {}", model.display());
            return;
        }

        // Drive the actual production seam function.
        let stop = AtomicBool::new(false);
        ensure_rust_oracle_server(&root, &stop).expect("rust oracle server became ready");

        let (base_url, _port) = crate::oracle::python_oracle::oracle_session_endpoint();
        let token = crate::oracle::python_oracle::oracle_operator_token();
        let client = reqwest::blocking::Client::new();

        // /health — proves bind + auth + readiness through the app's own probe.
        let health = client
            .get(format!("{base_url}/health"))
            .header("x-oracle-auth-token", token)
            .send()
            .expect("GET /health");
        assert_eq!(health.status().as_u16(), 200, "health not 200");
        let hbody: serde_json::Value = health.json().expect("health json");
        println!("HEALTH: {hbody}");
        assert!(hbody["status"].is_string(), "health missing status");

        // Wrong token must be rejected (auth really enforced by the Rust server).
        let bad = client
            .get(format!("{base_url}/health"))
            .header("x-oracle-auth-token", "definitely-wrong")
            .send()
            .expect("GET /health bad token");
        assert_ne!(bad.status().as_u16(), 200, "wrong token must not pass");

        // /context against the REAL index (8196 lexical chunks) — proves the full
        // retrieval path through the seam, over HTTP, with the operator token.
        let ctx = client
            .get(format!("{base_url}/context"))
            .query(&[("q", "oracle server index"), ("limit", "5")])
            .header("x-oracle-auth-token", token)
            .send()
            .expect("GET /context");
        assert_eq!(ctx.status().as_u16(), 200, "context not 200");
        let cbody: serde_json::Value = ctx.json().expect("context json");
        let chunks = cbody["chunks"].as_array().expect("chunks array");
        println!("CONTEXT chunks returned: {}", chunks.len());
        if let Some(first) = chunks.first() {
            println!("  top file_source: {}", first["file_source"]);
        }
        assert!(!chunks.is_empty(), "context returned no chunks from real index");

        // /runtime — the app's READINESS GATE (get_oracle_runtime deserializes the
        // body into the app's OracleRuntime struct). Parity proof: the Rust
        // server's JSON must deserialize into that exact struct with sane values.
        let rt = client
            .get(format!("{base_url}/runtime"))
            .header("x-oracle-auth-token", token)
            .send()
            .expect("GET /runtime");
        assert_eq!(rt.status().as_u16(), 200, "runtime not 200");
        let rt_text = rt.text().expect("runtime text");
        let runtime: crate::oracle::model::OracleRuntime = serde_json::from_str(&rt_text)
            .unwrap_or_else(|e| panic!("/runtime did not match app OracleRuntime: {e}\nbody: {rt_text}"));
        println!(
            "RUNTIME: chunk_store files={} records={} vectors={} ready={}",
            runtime.chunk_store.files,
            runtime.chunk_store.records,
            runtime.chunk_store.vector_records,
            runtime.chunk_store.ready
        );
        // The real store has 8196 chunk records; deserialization + counts prove the
        // readiness gate works against the Rust server exactly as against Python.
        assert_eq!(
            runtime.chunk_store.records, 8196,
            "chunk_store.records should reflect the real sqlite chunk table"
        );

        stop_rust_oracle_server();
    }

    /// Live `/ask` test against a REAL remote LLM (operator-run). The API key is
    /// read from `ORACLE_LLM_API_KEY` in the env — NEVER hardcoded — so it never
    /// lands in the repo. Skips if the key is unset. Invoke e.g.:
    ///   ORACLE_LLM_PROVIDER=deepseek ORACLE_LLM_MODEL=deepseek-chat \
    ///   ORACLE_LLM_API_KEY=sk-... \
    ///   cargo test -p aspis-management --lib rust_oracle::live_test::ask_llm_live \
    ///     -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ask_llm_live() {
        if std::env::var("ORACLE_LLM_API_KEY").map(|k| k.is_empty()).unwrap_or(true) {
            eprintln!("skipping: ORACLE_LLM_API_KEY not set");
            return;
        }
        let root = std::path::Path::new("..")
            .canonicalize()
            .expect("canonicalize repo root");
        if !root.join("oracle-data/models/qwen3-onnx/onnx/model.onnx").exists() {
            eprintln!("skipping: model missing");
            return;
        }

        let stop = AtomicBool::new(false);
        ensure_rust_oracle_server(&root, &stop).expect("rust oracle server became ready");

        let (base_url, _port) = crate::oracle::python_oracle::oracle_session_endpoint();
        let token = crate::oracle::python_oracle::oracle_operator_token();
        let client = reqwest::blocking::Client::new();

        let resp = client
            .get(format!("{base_url}/ask"))
            .query(&[
                ("q", "What does the Oracle server do and how is an answer produced?"),
                ("limit", "6"),
            ])
            .header("x-oracle-auth-token", token)
            .timeout(std::time::Duration::from_secs(90))
            .send()
            .expect("GET /ask");
        let status = resp.status();
        let body: serde_json::Value = resp.json().expect("ask json");
        println!("ASK status: {status}");
        println!(
            "  answer_source: {}  llm_provider: {}  llm_model: {}",
            body["answer_source"], body["llm_provider"], body["llm_model"]
        );
        println!("  fallback_reason: {}", body["fallback_reason"]);
        println!("  not_found: {}", body["not_found"]);
        let answer = body["answer"].as_str().unwrap_or("");
        let shown: String = answer.chars().take(600).collect();
        println!("  answer[..600]: {shown}");

        assert_eq!(status.as_u16(), 200, "ask not 200: {body}");
        assert!(!answer.trim().is_empty(), "empty answer");
        stop_rust_oracle_server();
    }
}
