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
use std::sync::Arc;

use anyhow::Result;
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
use crate::jobs::OracleIndexJobManager;
use crate::query::engine::{
    QueryEmbedder, QueryEngine,
};
use crate::store::lance::{hash_embed, LanceStore};
use crate::store::manifest::{self, load_manifest};
use crate::store::sqlite::SqliteStore;

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

const MAX_BOUNDED_LIMIT: usize = 100;
const MAX_BOUNDED_ALLOWED_IDS: usize = 10_000;
const MAX_BOUNDED_EMBED_TEXTS: usize = 64;
// M3-P12c: cap on bounded filter lists (symbols/imports). Kept small since
// the Rust engine applies these as per-chunk membership checks; a huge list
// is almost certainly a client bug, so surface it as a 400 rather than
// silently truncating.
const MAX_BOUNDED_FILTER_ENTRIES: usize = 64;

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
    ///
    /// Fallible: a corrupt/locked/unreadable metadata.sqlite must surface as a
    /// clean 500, never panic the handler task.
    fn engine(&self) -> Result<QueryEngine> {
        let sqlite = SqliteStore::new(&self.sqlite_path)?;
        let vectors = LanceStore::new(&self.vectors_path);
        let chunk_vectors = LanceStore::new(&self.chunk_vectors_path);
        let file_vectors = LanceStore::new(&self.file_vectors_path);
        Ok(QueryEngine::new(
            sqlite,
            vectors,
            Some(chunk_vectors),
            Some(file_vectors),
        ))
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

/// Map any internal error to a clean 500 JSON body (never a panic/reset).
fn internal_error<E: std::fmt::Display>(e: E) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"detail": format!("{e}")})),
    )
        .into_response()
}

/// Authorize an index-write `root` parameter against the server's workspace.
///
/// Hardening BEYOND the Python server (which resolves any `root` verbatim):
/// an index write walks + embeds an entire tree into the shared stores, so a
/// caller-supplied `root` outside the workspace would exfiltrate arbitrary
/// filesystem content into the queryable index. The app always indexes its
/// own workspace, so we require the requested root to be the workspace root
/// or a descendant of it. `None` → the workspace root (the common case).
fn authorize_index_root(requested: Option<&str>, state: &AppState) -> Result<PathBuf, Response> {
    let workspace = std::fs::canonicalize(&state.index_root).unwrap_or(state.index_root.clone());
    let Some(req) = requested.filter(|s| !s.trim().is_empty()) else {
        return Ok(state.index_root.clone());
    };
    let resolved = std::fs::canonicalize(req).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"detail": format!("root path unusable: {e}")})),
        )
            .into_response()
    })?;
    if resolved == workspace || resolved.starts_with(&workspace) {
        Ok(resolved)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "detail": "root is outside the Oracle workspace and cannot be indexed."
            })),
        )
            .into_response())
    }
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
    // M3-P12c: bounded filters forwarded to the engine. serde default None
    // keeps old clients working; absent filters are silently ignored.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    symbols: Option<Vec<String>>,
    #[serde(default)]
    imports: Option<Vec<String>>,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    group_by_file: Option<bool>,
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
    let engine = state.engine().map_err(internal_error)?;
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
// Handler: GET /runtime — retrieval-runtime readiness (verify_runtime.py)
// ═══════════════════════════════════════════════════════════════════════════

async fn runtime_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;

    let vectors_path = state.vectors_path.clone();
    let chunk_vectors_path = state.chunk_vectors_path.clone();
    let sqlite_path = state.sqlite_path.clone();
    let manifest_path = OracleDataPaths::from_root(&state.index_root).manifest;

    let vectors = LanceStore::new(&vectors_path);
    let chunk_vectors = LanceStore::new(&chunk_vectors_path);
    let vector_records = vectors.count().await.map_err(internal_error)?;
    let chunk_vector_records = chunk_vectors.count().await.map_err(internal_error)?;

    let (chunk_files, chunk_records) = tokio::task::spawn_blocking(move || {
        let sqlite = SqliteStore::new(&sqlite_path)?;
        Ok::<_, anyhow::Error>((sqlite.chunk_file_count()?, sqlite.chunk_count()?))
    })
    .await
    .map_err(internal_error)?
    .map_err(internal_error)?;

    // AUTHORITATIVE readiness = chunk store (dense retrieval + /ask), never the
    // legacy node vectors.lancedb. Ready = has vectors AND matching sqlite rows.
    let chunk_ready = chunk_vector_records > 0 && chunk_records > 0;

    let manifest = load_manifest(&manifest_path);
    let manifest_files = manifest.files.len();
    let manifest_root = manifest.root.clone();

    let llm_model =
        std::env::var("ORACLE_LLM_MODEL").unwrap_or_else(|_| "voxtral-small-24b-2507".to_string());

    Ok(Json(serde_json::json!({
        "vector_store": {
            "backend": "lance",
            "path": vectors_path.to_string_lossy(),
            "records": vector_records,
            "ready": vector_records > 0,
        },
        "chunk_store": {
            "backend": "lance",
            "path": chunk_vectors_path.to_string_lossy(),
            "manifest_path": manifest_path.to_string_lossy(),
            "manifest_root": manifest_root,
            "manifest_files": manifest_files,
            "files": chunk_files,
            "records": chunk_records,
            "vector_records": chunk_vector_records,
            "ready": chunk_ready,
        },
        "ready": chunk_ready,
        "ollama": {
            "cli": serde_json::Value::Null,
            "server": "removed",
            "model": llm_model,
            "model_available": false,
            "models": [],
            "message": "Local Ollama chat path removed; Oracle answers are API-only.",
        },
        "setup_commands": [],
    })))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /coverage — node source coverage (verify_coverage.py)
// ═══════════════════════════════════════════════════════════════════════════

async fn coverage_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let sqlite_path = state.sqlite_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let sqlite = SqliteStore::new(&sqlite_path)?;
        let nodes = sqlite.all_nodes()?;
        let total = nodes.len();
        let oracle = nodes.iter().filter(|n| n.source == "oracle").count();
        let percent = if total > 0 {
            ((oracle as f64 / total as f64) * 10000.0).round() / 100.0
        } else {
            0.0
        };
        Ok::<_, anyhow::Error>(serde_json::json!({
            "total_nodes": total,
            "oracle_nodes": oracle,
            "oracle_percent": percent,
        }))
    })
    .await
    .map_err(internal_error)?
    .map_err(internal_error)?;
    Ok(Json(result))
}

// ═══════════════════════════════════════════════════════════════════════════
// Handler: GET /snapshot
// ═══════════════════════════════════════════════════════════════════════════

async fn snapshot_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Response> {
    require_operator(&headers, &state).map_err(auth_error)?;
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
    run_context(&state, &q, limit, None, false, None, None, None, None, None).await
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
    run_context(&state, &q, limit, None, prefer_lexical, None, None, None, None, None).await
}

async fn run_context(
    state: &Arc<AppState>,
    q: &str,
    limit: usize,
    allowed: Option<HashSet<String>>,
    prefer_lexical: bool,
    kind: Option<&str>,
    language: Option<&str>,
    symbols: Option<&[String]>,
    imports: Option<&[String]>,
    module: Option<&str>,
) -> Result<Json<serde_json::Value>, Response> {
    let engine = state.engine().map_err(internal_error)?;
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
                kind,
                language,
                symbols,
                imports,
                module,
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
    let (q, limit, allowed, filters) = parse_bounded_payload(payload, 8)?;
    let prefer_lexical = state.job_manager.indexing_in_progress();
    run_context(
        &state,
        &q,
        limit,
        Some(allowed),
        prefer_lexical,
        filters.kind.as_deref(),
        filters.language.as_deref(),
        filters.symbols.as_deref(),
        filters.imports.as_deref(),
        filters.module.as_deref(),
    )
    .await
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
    let (q, limit, allowed, filters) = parse_bounded_payload(payload, 5)?;
    let prefer_lexical = state.job_manager.indexing_in_progress();

    let engine = state.engine().map_err(internal_error)?;
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
                filters.kind.as_deref(),
                filters.language.as_deref(),
                filters.symbols.as_deref(),
                filters.imports.as_deref(),
                filters.module.as_deref(),
                filters.group_by_file,
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
    if texts.len() > MAX_BOUNDED_EMBED_TEXTS {
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
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
    let engine = state.engine().map_err(internal_error)?;
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
            file_ids.retain(|id| id.to_lowercase().contains(&needle));
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
    let index_root = authorize_index_root(params.root.as_deref(), &state)?;
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
    let index_root = authorize_index_root(params.root.as_deref(), &state)?;
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
            Some(index_root.as_path()),
            force,
            effective_max_batches,
            effective_idle,
            move || Arc::new(PoolTextEmbedder(pool)) as Arc<dyn TextEmbedder>,
        );
        return Ok(Json(serde_json::to_value(job_state).unwrap_or_default()));
    }

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
    let index_root = authorize_index_root(params.root.as_deref(), &state)?;
    let root = Some(index_root.as_path());
    let mode = params.mode.as_deref();

    // Watcher-triggered reindex MUST use the real embedder pool, exactly like
    // the manual "Index now" path and Python (index_jobs.py always embeds with
    // the real model). A hash embedder here would silently poison the live
    // vector store with near-random vectors for every auto-reindexed file.
    // Commit kick = full delta (max_batches=None); fs batch = single
    // opportunistic batch (max_batches=1), mirroring Python's kick params.
    let job_mgr = Arc::clone(&state.job_manager);
    let jr = index_root.clone();
    let pool_commit = Arc::clone(&state.embedder_pool);
    let on_commit: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let pool = Arc::clone(&pool_commit);
        let _ = job_mgr.start_background(Some(&jr), false, None, true, move || {
            Arc::new(PoolTextEmbedder(pool)) as Arc<dyn TextEmbedder>
        });
    });

    let job_mgr2 = Arc::clone(&state.job_manager);
    let jr2 = index_root.clone();
    let pool_batch = Arc::clone(&state.embedder_pool);
    let on_batch_ready: Arc<dyn Fn(Vec<String>) + Send + Sync> = Arc::new(move |_paths| {
        let pool = Arc::clone(&pool_batch);
        let _ = job_mgr2.start_background(Some(&jr2), false, Some(1), true, move || {
            Arc::new(PoolTextEmbedder(pool)) as Arc<dyn TextEmbedder>
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
// M3-P12c: also parses the bounded filters (kind/language/symbols/imports/
// module/group_by_file) and forwards them to the engine. Empty strings in
// symbols/imports are trimmed to None (mirrors allowed_file_ids trimming).
// symbols/imports exceeding MAX_BOUNDED_FILTER_ENTRIES return a 400 so a
// client misconfiguration surfaces cleanly rather than silently truncating.
// ═══════════════════════════════════════════════════════════════════════════

fn parse_bounded_payload(
    payload: BoundedPayload,
    default_limit: usize,
) -> Result<(String, usize, HashSet<String>, FilterOptions), Response> {
    use FilterOptions;
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

    let filters = FilterOptions {
        kind: payload.kind.filter(|s| !s.trim().is_empty()),
        language: payload.language.filter(|s| !s.trim().is_empty()),
        module: payload.module.filter(|s| !s.trim().is_empty()),
        symbols: match payload.symbols {
            None => None,
            Some(raw) => {
                if raw.len() > MAX_BOUNDED_FILTER_ENTRIES {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"detail": format!(
                            "symbols exceeds the maximum of {} entries.",
                            MAX_BOUNDED_FILTER_ENTRIES
                        )})),
                    )
                        .into_response());
                }
                let filtered: Vec<String> = raw
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                if filtered.is_empty() {
                    None
                } else {
                    Some(filtered)
                }
            }
        },
        imports: match payload.imports {
            None => None,
            Some(raw) => {
                if raw.len() > MAX_BOUNDED_FILTER_ENTRIES {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"detail": format!(
                            "imports exceeds the maximum of {} entries.",
                            MAX_BOUNDED_FILTER_ENTRIES
                        )})),
                    )
                        .into_response());
                }
                let filtered: Vec<String> = raw
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                if filtered.is_empty() {
                    None
                } else {
                    Some(filtered)
                }
            }
        },
        group_by_file: payload.group_by_file.unwrap_or(false),
    };

    Ok((q, limit, allowed, filters))
}

#[derive(Debug, Default)]
struct FilterOptions {
    kind: Option<String>,
    language: Option<String>,
    symbols: Option<Vec<String>>,
    imports: Option<Vec<String>>,
    module: Option<String>,
    group_by_file: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// Discovery file writer
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Serialize, Deserialize)]
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

/// Custom Debug redacts the agent token — the discovery payload must never
/// leak its bearer credential into a log line.
impl std::fmt::Debug for DiscoveryPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveryPayload")
            .field("base_url", &self.base_url)
            .field("auth_token", &"[redacted]")
            .field("index_root", &self.index_root)
            .field("pid", &self.pid)
            .field("heartbeat_at", &self.heartbeat_at)
            .finish()
    }
}

/// Write bytes to `path` with owner-only perms (0600 on unix) established
/// AT CREATION — never a world-readable window between create and chmod.
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    // A pre-existing file keeps its old mode through OpenOptions; re-assert.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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
    write_owner_only(path, text.as_bytes())
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
    write_owner_only(path, out.as_bytes())
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
        .route("/runtime", get(runtime_handler))
        .route("/coverage", get(coverage_handler))
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
