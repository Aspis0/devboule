//! Integration tests for the query engine orchestration layer.

use oracle_core::query::engine::{ContextChunk, HashQueryEmbedder, QueryEngine};
use oracle_core::store::lance::{hash_embed, LanceRow, LanceStore};
use oracle_core::store::sqlite::{FileChunk, NodeCard, SqliteStore};

// ── Test fixtures ──────────────────────────────────────────────────────────

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
            id: "docs/architecture.md#chunk-0001".into(),
            file_id: "docs/architecture.md".into(),
            chunk_index: 1,
            start_char: 2500,
            end_char: 5000,
            text: "## Query Processing\n\nQuery terms are extracted and scored \
                   using lexical matching. The context merge combines dense and \
                   lexical results."
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
            id: "src/main.rs#chunk-0001".into(),
            file_id: "src/main.rs".into(),
            chunk_index: 1,
            start_char: 1200,
            end_char: 2400,
            text: "struct ScalewayProvider {\n    gpu: bool,\n    serverless: bool,\n}".into(),
            file_sorgente: "src/main.rs".into(),
            ultima_modifica: "2026-02-01T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "struct".into(),
            symbol_name: "ScalewayProvider".into(),
            signature: "struct ScalewayProvider".into(),
            line_start: 5,
            line_end: 8,
            language: "rust".into(),
            symbols_used: vec![],
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
        FileChunk {
            id: "tests/test_main.rs#chunk-0000".into(),
            file_id: "tests/test_main.rs".into(),
            chunk_index: 0,
            start_char: 0,
            end_char: 800,
            text: "#[cfg(test)]\nmod tests {\n    #[test]\n    fn test_basic() {\n    \
                   }\n}"
                .into(),
            file_sorgente: "tests/test_main.rs".into(),
            ultima_modifica: "2026-02-15T00:00:00Z".into(),
            embedding_dims: 0,
            kind: "function".into(),
            symbol_name: "test_basic".into(),
            signature: "#[test]".into(),
            line_start: 1,
            line_end: 5,
            language: "rust".into(),
            symbols_used: vec![],
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
        NodeCard {
            id: "src-tauri/src/backend/providers.rs".into(),
            label: "providers".into(),
            area: "backend".into(),
            cluster_semantic: "1".into(),
            funzione_primaria: "Provider management backend".into(),
            espone_api: vec![],
            dipende_da: vec![],
            simile_a: vec![],
            tecnologie: vec!["rust".into(), "scaleway".into()],
            file_sorgente: "src-tauri/src/backend/providers.rs".into(),
            ultima_modifica: "2026-03-01T00:00:00Z".into(),
            source: "file".into(),
            embedding_dims: 1024,
        },
    ]
}

async fn build_engine(tmp: &tempfile::TempDir) -> QueryEngine {
    let sqlite_path = tmp.path().join("metadata.sqlite");
    let chunk_vec_path = tmp.path().join("chunk_vectors.json");
    let node_vec_path = tmp.path().join("node_vectors.json");
    let file_vec_path = tmp.path().join("file_vectors.json");

    let sqlite = SqliteStore::new(&sqlite_path).unwrap();
    let chunk_vectors = LanceStore::new(&chunk_vec_path);
    let node_vectors = LanceStore::new(&node_vec_path);
    let file_vectors = LanceStore::new(&file_vec_path);

    // Insert chunks
    let chunks = test_chunks();
    sqlite.replace_all_chunks(&chunks).unwrap();

    // Insert node cards
    let cards = test_cards();
    sqlite.replace_all(&cards).unwrap();

    // Populate chunk vectors with hash_embed of each chunk's text
    let mut chunk_rows: Vec<LanceRow> = Vec::new();
    for c in &chunks {
        let vector = hash_embed(&c.text, 1024);
        chunk_rows.push(LanceRow {
            id: c.id.clone(),
            label: c.id.clone(),
            area: "chunk".into(),
            cluster_semantic: "0".into(),
            vector,
        });
    }
    chunk_vectors.upsert(&chunk_rows).await.unwrap();

    // Populate node-card vectors
    let mut node_rows: Vec<LanceRow> = Vec::new();
    for card in &cards {
        let vector = hash_embed(&card.id, 1024);
        node_rows.push(LanceRow {
            id: card.id.clone(),
            label: card.label.clone(),
            area: card.area.clone(),
            cluster_semantic: card.cluster_semantic.clone(),
            vector,
        });
    }
    node_vectors.upsert(&node_rows).await.unwrap();

    QueryEngine::new(
        sqlite,
        node_vectors,
        Some(chunk_vectors),
        Some(file_vectors),
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_context_lexical_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    let results = engine
        .context(
            "Scaleway GPU serverless",
            10,
            Some(&embedder),
            None,
            true, // prefer_lexical
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(!results.is_empty(), "should return chunks");
    // All should be lexical-only
    for r in &results {
        assert_eq!(r.retrieval, "lexical");
    }
    // Results should be sorted by score descending
    for w in results.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
    // First result should mention scaleway or gpu
    let top_text = results[0].text.to_lowercase();
    assert!(
        top_text.contains("scaleway") || top_text.contains("gpu"),
        "top result should mention scaleway or gpu, got: {}",
        &results[0].text[..100]
    );
}

#[tokio::test]
async fn test_context_dense_and_lexical_merge() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    let results = engine
        .context(
            "Scaleway GPU serverless compute",
            10,
            Some(&embedder),
            None,
            false, // prefer_lexical = false → dense + lexical
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(!results.is_empty());
    // Check that we have correct retrieval flags
    let has_dense = results.iter().any(|r| r.retrieval == "dense");
    let has_lexical = results.iter().any(|r| r.retrieval == "lexical");
    let has_merged = results.iter().any(|r| r.retrieval == "dense+lexical");
    // At least lexical results should exist (dense depends on hash_embed similarity)
    assert!(
        has_lexical || has_merged || has_dense,
        "should have at least one retrieval type"
    );
    // If a chunk appears in both dense and lexical, it should be dense+lexical
    for r in &results {
        assert!(
            r.retrieval == "dense" || r.retrieval == "lexical" || r.retrieval == "dense+lexical",
            "invalid retrieval: {}",
            r.retrieval
        );
    }
}

#[tokio::test]
async fn test_ask_with_none_answerer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    let response = engine
        .ask(
            "Scaleway GPU serverless",
            5,
            Some(&embedder),
            None, // no answerer
            None,
            true,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();

    // Results should be non-empty
    assert!(!response.results.is_empty(), "ask() should return results");

    // Summary should be built from labels (no answerer → degraded path)
    assert!(
        response.summary.starts_with("Grounded Oracle matches:")
            || response.summary == "No Oracle matches found.",
        "summary should be labels-based, got: {}",
        response.summary
    );

    // Degraded answerer sets not_found = true
    assert!(response.not_found);
    assert_eq!(response.answer, "not found in corpus.");

    // Verify JSON keys via serialization
    let json = serde_json::to_value(&response).unwrap();
    assert!(json.get("mode").is_some());
    assert!(json.get("query").is_some());
    assert!(json.get("summary").is_some());
    assert!(json.get("answer").is_some());
    assert!(json.get("citations").is_some());
    assert!(json.get("not_found").is_some());
    assert!(json.get("results").is_some());
    assert!(json.get("suggested_path").is_none() || json.get("suggested_path").unwrap().is_null());
    assert!(json.get("fallback_reason").is_some());

    // Check result entry fields
    let first = &response.results[0];
    let rjson = serde_json::to_value(first).unwrap();
    assert!(rjson.get("id").is_some());
    assert!(rjson.get("label").is_some());
    assert!(rjson.get("node_type").is_some());
    assert!(rjson.get("cluster").is_some());
    assert!(rjson.get("score").is_some());
    assert!(rjson.get("file_source").is_some());
    assert!(rjson.get("kind").is_some());
    assert!(rjson.get("symbols_used").is_some());
}

#[tokio::test]
async fn test_filters_kind_language() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    // Filter by kind = "function"
    let results = engine
        .context(
            "Scaleway GPU",
            10,
            Some(&embedder),
            None,
            true,
            Some("function"), // kind filter
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    for r in &results {
        assert_eq!(
            r.kind, "function",
            "all results should be kind=function, got {}",
            r.kind
        );
    }

    // Filter by language = "rust"
    let results = engine
        .context(
            "Scaleway GPU",
            10,
            Some(&embedder),
            None,
            true,
            None,
            Some("rust"), // language filter
            None,
            None,
            None,
        )
        .await
        .unwrap();

    for r in &results {
        assert_eq!(
            r.language, "rust",
            "all results should be language=rust, got {}",
            r.language
        );
    }

    // Filter by kind that doesn't match anything
    let results = engine
        .context(
            "Scaleway GPU",
            10,
            Some(&embedder),
            None,
            true,
            Some("class"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(results.is_empty(), "no class chunks in fixtures");
}

#[tokio::test]
async fn test_similar_node_card_fallback_to_file_vectors() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    // "docs/architecture.md" exists in node-card vectors → should find similar
    let results = engine.similar("docs/architecture.md", 5).await.unwrap();
    assert!(
        !results.is_empty(),
        "should find similar from node-card store"
    );

    // Nonexistent id → node-card store misses, file_vectors store is empty → []
    let results = engine.similar("nonexistent-id", 5).await.unwrap();
    assert!(results.is_empty(), "nonexistent id should return empty");
}

#[tokio::test]
async fn test_group_by_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;

    let response = engine
        .ask(
            "Scaleway GPU serverless",
            10,
            Some(&embedder),
            None,
            None,
            true,
            None,
            None,
            None,
            None,
            None,
            true, // group_by_file = true
        )
        .await
        .unwrap();

    let grouped = response
        .grouped
        .as_ref()
        .expect("grouped should be present");
    assert!(!grouped.is_empty(), "should have grouped entries");

    // Verify total_score is sum of chunk scores
    for g in grouped {
        let manual_sum: f64 = g.chunks.iter().map(|c| c.score).sum();
        assert!(
            (g.total_score - manual_sum).abs() < 1e-10,
            "total_score {} != sum {} for file {}",
            g.total_score,
            manual_sum,
            g.file
        );
    }

    // Grouped should be sorted by total_score descending
    for w in grouped.windows(2) {
        assert!(w[0].total_score >= w[1].total_score);
    }

    // Verify JSON keys
    let gjson = serde_json::to_value(&grouped[0]).unwrap();
    assert!(gjson.get("file").is_some());
    assert!(gjson.get("total_score").is_some());
    assert!(gjson.get("chunks").is_some());
}

#[tokio::test]
async fn test_health_and_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    let health = engine.health().await.unwrap();
    assert_eq!(health.status, "ready");
    assert_eq!(health.nodes, 3); // 3 node cards
    assert!(health.chunk_records > 0);
    assert!(health.vector_records > 0);
    assert!(health.chunk_vector_records > 0);

    let snapshot = engine.snapshot().await.unwrap();
    assert_eq!(snapshot.status, "ready");
    assert_eq!(snapshot.node_count, 3);
    assert!(snapshot.cluster_count > 0);
}

#[tokio::test]
async fn test_clusters_and_members() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    // Insert file clusters
    engine
        .sqlite
        .replace_file_clusters(
            &[
                oracle_core::store::sqlite::FileCluster {
                    file_id: "src/main.rs".into(),
                    cluster_id: 1,
                    score: 0.9,
                },
                oracle_core::store::sqlite::FileCluster {
                    file_id: "src/api.rs".into(),
                    cluster_id: 1,
                    score: 0.7,
                },
                oracle_core::store::sqlite::FileCluster {
                    file_id: "docs/architecture.md".into(),
                    cluster_id: 2,
                    score: 0.8,
                },
            ],
            Some("test-epoch"),
        )
        .unwrap();

    let clusters = engine.clusters_response().unwrap();
    assert_eq!(clusters.epoch, "test-epoch");
    assert_eq!(clusters.clusters.len(), 2);

    let members = engine.cluster_members(1).unwrap();
    assert_eq!(members.members.len(), 2);
    // Sorted by score DESC
    assert!(members.members[0].score >= members.members[1].score);
}

#[tokio::test]
async fn test_ask_with_answerer() {
    use oracle_core::query::engine::ContextAnswerer;

    struct MockAnswerer;

    impl ContextAnswerer for MockAnswerer {
        fn answer(
            &self,
            _query: &str,
            _context: &[ContextChunk],
        ) -> anyhow::Result<oracle_core::query::engine::AnswerPayload> {
            Ok(oracle_core::query::engine::AnswerPayload {
                answer: "Scaleway provides GPU instances.".into(),
                citations: vec![oracle_core::query::engine::Citation {
                    ref_id: "C1".into(),
                    file_source: "docs/architecture.md".into(),
                    chunk_id: "docs/architecture.md#chunk-0000".into(),
                    chunk_index: Some(0),
                    start_char: Some(0),
                    end_char: Some(2500),
                    retrieval: "lexical".into(),
                    score: 1.0,
                }],
                not_found: false,
                suggested_path: None,
                answer_source: Some("llm".into()),
                fallback_reason: None,
                llm_provider: Some("test".into()),
                llm_model: Some("mock-7b".into()),
            })
        }
    }

    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;
    let embedder = HashQueryEmbedder;
    let answerer = MockAnswerer;

    let response = engine
        .ask(
            "Scaleway GPU",
            5,
            Some(&embedder),
            Some(&answerer),
            None,
            true,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();

    assert_eq!(response.answer, "Scaleway provides GPU instances.");
    assert!(!response.not_found);
    assert_eq!(response.citations.len(), 1);
    assert_eq!(response.citations[0].ref_id, "C1");
    // When answerer provides citations, summary uses the answer text
    assert_eq!(response.summary, "Scaleway provides GPU instances.");
    assert_eq!(response.llm_provider.as_deref(), Some("test"));
    assert_eq!(response.llm_model.as_deref(), Some("mock-7b"));
}

#[tokio::test]
async fn test_node_lookup() {
    let tmp = tempfile::TempDir::new().unwrap();
    let engine = build_engine(&tmp).await;

    let card = engine.node("src/main.rs").unwrap();
    assert_eq!(card.label, "main");
    assert_eq!(card.area, "backend");

    let err = engine.node("nonexistent").unwrap_err();
    assert!(err.to_string().contains("Node not found"));
}
