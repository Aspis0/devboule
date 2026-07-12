//! Oracle HTTP server — axum port of `oracle/server/routes.py`.
//!
//! Binds to `127.0.0.1:<port>` (loopback only) with two auth tiers:
//!   - **Operator** (`ORACLE_AUTH_TOKEN`): full access to every endpoint.
//!   - **Agent** (`ORACLE_AGENT_AUTH_TOKEN`): bounded endpoints only
//!     (`/*-bounded`, `/embed-bounded`).
//!
//! The JSON contract is byte-identical to the Python server so the existing
//! `src-tauri` deserializers and `aspis_mcp.py` HTTP thin-client continue to
//! work unchanged.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::answer::LlmAnswerer;
use crate::config::OracleDataPaths;
use crate::embed::{CancelFlag, EmbedderPool};
use crate::ingest::indexer::{self, chunk_index_status, TextEmbedder};
use crate::jobs::{OracleIndexJobManager, StatusResponse};
use crate::query::engine::{
    AskResponse, HashQueryEmbedder, HealthResponse, QueryEmbedder, QueryEngine,
};
use crate::store::lance::{hash_embed, LanceStore};
use crate::store::manifest::{self, load_manifest};
use crate::store::sqlite::SqliteStore;

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

const MAX_BOUNDED_LIMIT: usize = 100;
const MAX_BOUNDED_ALLOWED_IDS: usize = 10_000;
const MAX_EMBED_TEXTS: usize = 64;

// ═══════════════════════════════════════════════════════════════════════════
// AppState
// ═══════════════════════════════════════════════════════════════════════════

/// Shared application state threaded through every axum handler.
pub struct AppState {
    /// Paths to stores (constructed on demand since stores aren't Clone).
    pub sqlite_path: PathBuf,
    pub vectors_path: PathBuf,
    pub chunk_vectors_path: PathBuf,
    pub file_vectors_path: PathBuf,
    /// Index job manager.
    pub job_manager: Arc<OracleIndexJobManager>,
    /// Embedder pool for query embedding and /embed-bounded.
    pub embedder_pool: Arc<EmbedderPool>,
    /// Operator auth token (from `ORACLE_AUTH_TOKEN`).
    pub operator_token: String,
    /// Agent auth token (from `ORACLE_AGENT_AUTH_TOKEN`).
    pub agent_token: String,
    /// Canonical server root (resolved cwd, verbatim-stripped).
    pub server_root: String,
    /// Workspace root for index operations.
    pub index_root: PathBuf,
    /// Use the deterministic hash query embedder instead of the model.
    /// Seeded from `ORACLE_QUERY_EMBEDDER=hash` at construction; tests set it
    /// directly (env vars are process-global and race under parallel tests).
    pub query_embedder_hash: bool,
}

impl AppState {
    /// Build a `QueryEngine` on demand (cheap to construct).
    fn engine(&self) -> QueryEngine {
        let sqlite = SqliteStore::new(&self.sqlite_path).expect("sqlite open");
        let vectors = LanceStore::new(&self.vectors_path);
        let chunk_vectors = LanceStore::new(&self.chunk_vectors_path);
        let file_vectors = LanceStore::new(&self.file_vectors_path);
        QueryEngine::new(sqlite, vectors, Some(chunk_vectors), Some(file_vectors))
    }

    /// Build a fresh SqliteStore on demand.
    fn sqlite(&self) -> SqliteStore {
        SqliteStore::new(&self.sqlite_path).expect("sqlite open")
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Auth — constant-time comparison, fail-closed
// ═══════════════════════════════════════════════════════════════════════════

/// Constant-time string comparison. An empty `expected` never matches.
fn ct_eq(provided: &str, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    let pb = provided.as_bytes();
    let eb = expected.as_bytes();
    if pb.len() != eb.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in pb.iter().zip(eb.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn header_token(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// Fail-closed operator gate.
fn require_operator(headers: &HeaderMap, state: &AppState) -> Result<(), (StatusCode, String)> {
    if state.operator_token.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Oracle server authentication is not configured. \
             Set ORACLE_AUTH_TOKEN before serving Oracle endpoints."
                .to_string(),
        ));
    }
    let provided = header_token(headers, "x-oracle-auth-token");
    if !ct_eq(&provided, &state.operator_token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Oracle server authentication failed".to_string(),
        ));
    }
    Ok(())
}

/// Fail-closed operator-OR-agent gate (bounded routes).
fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), (StatusCode, String)> {
    if state.operator_token.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Oracle server authentication is not configured. \
             Set ORACLE_AUTH_TOKEN before serving Oracle endpoints."
                .to_string(),
        ));
    }
    let provided = header_token(headers, "x-oracle-auth-token");
    if ct_eq(&provided, &state.operator_token) || ct_eq(&provided, &state.agent_token) {
        return Ok(());
    }
    Err((
        StatusCode::UNAUTHORIZED,
        "Oracle server authentication failed".to_string(),
    ))
}

// ═══════════════════════════════════════════════════════════════════════════
// Auth error response helper
// ═══════════════════════════════════════════════════════════════════════════

fn auth_error(err: (StatusCode, String)) -> Response {
    let (status, detail) = err;
    (status, Json(serde_json::json!({"detail": detail}))).into_response()
}

// ═══════════════════════════════════════════════════════════════════════════
// Request / response types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct AskPayload {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ContextPayload {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BoundedPayload {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    allowed_file_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct EmbedBoundedPayload {
    #[serde(default)]
    texts: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct IndexRunQuery {
    root: Option<String>,
    force: Option<bool>,
    max_batches: Option<usize>,
    idle: Option<bool>,
    background: Option<bool>,
    manual: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct IndexFilesQuery {
    root: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IndexWatchStartQuery {
    root: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IndexSyncQuery {
    root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IndexStatusQuery {
    root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SimilarQuery {
    limit: Option<usize>,
}

// ═══════════════════════════════════════════════════════════════════════════
// PoolQueryEmbedder — adapter from EmbedderPool to QueryEmbedder
// ═══════════════════════════════════════════════════════════════════════════

/// Wraps `Arc<EmbedderPool>` to implement `QueryEmbedder` (not `TextEmbedder`).
struct PoolQueryEmbedder {
    pool: Arc<EmbedderPool>,
    use_hash: bool,
}

impl QueryEmbedder for PoolQueryEmbedder {
    fn embed_query(&self, text: &str, dims: usize) -> Result<Vec<f32>> {
        if self.use_hash || crate::config::query_embedder_is_hash() {
            if crate::config::require_real_embedder() {
                anyhow::bail!(
                    "Qwen embedding model is unavailable. \
                     Run Oracle doctor / check the runtime install."
                );
            }
            let prefixed = crate::ingest::retrieval_text::query_embedding_text(text, None);
            return Ok(hash_embed(&prefixed, dims));
        }
        let prefixed = crate::ingest::retrieval_text::query_embedding_text(text, None);
        let texts = vec![prefixed];
        let cancel = CancelFlag::new();
        let vectors = self.pool.embed(&texts, 1, &cancel)?;
        vectors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty embedding result"))
    }
}

/// Wrapper to adapt `Arc<EmbedderPool>` to `TextEmbedder` (for jobs module).
struct PoolTextEmbedder(Arc<EmbedderPool>);

impl TextEmbedder for PoolTextEmbedder {
    fn embed(
        &self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        self.0.embed(texts, batch_size, cancel)
    }
}

/// Wrapper to adapt `HashQueryEmbedder` to `TextEmbedder` (for watcher callbacks).
struct HashTextEmbedder;

impl TextEmbedder for HashTextEmbedder {
    fn embed(
        &self,
        texts: &[String],
        _batch_size: usize,
        _cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        let dims = crate::config::EMBED_DIMS;
        Ok(texts
            .iter()
            .map(|t| {
                let prefixed = crate::ingest::retrieval_text::query_embedding_text(t, None);
                Ok(hash_embed(&prefixed, dims))
            })
            .collect::<Result<Vec<Vec<f32>>>>()?)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Camelize index status — exact port of Python's camelize_index_status
// ═══════════════════════════════════════════════════════════════════════════

fn camelize_index_status(status: &serde_json::Value) -> serde_json::Value {
    let aliases: HashMap<&str, &str> = [
        ("expected_files", "expectedFiles"),
        ("indexed_files", "indexedFiles"),
        ("pending_files", "pendingFiles"),
        ("stale_files", "staleFiles"),
        ("sqlite_chunk_files", "sqliteChunkFiles"),
        ("sqlite_chunks", "sqliteChunks"),
        ("vector_records", "vectorRecords"),
        ("first_pending", "firstPending"),
        ("first_stale", "firstStale"),
        ("free_gb", "freeRamGb"),
    ]
    .iter()
    .copied()
    .collect();

    match status {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = aliases.get(k.as_str()).copied().unwrap_or(k.as_str());
                out.insert(key.to_string(), v.clone());
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /health
// ═══════════════════════════════════════════════════════════════════════════

async fn health_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let health = engine.health().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
        )
            .into_response()
    })?;
    let mut payload = serde_json::to_value(&health).unwrap_or_default();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "server_root".to_string(),
            serde_json::Value::String(state.server_root.clone()),
        );
        obj.insert(
            "auth".to_string(),
            serde_json::Value::String(if state.operator_token.is_empty() {
                "disabled".to_string()
            } else {
                "enabled".to_string()
            }),
        );
    }
    Ok(Json(payload))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /snapshot
// ═══════════════════════════════════════════════════════════════════════════

async fn snapshot_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let snapshot = tokio::task::spawn_blocking(move || {
        let nodes = engine.sqlite.all_nodes()?;
        let dup_groups = engine.duplicates()?;
        let dup_labels: Vec<serde_json::Value> = dup_groups
            .iter()
            .filter_map(|ids| {
                engine.sqlite.get_node(&ids[0]).ok().flatten().map(|card| {
                    serde_json::json!({
                        "label": card.label,
                        "node_ids": ids,
                    })
                })
            })
            .collect();
        let cluster_count: usize = nodes
            .iter()
            .map(|n| n.cluster_semantic.clone())
            .collect::<HashSet<_>>()
            .len();
        let status_str = if nodes.is_empty() { "empty" } else { "ready" };
        Ok::<_, anyhow::Error>(serde_json::json!({
            "status": status_str,
            "source": "rust-oracle",
            "phase": "phase1",
            "node_count": nodes.len(),
            "edge_count": 0,
            "cluster_count": cluster_count,
            "duplicate_labels": dup_labels,
        }))
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "task join error".to_string(),
        )
            .into_response()
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
        )
            .into_response()
    })?;
    Ok(Json(snapshot))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST+GET /ask
// ═══════════════════════════════════════════════════════════════════════════

async fn ask_get_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let q = params.get("q").cloned().unwrap_or_default();
    let limit: usize = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    run_ask(&state, &q, limit).await
}

async fn ask_post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<AskPayload>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let q = payload.query.or(payload.q).unwrap_or_default();
    let limit = payload.limit.unwrap_or(5).max(1) as usize;
    run_ask(&state, &q, limit).await
}

async fn run_ask(
    state: &Arc<AppState>,
    q: &str,
    limit: usize,
) -> Result<Json<serde_json::Value>, Response> {
    let engine = state.engine();
    let pool = Arc::clone(&state.embedder_pool);
    let q = q.to_string();
    let result = {
        let embedder = PoolQueryEmbedder {
            pool,
            use_hash: state.query_embedder_hash,
        };
        let answerer = LlmAnswerer::from_env();
        engine
            .ask(
                &q,
                limit,
                &embedder,
                Some(&answerer),
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                false,
            )
            .await
    }
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
        )
            .into_response()
    })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET+POST /context
// ═══════════════════════════════════════════════════════════════════════════

async fn context_get_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let q = params.get("q").cloned().unwrap_or_default();
    let limit: usize = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let limit = limit.clamp(1, MAX_BOUNDED_LIMIT);
    run_context(&state, &q, limit, None, false).await
}

async fn context_post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ContextPayload>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let q = payload.query.or(payload.q).unwrap_or_default();
    let limit = payload
        .limit
        .unwrap_or(8)
        .max(1)
        .min(MAX_BOUNDED_LIMIT as i64) as usize;
    let prefer_lexical = state.job_manager.indexing_in_progress();
    run_context(&state, &q, limit, None, prefer_lexical).await
}

async fn run_context(
    state: &Arc<AppState>,
    q: &str,
    limit: usize,
    allowed: Option<HashSet<String>>,
    prefer_lexical: bool,
) -> Result<Json<serde_json::Value>, Response> {
    let engine = state.engine();
    let pool = Arc::clone(&state.embedder_pool);
    let q2 = q.to_string();
    let result = {
        let embedder = PoolQueryEmbedder {
            pool,
            use_hash: state.query_embedder_hash,
        };
        engine
            .context(
                &q2,
                limit,
                &embedder,
                allowed.as_ref(),
                prefer_lexical,
                None,
                None,
                None,
                None,
                None,
            )
            .await
    }
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
        )
            .into_response()
    })?;
    Ok(Json(serde_json::json!({
        "query": q,
        "chunks": result,
    })))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST /context-bounded
// ═══════════════════════════════════════════════════════════════════════════

async fn context_bounded_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<BoundedPayload>,
) -> Result<Json<serde_json::Value>, Response> {
    require_auth(&headers, &state).map_err(auth_error)?;
    let (q, limit, allowed) = parse_bounded_payload(payload, 8)?;
    let prefer_lexical = state.job_manager.indexing_in_progress();
    run_context(&state, &q, limit, Some(allowed), prefer_lexical).await
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST /ask-bounded
// ═══════════════════════════════════════════════════════════════════════════

async fn ask_bounded_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<BoundedPayload>,
) -> Result<Json<serde_json::Value>, Response> {
    require_auth(&headers, &state).map_err(auth_error)?;
    let (q, limit, allowed) = parse_bounded_payload(payload, 5)?;
    let prefer_lexical = state.job_manager.indexing_in_progress();

    let engine = state.engine();
    let pool = Arc::clone(&state.embedder_pool);
    let q = q.to_string();
    let result = {
        let embedder = PoolQueryEmbedder {
            pool,
            use_hash: state.query_embedder_hash,
        };
        let answerer = LlmAnswerer::from_env();
        engine
            .ask(
                &q,
                limit,
                &embedder,
                Some(&answerer),
                Some(&allowed),
                prefer_lexical,
                None,
                None,
                None,
                None,
                None,
                false,
            )
            .await
    }
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
        )
            .into_response()
    })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST /embed-bounded
// ═══════════════════════════════════════════════════════════════════════════

async fn embed_bounded_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<EmbedBoundedPayload>,
) -> Result<Json<serde_json::Value>, Response> {
    require_auth(&headers, &state).map_err(auth_error)?;
    let texts = match payload.texts {
        Some(t) if !t.is_empty() => t,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"detail": "texts must be a non-empty list"})),
            )
                .into_response());
        }
    };
    if texts.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"detail": "texts must be a non-empty list"})),
        )
            .into_response());
    }
    if texts.len() > MAX_EMBED_TEXTS {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"detail": "too many texts (max 64)"})),
        )
            .into_response());
    }
    if state.job_manager.indexing_in_progress() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"detail": "embedder busy (indexing); use BM25-only"})),
        )
            .into_response());
    }
    let pool = Arc::clone(&state.embedder_pool);
    let cancel = CancelFlag::new();
    let result = tokio::task::spawn_blocking(move || pool.embed(&texts, 1, &cancel))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "task join error".to_string(),
            )
                .into_response()
        })?
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "embedder error".to_string(),
            )
                .into_response()
        })?;
    let dims = crate::config::EMBED_DIMS;
    Ok(Json(serde_json::json!({
        "embeddings": result,
        "dims": dims,
    })))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /node/{id}
// ═══════════════════════════════════════════════════════════════════════════

async fn node_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(node_id): AxPath<String>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let result = tokio::task::spawn_blocking(move || engine.node(&node_id))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "task join error".to_string(),
            )
                .into_response()
        })?
        .map_err(|_| (StatusCode::NOT_FOUND, "Node not found".to_string()).into_response())?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /similar/{id}
// ═══════════════════════════════════════════════════════════════════════════

async fn similar_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(node_id): AxPath<String>,
    Query(params): Query<SimilarQuery>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let limit = params.limit.unwrap_or(5);
    let engine = state.engine();
    let result = engine.similar(&node_id, limit).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
        )
            .into_response()
    })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /clusters
// ═══════════════════════════════════════════════════════════════════════════

async fn clusters_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let result = tokio::task::spawn_blocking(move || engine.clusters_response())
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "task join error".to_string(),
            )
                .into_response()
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
            )
                .into_response()
        })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /cluster/{id}/members
// ═══════════════════════════════════════════════════════════════════════════

async fn cluster_members_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(cluster_id): AxPath<i64>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let result = tokio::task::spawn_blocking(move || engine.cluster_members(cluster_id))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "task join error".to_string(),
            )
                .into_response()
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
            )
                .into_response()
        })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /cluster/{name}
// ═══════════════════════════════════════════════════════════════════════════

async fn cluster_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(name): AxPath<String>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let result = tokio::task::spawn_blocking(move || engine.cluster(&name))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "task join error".to_string(),
            )
                .into_response()
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
            )
                .into_response()
        })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /area/{name}
// ═══════════════════════════════════════════════════════════════════════════

async fn area_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxPath(name): AxPath<String>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let result = tokio::task::spawn_blocking(move || engine.area(&name))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "task join error".to_string(),
            )
                .into_response()
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
            )
                .into_response()
        })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /duplicates
// ═══════════════════════════════════════════════════════════════════════════

async fn duplicates_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let result = tokio::task::spawn_blocking(move || engine.duplicates())
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "task join error".to_string(),
            )
                .into_response()
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
            )
                .into_response()
        })?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /duplicate-labels
// ═══════════════════════════════════════════════════════════════════════════

async fn duplicate_labels_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine();
    let result = tokio::task::spawn_blocking(move || {
        let dup_groups = engine.duplicates()?;
        let labels: Vec<serde_json::Value> = dup_groups
            .iter()
            .filter_map(|ids| {
                engine.sqlite.get_node(&ids[0]).ok().flatten().map(|card| {
                    serde_json::json!({
                        "label": card.label,
                        "node_ids": ids,
                    })
                })
            })
            .collect();
        Ok::<_, anyhow::Error>(labels)
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "task join error".to_string(),
        )
            .into_response()
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"detail": format!("engine error: {e:#}")})),
        )
            .into_response()
    })?;
    Ok(Json(serde_json::Value::Array(result)))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /index/status
// ═══════════════════════════════════════════════════════════════════════════

async fn index_status_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<IndexStatusQuery>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let root = params.root.as_deref().map(Path::new);
    let job_status = state.job_manager.status(root);
    let watcher_running = job_status.watcher_running;
    let mut job_val = serde_json::to_value(&job_status.job).unwrap_or_default();
    if let Some(obj) = job_val.as_object_mut() {
        if let Some(pm) = obj.remove("phase_message") {
            obj.insert("phaseMessage".to_string(), pm);
        }
    }

    let index_root = crate::jobs::default_index_root(root);
    let sqlite_path = state.sqlite_path.clone();
    let chunk_vectors_path = state.chunk_vectors_path.clone();
    let manifest_path = OracleDataPaths::from_root(&index_root).manifest;

    let index_snap = {
        let sqlite = SqliteStore::new(&sqlite_path).ok();
        let chunk_vectors = LanceStore::new(&chunk_vectors_path);
        if let Some(sqlite) = sqlite {
            chunk_index_status(&index_root, &sqlite, &chunk_vectors, &manifest_path)
                .await
                .ok()
        } else {
            None
        }
    };

    let index_camelized = index_snap
        .map(|snap| {
            let raw = serde_json::to_value(&snap).unwrap_or_default();
            camelize_index_status(&raw)
        })
        .unwrap_or(serde_json::Value::Null);

    Ok(Json(serde_json::json!({
        "job": job_val,
        "watcherRunning": watcher_running,
        "index": index_camelized,
    })))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /index/files
// ═══════════════════════════════════════════════════════════════════════════

async fn index_files_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<IndexFilesQuery>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let index_root = params
        .root
        .as_deref()
        .map(Path::new)
        .map(|r| crate::jobs::default_index_root(Some(r)))
        .unwrap_or_else(|| state.index_root.clone());
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let offset = params.offset.unwrap_or(0);
    let filter = params.filter.unwrap_or_default();
    let manifest_path = OracleDataPaths::from_root(&index_root).manifest;

    let result = tokio::task::spawn_blocking(move || {
        let mut manifest = load_manifest(&manifest_path);
        let manifest_files = manifest::manifest_files_for_root(&mut manifest, &index_root, false)
            .cloned()
            .unwrap_or_default();
        let needle = filter.trim().to_lowercase();
        let mut file_ids: Vec<String> = manifest_files.keys().cloned().collect();
        file_ids.sort();
        if !needle.is_empty() {
            file_ids = file_ids
                .into_iter()
                .filter(|id| id.to_lowercase().contains(&needle))
                .collect();
        }
        let total = file_ids.len();
        let page: Vec<String> = file_ids.into_iter().skip(offset).take(limit).collect();
        serde_json::json!({
            "files": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "task join error".to_string(),
        )
            .into_response()
    })?;
    Ok(Json(result))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST /index/sync
// ═══════════════════════════════════════════════════════════════════════════

async fn index_sync_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<IndexSyncQuery>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let index_root = params
        .root
        .as_deref()
        .map(Path::new)
        .map(|r| crate::jobs::default_index_root(Some(r)))
        .unwrap_or_else(|| state.index_root.clone());
    let paths = OracleDataPaths::from_root(&index_root);
    let sqlite_path = paths.metadata.clone();
    let manifest_path = paths.manifest.clone();
    let result = tokio::task::spawn_blocking(move || {
        let sqlite = SqliteStore::new(&sqlite_path)?;
        indexer::sync_text_chunks(&index_root, &sqlite, &manifest_path, 100, false, None)
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "task join error".to_string(),
        )
            .into_response()
    })?
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "sync error".to_string()).into_response())?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST /index/run
// ═══════════════════════════════════════════════════════════════════════════

async fn index_run_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<IndexRunQuery>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let root = params.root.as_deref().map(Path::new);
    let force = params.force.unwrap_or(false);
    let max_batches = params.max_batches;
    let idle = params.idle.unwrap_or(true);
    let background = params.background.unwrap_or(true);
    let manual = params.manual.unwrap_or(false);

    let (effective_idle, effective_max_batches) =
        crate::jobs::resolve_index_run_params(manual, max_batches, idle);

    if background {
        let pool = Arc::clone(&state.embedder_pool);
        let job_state = state.job_manager.start_background(
            root,
            force,
            effective_max_batches,
            effective_idle,
            move || Arc::new(PoolTextEmbedder(pool)) as Arc<dyn TextEmbedder>,
        );
        return Ok(Json(serde_json::to_value(job_state).unwrap_or_default()));
    }

    let index_root = crate::jobs::default_index_root(root);
    let paths = OracleDataPaths::from_root(&index_root);
    let pool = Arc::clone(&state.embedder_pool);
    let job_manager = Arc::clone(&state.job_manager);

    let embedder = PoolTextEmbedder(pool);
    let result = tokio::task::spawn_blocking(move || {
        job_manager.run_once(
            Some(&index_root),
            force,
            effective_max_batches,
            effective_idle,
            &embedder,
        )
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "task join error".to_string(),
        )
            .into_response()
    })?
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "index error".to_string()).into_response())?;
    Ok(Json(serde_json::to_value(result).unwrap_or_default()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST /index/watch/start
// ═══════════════════════════════════════════════════════════════════════════

async fn index_watch_start_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<IndexWatchStartQuery>,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let root = params.root.as_deref().map(Path::new);
    let mode = params.mode.as_deref();
    let index_root = crate::jobs::default_index_root(root);

    let job_mgr = Arc::clone(&state.job_manager);
    let jr = index_root.clone();
    let on_commit: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = job_mgr.start_background(Some(&jr), false, None, true, || {
            Arc::new(HashTextEmbedder) as Arc<dyn TextEmbedder>
        });
    });

    let job_mgr2 = Arc::clone(&state.job_manager);
    let jr2 = index_root.clone();
    let on_batch_ready: Arc<dyn Fn(Vec<String>) + Send + Sync> = Arc::new(move |_paths| {
        let _ = job_mgr2.start_background(Some(&jr2), false, Some(1), true, || {
            Arc::new(HashTextEmbedder) as Arc<dyn TextEmbedder>
        });
    });

    let result = state
        .job_manager
        .start_watcher(root, mode, on_commit, on_batch_ready);
    Ok(Json(result))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: POST /index/watch/stop
// ═══════════════════════════════════════════════════════════════════════════

async fn index_watch_stop_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let result = state.job_manager.stop_watcher();
    Ok(Json(result))
}

// ═══════════════════════════════════════════════════════════════════════════
// Bounded payload parser — port of parse_bounded_payload
// ═══════════════════════════════════════════════════════════════════════════

fn parse_bounded_payload(
    payload: BoundedPayload,
    default_limit: usize,
) -> Result<(String, usize, HashSet<String>), Response> {
    let q = payload.query.or(payload.q).unwrap_or_default();

    let limit = match payload.limit {
        None => default_limit,
        Some(v) => {
            if v < 1 {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({"detail": "limit must be an integer."})),
                )
                    .into_response());
            }
            v as usize
        }
    };
    let limit = limit.clamp(1, MAX_BOUNDED_LIMIT);

    let allowed = match payload.allowed_file_ids {
        None => HashSet::new(),
        Some(ids) => {
            if ids.len() > MAX_BOUNDED_ALLOWED_IDS {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({"detail": format!(
                        "allowed_file_ids exceeds the maximum of {} entries.",
                        MAX_BOUNDED_ALLOWED_IDS
                    )})),
                )
                    .into_response());
            }
            ids.into_iter().filter(|s| !s.trim().is_empty()).collect()
        }
    };

    Ok((q, limit, allowed))
}

// ═══════════════════════════════════════════════════════════════════════════
// Discovery file writer
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, Deserialize)]
struct DiscoveryPayload {
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "authToken")]
    auth_token: String,
    #[serde(rename = "indexRoot")]
    index_root: String,
    pid: u32,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    #[serde(rename = "heartbeatAt")]
    heartbeat_at: String,
}

/// Write the `.oracle-server.json` discovery file with owner-only perms.
pub fn write_discovery_file(
    path: &Path,
    base_url: &str,
    agent_token: &str,
    index_root: &str,
) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let payload = DiscoveryPayload {
        base_url: base_url.to_string(),
        auth_token: agent_token.to_string(),
        index_root: index_root.to_string(),
        pid: std::process::id(),
        updated_at: now.clone(),
        heartbeat_at: now,
    };
    let text = serde_json::to_string_pretty(&payload)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Refresh only the heartbeatAt field.
pub fn refresh_discovery_heartbeat(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path)?;
    let mut payload: DiscoveryPayload = serde_json::from_str(&text)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    payload.heartbeat_at = now.clone();
    payload.updated_at = now;
    let out = serde_json::to_string_pretty(&payload)?;
    std::fs::write(path, out)?;
    Ok(())
}

/// Delete the discovery file.
pub fn delete_discovery_file(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Spawn a background heartbeat task refreshing every `interval_secs`.
pub fn spawn_heartbeat(discovery_path: PathBuf, interval_secs: u64) -> watch::Sender<bool> {
    let (stop_tx, mut stop_rx) = watch::channel(false);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = refresh_discovery_heartbeat(&discovery_path) {
                        eprintln!("[server] heartbeat refresh failed: {}", e);
                    }
                }
                _ = stop_rx.changed() => {
                    break;
                }
            }
        }
    });
    stop_tx
}

// ═══════════════════════════════════════════════════════════════════════════
// Router builder
// ═══════════════════════════════════════════════════════════════════════════

fn build_router(state: Arc<AppState>) -> Router {
    let operator_routes = Router::new()
        .route("/health", get(health_handler))
        .route("/snapshot", get(snapshot_handler))
        .route("/ask", get(ask_get_handler).post(ask_post_handler))
        .route(
            "/context",
            get(context_get_handler).post(context_post_handler),
        )
        .route("/node/{*id}", get(node_handler))
        .route("/similar/{*id}", get(similar_handler))
        .route("/clusters", get(clusters_handler))
        .route("/cluster/{id}/members", get(cluster_members_handler))
        .route("/cluster/{name}", get(cluster_handler))
        .route("/area/{name}", get(area_handler))
        .route("/duplicates", get(duplicates_handler))
        .route("/duplicate-labels", get(duplicate_labels_handler))
        .route("/index/status", get(index_status_handler))
        .route("/index/files", get(index_files_handler))
        .route("/index/sync", post(index_sync_handler))
        .route("/index/run", post(index_run_handler))
        .route("/index/watch/start", post(index_watch_start_handler))
        .route("/index/watch/stop", post(index_watch_stop_handler));

    let bounded_routes = Router::new()
        .route("/context-bounded", post(context_bounded_handler))
        .route("/ask-bounded", post(ask_bounded_handler))
        .route("/embed-bounded", post(embed_bounded_handler));

    Router::new()
        .merge(operator_routes)
        .merge(bounded_routes)
        .with_state(state)
}

// ═══════════════════════════════════════════════════════════════════════════
// Public serve() entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Bind to `127.0.0.1:<port>` and return the axum serve future.
pub async fn serve(
    state: Arc<AppState>,
    port: u16,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("bind 127.0.0.1:{}: {}", port, e))?;

    let server = axum::serve(listener, router);
    let graceful = server.with_graceful_shutdown(async move {
        let _ = shutdown.changed().await;
    });

    graceful
        .await
        .map_err(|e| anyhow::anyhow!("server error: {}", e))
}

// ═══════════════════════════════════════════════════════════════════════════
// Public test helper
// ═══════════════════════════════════════════════════════════════════════════

/// Build the full Router from the given state. Public for integration tests.
pub fn build_router_for_test(state: Arc<AppState>) -> Router {
    build_router(state)
}
