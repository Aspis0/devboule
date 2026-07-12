//! Server-layer end-to-end with the REAL ONNX pool (operator-run, `--ignored`).
//!
//! `server_test.rs` drives the router hermetically with `query_embedder_hash =
//! true` (deterministic hash, no model). This test instead flips it to `false`
//! and puts a REAL `EmbedderPool` in `AppState`, so an actual HTTP request goes
//! all the way through: auth -> handler -> `spawn_blocking` -> real query embed
//! -> LanceDB search -> JSON body. It proves the loopback HTTP surface (the one
//! `aspis_mcp` talks to) works against the real model, not just the in-process
//! `QueryEngine` (covered by `e2e_real_onnx_test.rs`).
//!
//! Requires the local model at `models/qwen3-onnx/`. Run with:
//!   cargo test --test e2e_real_onnx_server_test -- --ignored --nocapture

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oracle_core::config::OracleDataPaths;
use oracle_core::embed::{BackendChoice, CancelFlag, EmbedderPool};
use oracle_core::ingest::indexer::{self, IndexerConfig};
use oracle_core::jobs::OracleIndexJobManager;
use oracle_core::server::{self, AppState};
use oracle_core::store::lance::LanceStore;
use oracle_core::store::sqlite::SqliteStore;
use std::path::PathBuf;
use tower::ServiceExt;

fn model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/qwen3-onnx")
}

/// Same three-domain corpus as the in-process e2e; queries below share zero
/// literal tokens with their target file, so a win is a DENSE win.
const CORPUS: &[(&str, &str)] = &[
    (
        "billing.md",
        "# Charges\n\nCharges are collected from each customer's saved card at \
         the close of the cycle. Invoices enumerate every line item and the \
         total amount owed.\n",
    ),
    (
        "astronomy.md",
        "# Sky\n\nOur natural satellite completes one revolution around the \
         planet about every twenty-seven days, held in place by gravity.\n",
    ),
    (
        "cooking.md",
        "# Starter\n\nA sourdough culture must be fed with flour and water each \
         day until it foams and doubles, ready for the oven.\n",
    ),
];

/// GET /context?q=..&limit=10 with the operator token, returning the parsed
/// JSON body and the HTTP status.
async fn get_context(
    router: axum::Router,
    query: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let uri = format!("/context?q={}&limit=10", query.replace(' ', "%20"));
    let req = Request::builder()
        .uri(uri)
        .header("x-oracle-auth-token", token)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

fn top_file(body: &serde_json::Value) -> &str {
    body["chunks"][0]["file_source"].as_str().unwrap_or("")
}

#[tokio::test]
#[ignore]
async fn real_onnx_server_context_endpoint() {
    let model = model_dir();
    if !model.join("tokenizer.json").exists() {
        eprintln!("skipping: no local model at {}", model.display());
        return;
    }

    // ── World: corpus + canonical store paths ────────────────────────────
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    for (name, body) in CORPUS {
        std::fs::write(root.join(name), body).unwrap();
    }
    let paths = OracleDataPaths::from_root(&root);
    std::fs::create_dir_all(&paths.root).unwrap();

    // ── Index with the REAL pool into the server's canonical paths ───────
    let pool = EmbedderPool::new(BackendChoice::Ort {
        model_dir: model,
        int8: false,
    });
    let cancel = CancelFlag::new();
    let cfg = IndexerConfig {
        min_free_gb: 0.0,
        max_gpu_temp_c: None,
        ..Default::default()
    };
    {
        let sqlite = SqliteStore::new(&paths.metadata).unwrap();
        let chunk_vectors = LanceStore::new(&paths.chunks);
        indexer::index_file_chunks(
            &root,
            &sqlite,
            &chunk_vectors,
            &paths.manifest,
            &pool,
            &cancel,
            &cfg,
            None,
        )
        .await
        .unwrap();
        assert!(sqlite.chunk_count().unwrap() >= 3);
    }

    // ── AppState with the REAL pool and hash DISABLED ────────────────────
    let state = Arc::new(AppState {
        sqlite_path: paths.metadata.clone(),
        vectors_path: paths.vectors.clone(),
        chunk_vectors_path: paths.chunks.clone(),
        file_vectors_path: paths.file_vectors.clone(),
        job_manager: Arc::new(OracleIndexJobManager::new()),
        embedder_pool: Arc::new(pool),
        operator_token: "op-tok".into(),
        agent_token: "ag-tok".into(),
        server_root: root.to_string_lossy().to_string(),
        index_root: root.clone(),
        query_embedder_hash: false, // <-- real model, the whole point
    });

    // Auth still enforced: no token -> not 200.
    let (unauth, _) = get_context(
        server::build_router_for_test(Arc::clone(&state)),
        "anything",
        "wrong-token",
    )
    .await;
    assert_ne!(unauth, StatusCode::OK, "wrong token must not pass");

    // Query A -> billing.md, through the full HTTP stack.
    let (sa, a) = get_context(
        server::build_router_for_test(Arc::clone(&state)),
        "how does the platform handle payments and monetary transactions from subscribers",
        "op-tok",
    )
    .await;
    assert_eq!(sa, StatusCode::OK, "body: {a}");
    assert!(a["chunks"].as_array().map(|c| !c.is_empty()).unwrap_or(false));
    println!("server A top: {}", top_file(&a));
    assert!(
        top_file(&a).contains("billing"),
        "expected billing.md via HTTP, got {}",
        top_file(&a)
    );

    // Query B -> astronomy.md: discriminates, not a constant winner.
    let (sb, b) = get_context(
        server::build_router_for_test(Arc::clone(&state)),
        "movement of celestial objects through outer space",
        "op-tok",
    )
    .await;
    assert_eq!(sb, StatusCode::OK, "body: {b}");
    println!("server B top: {}", top_file(&b));
    assert!(
        top_file(&b).contains("astronomy"),
        "expected astronomy.md via HTTP, got {}",
        top_file(&b)
    );
}
