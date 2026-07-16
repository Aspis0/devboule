//! Integration tests for the `oracle-mcp` rmcp stdio binary.
//!
//! These run WITHOUT the real ONNX model: the query embedder is forced onto the
//! deterministic `hash_embed` path via `ORACLE_QUERY_EMBEDDER=hash`, and a tiny
//! Oracle data dir is seeded with hash embeddings (same recipe as
//! `engine_test.rs`). The tests assert the advertised tool set and the
//! per-tool JSON envelope *shape*; live parity against the real model is
//! covered by the orchestrator's P5 stdio validation.

use std::sync::Arc;

use oracle_core::config::OracleDataPaths;
use oracle_core::embed::{BackendChoice, EmbedderPool};
use oracle_core::query::engine::HashQueryEmbedder;
use oracle_core::store::lance::{hash_embed, LanceRow, LanceStore};
use oracle_core::store::sqlite::{FileChunk, NodeCard, SqliteStore};
use oracle_core::mcp::{OracleInner, OracleMcp};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Tool};

/// Extract the first text content block from a `CallToolResult` and parse it
/// as JSON (the handlers serialise their `serde_json::Value` to a string and
/// wrap it as text content — no output schema is advertised).
fn call_json(res: &CallToolResult) -> serde_json::Value {
    let text = res
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .expect("tool result has a text content block");
    serde_json::from_str(&text).expect("tool result text is valid JSON")
}

// ── Fixtures ───────────────────────────────────────────────────────────────

fn chunks() -> Vec<FileChunk> {
    vec![FileChunk {
        id: "src/main.rs#chunk-0000".into(),
        file_id: "src/main.rs".into(),
        chunk_index: 0,
        start_char: 0,
        end_char: 1200,
        text: "fn main() {\n    let provider = ScalewayProvider::new();\n}".into(),
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
    }]
}

fn cards() -> Vec<NodeCard> {
    vec![NodeCard {
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
    }]
}

/// Seed a minimal `oracle-data` directory at `oracle_data_root` and return its
/// `OracleDataPaths`. Mirrors `engine_test::build_engine` but routes through
/// the public `OracleDataPaths::from_root`.
async fn seed_oracle_dir(oracle_data_root: &std::path::Path) -> OracleDataPaths {
    let paths = OracleDataPaths::from_root(oracle_data_root);

    let sqlite = SqliteStore::new(&paths.metadata).unwrap();
    let chunk_vectors = LanceStore::new(&paths.chunks);
    let node_vectors = LanceStore::new(&paths.vectors);
    let _file_vectors = LanceStore::new(&paths.file_vectors);

    sqlite.replace_all_chunks(&chunks()).unwrap();
    sqlite.replace_all(&cards()).unwrap();

    let mut chunk_rows = Vec::new();
    for c in &chunks() {
        chunk_rows.push(LanceRow {
            id: c.id.clone(),
            label: c.id.clone(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector: hash_embed(&c.text, 1024),
        });
    }
    chunk_vectors.upsert(&chunk_rows).await.unwrap();

    let mut node_rows = Vec::new();
    for card in &cards() {
        node_rows.push(LanceRow {
            id: card.id.clone(),
            label: card.label.clone(),
            area: card.area.clone(),
            cluster_semantic: card.cluster_semantic.clone(),
            vector: hash_embed(&card.id, 1024),
        });
    }
    node_vectors.upsert(&node_rows).await.unwrap();

    paths
}

/// Build the `OracleMcp` server over a freshly seeded temp Oracle dir.
/// Returns the server together with the `TempDir` guard so the seeded data
/// stays alive for the whole test (dropping it would delete the stores).
async fn build_server() -> (OracleMcp, tempfile::TempDir) {
    // Force the deterministic hash query embedder so no ONNX model is needed.
    std::env::set_var("ORACLE_QUERY_EMBEDDER", "hash");

    let tmp = tempfile::tempdir().unwrap();
    // Use an absolute ORACLE_DIR so `from_root` resolves to our seeded dir.
    let oracle_dir = tmp.path().join("oracle-data");
    std::fs::create_dir_all(&oracle_dir).unwrap();
    std::env::set_var("ORACLE_DIR", oracle_dir.to_str().unwrap());

    let paths = seed_oracle_dir(&oracle_dir).await;

    // The pool is never actually loaded in hash mode; a dummy model dir is fine.
    let pool = EmbedderPool::new(BackendChoice::Ort {
        model_dir: oracle_dir.join("onnx"),
        int8: true,
    });

    let inner = OracleInner::new(paths, Arc::new(pool));
    (OracleMcp::new(Arc::new(inner)), tmp)
}

// ── Single consolidated test (one seed, one server, no intra-binary concurrency) ──

#[tokio::test]
async fn oracle_mcp_end_to_end() {
    // HashQueryEmbedder import is used to guarantee the seeded engine's
    // embedding dimension matches the bin's hash path (both 1024).
    let _ = std::mem::size_of::<HashQueryEmbedder>();

    // Build once; keep `_tmp` alive for the whole test so the seeded stores
    // are never dropped/deleted mid-test.
    let (server, _tmp) = build_server().await;

    // ── Tool list assertion ─────────────────────────────────────────────
    let specs: Vec<Tool> = server.tool_specs();
    let mut names: Vec<&str> = specs.iter().map(|t| t.name.as_ref()).collect();
    names.sort_unstable();

    assert_eq!(
        names,
        vec![
            "oracle_ask",
            "oracle_context",
            "oracle_duplicates",
            "oracle_find",
            "oracle_node",
            "oracle_similar",
        ],
        "expected exactly the 6 retrieval tools"
    );

    // Spot-check that the descriptions match the Python rail verbatim.
    let by_name: std::collections::HashMap<&str, &Tool> =
        specs.iter().map(|t| (t.name.as_ref(), t)).collect();
    assert_eq!(
        by_name["oracle_ask"].description.as_deref(),
        Some(
            "Ask the Oracle for information about the project's architecture. \
             Pre-filter with kind/language/symbols."
        )
    );
    assert_eq!(
        by_name["oracle_find"].description.as_deref(),
        Some(
            "PRECISE: find EXACT symbols (functions, structs, classes) by name. \
             Returns kind, symbol_name, signature, language, and exact line \
             ranges (line_start, line_end) — open the file at that line."
        )
    );
    assert_eq!(
        by_name["oracle_duplicates"].description.as_deref(),
        Some("List components with the same label in different areas.")
    );

    // ── oracle_find envelope shape ─────────────────────────────────────
    let args = oracle_core::mcp::FindArgs {
        query: "main".to_string(),
        kind: String::new(),
        language: String::new(),
        limit: Some(10),
    };
    let res = server
        .oracle_find(Parameters(args))
        .await
        .expect("oracle_find should succeed");
    let value = call_json(&res);

    let obj = value.as_object().expect("oracle_find returns an object");
    for key in ["query", "kind", "language", "chunks", "hint"] {
        assert!(obj.contains_key(key), "oracle_find envelope missing key {key}");
    }
    assert_eq!(obj["query"], serde_json::json!("main"));
    assert!(obj["chunks"].is_array(), "chunks must be an array");
    assert!(
        !obj["chunks"].as_array().unwrap().is_empty(),
        "expected at least one chunk"
    );
    assert!(
        obj["hint"]
            .as_str()
            .unwrap()
            .contains("line_start, line_end"),
        "hint must mention line ranges"
    );

    // ── oracle_context envelope shape ──────────────────────────────────
    let args = oracle_core::mcp::ContextArgs {
        query: "Scaleway provider".to_string(),
        limit: Some(8),
        kind: String::new(),
        language: String::new(),
        symbols: vec![],
        imports: vec![],
        module: String::new(),
    };
    let res = server
        .oracle_context(Parameters(args))
        .await
        .expect("oracle_context should succeed");
    let value = call_json(&res);

    let obj = value.as_object().expect("oracle_context returns an object");
    for key in ["query", "chunks"] {
        assert!(obj.contains_key(key), "oracle_context envelope missing key {key}");
    }
    assert_eq!(obj["query"], serde_json::json!("Scaleway provider"));
    assert!(obj["chunks"].is_array(), "chunks must be an array");
    assert!(
        !obj["chunks"].as_array().unwrap().is_empty(),
        "expected at least one chunk"
    );

    // ── oracle_node returns card ───────────────────────────────────────
    let args = oracle_core::mcp::NodeArgs {
        id: "src/main.rs".to_string(),
    };
    let res = server
        .oracle_node(Parameters(args))
        .await
        .expect("oracle_node should succeed for a seeded card");
    let value = call_json(&res);
    let obj = value.as_object().expect("oracle_node returns an object");
    assert_eq!(obj["id"], serde_json::json!("src/main.rs"));
    assert_eq!(obj["label"], serde_json::json!("main"));

    // ── oracle_similar returns entries ─────────────────────────────────
    let args = oracle_core::mcp::SimilarArgs {
        id: "src/main.rs".to_string(),
        limit: Some(5),
    };
    let res = server
        .oracle_similar(Parameters(args))
        .await
        .expect("oracle_similar should succeed");
    let value = call_json(&res);
    assert!(value.is_array(), "oracle_similar returns an array");

    // ── oracle_duplicates returns groups ───────────────────────────────
    let args = oracle_core::mcp::NoArgs {};
    let res = server
        .oracle_duplicates(Parameters(args))
        .await
        .expect("oracle_duplicates should succeed");
    let value = call_json(&res);
    assert!(value.is_array(), "oracle_duplicates returns an array");
}
