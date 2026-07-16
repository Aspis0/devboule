//! Hermetic tests for the Oracle HTTP server (axum Router).
//!
//! All tests use `tower::ServiceExt::oneshot` against the in-memory Router
//! (no network binding). Auth env vars are set per-test with a mutex guard
//! because `env` is process-global.

use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oracle_core::config::OracleDataPaths;
use oracle_core::embed::{BackendChoice, EmbedderPool};
use oracle_core::jobs::OracleIndexJobManager;
use oracle_core::server::{self, AppState};
use oracle_core::store::lance::{hash_embed, LanceRow, LanceStore};
use oracle_core::store::sqlite::{FileChunk, NodeCard, SqliteStore};
use tower::ServiceExt;

// ═══════════════════════════════════════════════════════════════════════════
// Env guard — process-global env is NOT safe for parallel tests, so we use
// a mutex to serialize env writes. Tests that care about auth tokens pass
// them through AppState fields instead.
// ═══════════════════════════════════════════════════════════════════════════

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Test fixtures — reuse engine_test.rs pattern
// ═══════════════════════════════════════════════════════════════════════════

fn test_chunks() -> Vec<FileChunk> {
    vec![
        FileChunk {
            id: "docs/architecture.md#chunk-0000".into(),
            file_id: "docs/architecture.md".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 2500,
            text: "# Oracle Architecture\n\nThe ingestion pipeline handles \
                   source code files. Scaleway GPU instances provide compute."
                .into(),
            file_sorgente: "docs/architecture.md".into(),
            ultima_modifica: "2026-01-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "text_slice".into(),
            symbol_name: "".into(),
            signature: "".into(),
            line_start: 0,
            line_end: 0,
            language: "".into(),
            symbols_used: vec![],
        },
        FileChunk {
            id: "src/main.rs#chunk-0000".into(),
            file_id: "src/main.rs".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 1200,
            text: "fn main() {\n    // Scaleway GPU provider backend\n    \
                   let provider = ScalewayProvider::new();\n}"
                .into(),
            file_sorgente: "src/main.rs".into(),
            ultima_modifica: "2026-02-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "main".into(),
            signature: "fn main()".into(),
            line_start: 1,
            line_end: 4,
            language: "rust".into(),
            symbols_used: vec!["ScalewayProvider".into()],
        },
        FileChunk {
            id: "src/api.rs#chunk-0000".into(),
            file_id: "src/api.rs".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 1500,
            text: "use crate::provider;\n\npub fn handle_request() {\n    \
                   // Cloudflare worker secret rotation logic\n}"
                .into(),
            file_sorgente: "src/api.rs".into(),
            ultima_modifica: "2026-03-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "handle_request".into(),
            signature: "pub fn handle_request()".into(),
            line_start: 1,
            line_end: 4,
            language: "rust".into(),
            symbols_used: vec!["crate::provider".into()],
        },
        FileChunk {
            id: "src/api.rs#chunk-0001".into(),
            file_id: "src/api.rs".into(),
            chunk_index: 1,
            start_char: 1500,
            end_char: 3000,
            text: "pub fn rotate_secret(provider: &str) {\n    // Rotation for \
                   cloudflare workers\n    // Uses secret management\n}"
                .into(),
            file_sorgente: "src/api.rs".into(),
            ultima_modifica: "2026-03-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "rotate_secret".into(),
            signature: "pub fn rotate_secret(provider: &str)".into(),
            line_start: 1,
            line_end: 4,
            language: "rust".into(),
            symbols_used: vec![],
        },
        FileChunk {
            id: "data/config.json#chunk-0000".into(),
            file_id: "data/config.json".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 2000,
            text: "{\n  \"pipeline\": {\n    \"name\": \"oracle-ingestion\"\n  },\n  \
                   \"query_engine\": {\n    \"lexical_weight\": 1.0\n  }\n}"
                .into(),
            file_sorgente: "data/config.json".into(),
            ultima_modifica: "2026-01-15T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "text_slice".into(),
            symbol_name: "".into(),
            signature: "".into(),
            line_start: 0,
            line_end: 0,
            language: "".into(),
            symbols_used: vec![],
        },
    ]
}

fn test_cards() -> Vec<NodeCard> {
    vec![
        NodeCard {
            id: "docs/architecture.md".into(),
            label: "architecture".into(),
            area: "documentation".into(),
            cluster_semantic: "3".into(),
            funzione_primaria: "Oracle architecture documentation".into(),
            espone_api: vec![],
            dipende_da: vec![],
            simile_a: vec![],
            tecnologie: vec!["rust".into(), "python".into()],
            file_sorgente: "docs/architecture.md".into(),
            ultima_modifica: "2026-01-01T00:00:00Z".into(),
            source: "file".into(),
            embedding_dims: 1024,
        },
        NodeCard {
            id: "src/main.rs".into(),
            label: "main".into(),
            area: "backend".into(),
            cluster_semantic: "1".into(),
            funzione_primaria: "Main entry point".into(),
            espone_api: vec![],
            dipende_da: vec!["src/provider.rs".into()],
            simile_a: vec![],
            tecnologie: vec!["rust".into()],
            file_sorgente: "src/main.rs".into(),
            ultima_modifica: "2026-02-01T00:00:00Z".into(),
            source: "file".into(),
            embedding_dims: 1024,
        },
    ]
}

struct TestWorld {
    tmp: tempfile::TempDir,
    state: Arc<AppState>,
}

impl TestWorld {
    async fn new(operator_token: &str, agent_token: &str) -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = OracleDataPaths::from_root(tmp.path());

        let sqlite = SqliteStore::new(&paths.metadata).unwrap();
        let chunk_vectors = LanceStore::new(&paths.chunks);
        let node_vectors = LanceStore::new(&paths.vectors);
        let _file_vectors = LanceStore::new(&paths.file_vectors);

        // Seed data.
        let chunks = test_chunks();
        sqlite.replace_all_chunks(&chunks).unwrap();
        let cards = test_cards();
        sqlite.replace_all(&cards).unwrap();

        // Populate chunk vectors with hash embeddings.
        {
            let chunk_rows: Vec<LanceRow> = chunks
                .iter()
                .map(|c| LanceRow {
                    id: c.id.clone(),
                    label: c.id.clone(),
                    area: "chunk".into(),
                    cluster_semantic: "0".into(),
                    vector: hash_embed(&c.text, 1024),
                })
                .collect();
            chunk_vectors.upsert(&chunk_rows).await.unwrap();

            let node_rows: Vec<LanceRow> = cards
                .iter()
                .map(|card| LanceRow {
                    id: card.id.clone(),
                    label: card.label.clone(),
                    area: card.area.clone(),
                    cluster_semantic: card.cluster_semantic.clone(),
                    vector: hash_embed(&card.id, 1024),
                })
                .collect();
            node_vectors.upsert(&node_rows).await.unwrap();
        }

        // Build a fake embedder pool (hash backend — model not loaded).
        let pool = Arc::new(EmbedderPool::new(BackendChoice::Ort {
            model_dir: tmp.path().join("fake-model"),
            int8: false,
        }));

        let state = Arc::new(AppState {
            sqlite_path: paths.metadata.clone(),
            vectors_path: paths.vectors.clone(),
            chunk_vectors_path: paths.chunks.clone(),
            file_vectors_path: paths.file_vectors.clone(),
            job_manager: Arc::new(OracleIndexJobManager::new()),
            embedder_pool: pool,
            operator_token: operator_token.to_string(),
            agent_token: agent_token.to_string(),
            server_root: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            index_root: tmp.path().to_path_buf(),
            query_embedder_hash: true,
        });

        Self { tmp, state }
    }

    fn router(&self) -> axum::Router {
        server::build_router_for_test(Arc::clone(&self.state))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_auth_no_token_configured_returns_503() {
    let world = TestWorld::new("", "").await; // No operator token.
    let req = Request::builder()
        .uri("/health")
        .header("x-oracle-auth-token", "anything")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_auth_wrong_token_returns_401() {
    let world = TestWorld::new("secret-token", "agent-token").await;
    let req = Request::builder()
        .uri("/health")
        .header("x-oracle-auth-token", "wrong-token")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_correct_operator_token_accepted() {
    let world = TestWorld::new("operator-123", "agent-456").await;
    let req = Request::builder()
        .uri("/health")
        .header("x-oracle-auth-token", "operator-123")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_auth_agent_token_on_operator_route_rejected() {
    let world = TestWorld::new("operator-123", "agent-456").await;
    let req = Request::builder()
        .uri("/health")
        .header("x-oracle-auth-token", "agent-456")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_agent_token_on_bounded_route_accepted() {
    let world = TestWorld::new("operator-123", "agent-456").await;
    let body = serde_json::json!({"query": "test", "allowed_file_ids": []});
    let req = Request::builder()
        .method("POST")
        .uri("/context-bounded")
        .header("x-oracle-auth-token", "agent-456")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    // Should NOT be 401 (might be other errors but auth passes).
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_health_shape() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .uri("/health")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert_eq!(body["auth"], "enabled");
    assert!(body["server_root"].is_string());
    assert!(body["status"].is_string());
    assert!(body["nodes"].is_number());
}

#[tokio::test]
async fn test_snapshot_keys() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .uri("/snapshot")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert!(body["status"].is_string());
    assert!(body["source"].is_string());
    assert!(body["phase"].is_string());
    assert!(body["node_count"].is_number());
    assert!(body["edge_count"].is_number());
    assert!(body["cluster_count"].is_number());
    assert!(body["duplicate_labels"].is_array());
}

#[tokio::test]
async fn test_context_post_returns_chunks() {
    let world = TestWorld::new("op", "ag").await;
    let body = serde_json::json!({"query": "Scaleway GPU", "limit": 5});
    let req = Request::builder()
        .method("POST")
        .uri("/context")
        .header("x-oracle-auth-token", "op")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert_eq!(body["query"], "Scaleway GPU");
    assert!(body["chunks"].is_array());
    let chunks = body["chunks"].as_array().unwrap();
    assert!(
        !chunks.is_empty(),
        "should return chunks for Scaleway GPU query"
    );
    // Each chunk should have retrieval flag.
    for c in chunks {
        assert!(
            c["retrieval"].is_string(),
            "chunk should have retrieval field"
        );
    }
}

#[tokio::test]
async fn test_context_bounded_empty_file_ids_returns_empty() {
    let world = TestWorld::new("op", "ag").await;
    let body = serde_json::json!({
        "query": "Scaleway GPU",
        "limit": 5,
        "allowed_file_ids": []
    });
    let req = Request::builder()
        .method("POST")
        .uri("/context-bounded")
        .header("x-oracle-auth-token", "ag")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert_eq!(body["query"], "Scaleway GPU");
    let chunks = body["chunks"].as_array().unwrap();
    assert!(
        chunks.is_empty(),
        "empty allowed_file_ids should yield grounded-empty"
    );
}

#[tokio::test]
async fn test_embed_bounded_too_many_texts_returns_400() {
    let world = TestWorld::new("op", "ag").await;
    let texts: Vec<String> = (0..65).map(|i| format!("text{}", i)).collect();
    let body = serde_json::json!({"texts": texts});
    let req = Request::builder()
        .method("POST")
        .uri("/embed-bounded")
        .header("x-oracle-auth-token", "ag")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_embed_bounded_empty_texts_returns_400() {
    let world = TestWorld::new("op", "ag").await;
    let body = serde_json::json!({"texts": []});
    let req = Request::builder()
        .method("POST")
        .uri("/embed-bounded")
        .header("x-oracle-auth-token", "ag")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_node_404_on_miss() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .uri("/node/nonexistent-file.txt")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_node_200_on_hit() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .uri("/node/src/main.rs")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert_eq!(body["id"], "src/main.rs");
    assert_eq!(body["label"], "main");
}

#[tokio::test]
async fn test_duplicate_labels_shape() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .uri("/duplicate-labels")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert!(body.is_array());
}

#[tokio::test]
async fn test_discovery_file_write_and_refresh() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join(".oracle-server.json");

    // Write.
    server::write_discovery_file(&path, "http://127.0.0.1:9999", "agent-tok", "/workspace")
        .unwrap();
    assert!(path.exists());

    let text = std::fs::read_to_string(&path).unwrap();
    let data: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(data["baseUrl"], "http://127.0.0.1:9999");
    assert_eq!(data["authToken"], "agent-tok");
    assert_eq!(data["indexRoot"], "/workspace");
    assert!(data["pid"].is_number());
    assert!(data["heartbeatAt"].is_string());
    assert!(data["updatedAt"].is_string());

    // Verify agent token is used, NOT operator token.
    assert_ne!(data["authToken"], "operator-token");

    // Refresh bumps heartbeatAt.
    let old_heartbeat = data["heartbeatAt"].as_str().unwrap().to_string();
    std::thread::sleep(std::time::Duration::from_millis(50));
    server::refresh_discovery_heartbeat(&path).unwrap();
    let text2 = std::fs::read_to_string(&path).unwrap();
    let data2: serde_json::Value = serde_json::from_str(&text2).unwrap();
    let new_heartbeat = data2["heartbeatAt"].as_str().unwrap().to_string();
    assert!(new_heartbeat >= old_heartbeat, "heartbeat should be bumped");

    // Delete.
    server::delete_discovery_file(&path).unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn test_index_status_camelized_keys() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .uri("/index/status")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert!(body["job"].is_object());
    assert!(body["watcherRunning"].is_boolean());
    assert!(body["index"].is_object());
    // Spot-check camelized keys in the index sub-object.
    let idx = &body["index"];
    // Should have camelCase keys, NOT snake_case.
    assert!(
        idx.get("indexedFiles").is_some() || idx.get("expectedFiles").is_some(),
        "index status should have camelized keys, got: {}",
        idx
    );
    assert!(
        idx.get("indexed_files").is_none(),
        "should NOT have snake_case 'indexed_files'"
    );
}

// ── Max-recall regression tests ─────────────────────────────────────────────

/// Path-traversal hardening: an operator caller must NOT be able to index a
/// directory outside the workspace (review angle-3 BLOCKER).
#[tokio::test]
async fn test_index_run_rejects_root_outside_workspace() {
    let world = TestWorld::new("op", "ag").await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.py"), "print('x')\n").unwrap();

    let uri = format!(
        "/index/run?background=true&root={}",
        outside.path().to_string_lossy()
    );
    let req = Request::builder()
        .method("POST")
        .uri(&uri)
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// The workspace root itself (and None) must still be accepted.
#[tokio::test]
async fn test_index_run_accepts_workspace_root() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .method("POST")
        .uri("/index/run?background=true")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// /runtime reports chunk-store readiness (consumed by the app's live probe).
#[tokio::test]
async fn test_runtime_endpoint_shape() {
    let world = TestWorld::new("op", "ag").await;
    let req = Request::builder()
        .uri("/runtime")
        .header("x-oracle-auth-token", "op")
        .body(Body::empty())
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert!(body.get("ready").is_some());
    assert!(body["chunk_store"].get("ready").is_some());
    assert_eq!(body["ollama"]["server"], "removed");
}

/// Discovery file carries the AGENT token (never operator) and heartbeatAt.
#[tokio::test]
async fn test_discovery_payload_uses_agent_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".oracle-server.json");
    oracle_core::server::write_discovery_file(
        &path,
        "http://127.0.0.1:12345",
        "AGENT_TOKEN_XYZ",
        "/ws",
    )
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["authToken"], "AGENT_TOKEN_XYZ");
    assert!(v.get("heartbeatAt").is_some());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "discovery file must be owner-only");
    }
}

// ── Bounded-filter cap and filter-forwarding tests ─────────────────────────

/// symbols exceeding the cap (MAX_BOUNDED_FILTER_ENTRIES == 64) must return 400
/// mentioning the cap — regression for M3-P12c unbounded-list hardening.
#[tokio::test]
async fn test_context_bounded_too_many_symbols_returns_400() {
    let world = TestWorld::new("op", "ag").await;
    let symbols: Vec<String> = (0..65).map(|i| format!("sym{}", i)).collect();
    let body = serde_json::json!({
        "query": "Scaleway GPU",
        "limit": 5,
        "allowed_file_ids": [],
        "symbols": symbols
    });
    let req = Request::builder()
        .method("POST")
        .uri("/context-bounded")
        .header("x-oracle-auth-token", "ag")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    let detail = body["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("symbols") && detail.contains("64"),
        "detail should mention 'symbols' and the cap 64, got: {}",
        detail
    );
}

/// A bounded request with kind/language/symbols/module filters present must be
/// accepted (200) and return the standard chunks-shaped response.
#[tokio::test]
async fn test_context_bounded_filters_accepted() {
    let world = TestWorld::new("op", "ag").await;
    let body = serde_json::json!({
        "query": "Scaleway GPU",
        "limit": 5,
        "allowed_file_ids": [],
        "kind": "function",
        "language": "rust",
        "symbols": ["main", "handle_request"],
        "module": "src"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/context-bounded")
        .header("x-oracle-auth-token", "ag")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert_eq!(body["query"], "Scaleway GPU");
    assert!(body["chunks"].is_array());
}

/// group_by_file:true must be forwarded without error (200) with the standard
/// chunks-shaped response.
#[tokio::test]
async fn test_ask_bounded_group_by_file_accepted() {
    let world = TestWorld::new("op", "ag").await;
    let body = serde_json::json!({
        "query": "Scaleway GPU",
        "limit": 5,
        "allowed_file_ids": [],
        "group_by_file": true
    });
    let req = Request::builder()
        .method("POST")
        .uri("/ask-bounded")
        .header("x-oracle-auth-token", "ag")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = world.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .map(|b| serde_json::from_slice(&b).unwrap())
        .unwrap();
    assert_eq!(body["query"], "Scaleway GPU");
    // /ask-bounded returns the ASK envelope (answer/citations), not the
    // /context-bounded chunks shape.
    assert!(body["answer"].is_string());
    assert!(body["citations"].is_array());
}
