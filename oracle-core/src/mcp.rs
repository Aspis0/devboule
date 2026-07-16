//!
//! `oracle-mcp` — standalone MCP **stdio** server over `oracle-core`.
//!
//! Replaces the Python `oracle/server/mcp_handler.py` FastMCP retrieval rail.
//! It exposes 6 retrieval tools (`oracle_ask`, `oracle_context`, `oracle_find`,
//! `oracle_node`, `oracle_similar`, `oracle_duplicates`) over an arbitrary
//! `ORACLE_DIR`. Retrieval ONLY — NO LLM (`answerer: None`), matching the
//! Python rail's `make_engine()` wiring.
//!
//! Wiring mirrors `crate::server::AppState::engine()` and `PoolQueryEmbedder`
//! via the shared public factories `build_query_engine` / `pool_query_embedder`
//! so the two code paths cannot drift. The actual `#[tokio::main]` entry point
//! lives in `src/bin/oracle_mcp.rs`, which calls [`serve_stdio`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use rmcp::{
    ErrorData as McpError, ServiceExt, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, serde::Deserialize,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde_json::Value;

use crate::{
    config::OracleDataPaths,
    embed::{BackendChoice, EmbedderPool},
    model_download,
    query::engine::QueryEngine,
    server::{build_query_engine, pool_query_embedder},
};

/// Shared, non-handler state for the server: store paths + a single embedder
/// pool (the ONNX model is expensive to load, so reuse it across calls).
pub struct OracleInner {
    paths: OracleDataPaths,
    embedder_pool: Arc<EmbedderPool>,
}

impl OracleInner {
    /// Construct from resolved store paths and an embedder pool.
    pub fn new(paths: OracleDataPaths, embedder_pool: Arc<EmbedderPool>) -> Self {
        Self {
            paths,
            embedder_pool,
        }
    }

    /// Build the engine for a single call (stores are not `Clone`).
    fn engine(&self) -> Result<QueryEngine> {
        build_query_engine(&self.paths)
    }

    /// Query embedder: real model (unless `ORACLE_QUERY_EMBEDDER=hash`).
    fn embedder(&self) -> crate::server::PoolQueryEmbedder {
        pool_query_embedder(Arc::clone(&self.embedder_pool), false)
    }
}

// The macro `tool_router` expects a field named `tool_router: ToolRouter<Self>`.
#[derive(Clone)]
pub struct OracleMcp {
    inner: Arc<OracleInner>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl OracleMcp {
    pub fn new(inner: Arc<OracleInner>) -> Self {
        Self {
            inner,
            tool_router: Self::tool_router(),
        }
    }
    #[tool(description = "Ask the Oracle for information about the project's architecture. Pre-filter with kind/language/symbols.")]
    pub async fn oracle_ask(
        &self,
        Parameters(args): Parameters<AskArgs>,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.inner.engine().map_err(internal)?;
        let emb = self.inner.embedder();
        let kind = none_if_empty(&args.kind);
        let lang = none_if_empty(&args.language);
        let syms = none_if_empty_vec(&args.symbols);
        let limit = args.limit.unwrap_or(5);
        let resp = engine
            .ask(
                &args.query,
                limit,
                &emb,
                None, // answerer: retrieval-only (extractive)
                None,
                false,
                kind,
                lang,
                syms.as_deref(),
                None,
                None,
                args.group_by_file.unwrap_or(false),
            )
            .await
            .map_err(internal)?;
        let value = serde_json::to_value(&resp).map_err(|e| internal_msg(&e))?;
        let text = serde_json::to_string(&value).map_err(|e| internal_msg(&e))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Returns semantically relevant text chunks. Pre-filter with kind/language/symbols.")]
    pub async fn oracle_context(
        &self,
        Parameters(args): Parameters<ContextArgs>,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.inner.engine().map_err(internal)?;
        let emb = self.inner.embedder();
        let kind = none_if_empty(&args.kind);
        let lang = none_if_empty(&args.language);
        let syms = none_if_empty_vec(&args.symbols);
        let imports = none_if_empty_vec(&args.imports);
        let module = none_if_empty(&args.module);
        let limit = args.limit.unwrap_or(8);
        let chunks = engine
            .context(
                &args.query,
                limit,
                &emb,
                None,
                false,
                kind,
                lang,
                syms.as_deref(),
                imports.as_deref(),
                module,
            )
            .await
            .map_err(internal)?;
        let envelope = serde_json::json!({ "query": args.query, "chunks": chunks });
        let text = serde_json::to_string(&envelope).map_err(|e| internal_msg(&e))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "PRECISE: find EXACT symbols (functions, structs, classes) by name. Returns kind, symbol_name, signature, language, and exact line ranges (line_start, line_end) — open the file at that line.")]
    pub async fn oracle_find(
        &self,
        Parameters(args): Parameters<FindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.inner.engine().map_err(internal)?;
        let emb = self.inner.embedder();
        let kind = none_if_empty(&args.kind);
        let lang = none_if_empty(&args.language);
        let limit = args.limit.unwrap_or(10);
        let chunks = engine
            .context(
                &args.query,
                limit,
                &emb,
                None,
                false,
                kind,
                lang,
                Some(&[args.query.clone()]),
                None,
                None,
            )
            .await
            .map_err(internal)?;
        let envelope = serde_json::json!({
            "query": args.query,
            "kind": kind.map(|s| Value::String(s.to_string())),
            "language": lang.map(|s| Value::String(s.to_string())),
            "chunks": chunks,
            "hint": "Each chunk has kind, symbol_name, signature, language, line_start, line_end — use these to decide which files to open.",
        });
        let text = serde_json::to_string(&envelope).map_err(|e| internal_msg(&e))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Get the full record of a component by ID.")]
    pub async fn oracle_node(
        &self,
        Parameters(args): Parameters<NodeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.inner.engine().map_err(internal)?;
        let card = engine.node(&args.id).map_err(internal)?;
        let value = serde_json::to_value(&card).map_err(|e| internal_msg(&e))?;
        let text = serde_json::to_string(&value).map_err(|e| internal_msg(&e))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Find similar components before duplicating logic.")]
    pub async fn oracle_similar(
        &self,
        Parameters(args): Parameters<SimilarArgs>,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.inner.engine().map_err(internal)?;
        let limit = args.limit.unwrap_or(5);
        let entries = engine.similar(&args.id, limit).await.map_err(internal)?;
        let value = serde_json::to_value(&entries).map_err(|e| internal_msg(&e))?;
        let text = serde_json::to_string(&value).map_err(|e| internal_msg(&e))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "List components with the same label in different areas.")]
    pub async fn oracle_duplicates(
        &self,
        Parameters(_args): Parameters<NoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.inner.engine().map_err(internal)?;
        let groups = engine.duplicates().map_err(internal)?;
        let value = serde_json::to_value(&groups).map_err(|e| internal_msg(&e))?;
        let text = serde_json::to_string(&value).map_err(|e| internal_msg(&e))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for OracleMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "architecture-oracle".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

impl OracleMcp {
    /// Return the registered tool specs (mirrors the `ListTools` result the
    /// MCP server advertises). Exposed for integration tests so they can assert
    /// the exact tool set without standing up a transport.
    pub fn tool_specs(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }
}

// ── Argument structs (serde + schemars) ────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AskArgs {
    pub query: String,
    #[serde(default = "default_limit5")]
    pub limit: Option<usize>,
    #[serde(default)]
    #[schemars(description = "Filter: function, struct, class, enum, trait, impl, module, type, macro, module_header.")]
    pub kind: String,
    #[serde(default)]
    #[schemars(description = "Filter: rust, python, typescript, javascript, java, kotlin.")]
    pub language: String,
    #[serde(default)]
    #[schemars(description = "Only chunks containing these symbols.")]
    pub symbols: Vec<String>,
    #[serde(default)]
    #[schemars(description = "Group results by file.")]
    pub group_by_file: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContextArgs {
    pub query: String,
    #[serde(default = "default_limit8")]
    pub limit: Option<usize>,
    #[serde(default)]
    #[schemars(description = "Filter: function, struct, class, enum, trait, impl, module, type, macro, module_header.")]
    pub kind: String,
    #[serde(default)]
    #[schemars(description = "Filter: rust, python, typescript, javascript, java, kotlin.")]
    pub language: String,
    #[serde(default)]
    #[schemars(description = "Only chunks containing these symbols.")]
    pub symbols: Vec<String>,
    #[serde(default)]
    #[schemars(description = "Only chunks that reference these imports.")]
    pub imports: Vec<String>,
    #[serde(default)]
    #[schemars(description = "Only chunks from files whose path contains this.")]
    pub module: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    #[schemars(description = "Symbol name to find.")]
    pub query: String,
    #[serde(default)]
    #[schemars(description = "Symbol kind: function, struct, class, enum, trait, impl, module, type, macro.")]
    pub kind: String,
    #[serde(default)]
    #[schemars(description = "Language: rust, python, typescript, javascript, java, kotlin.")]
    pub language: String,
    #[serde(default = "default_limit10")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NodeArgs {
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SimilarArgs {
    pub id: String,
    #[serde(default = "default_limit5")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NoArgs {}

fn default_limit5() -> Option<usize> {
    Some(5)
}
fn default_limit8() -> Option<usize> {
    Some(8)
}
fn default_limit10() -> Option<usize> {
    Some(10)
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn none_if_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn none_if_empty_vec(v: &[String]) -> Option<Vec<String>> {
    if v.is_empty() {
        None
    } else {
        Some(v.to_vec())
    }
}

/// Map an `anyhow::Error` to an MCP internal error.
fn internal(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn internal_msg(e: &impl std::fmt::Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

/// Resolve the model directory + verify the ONNX bundle, mirroring the
/// install-check done by the Python runtime before serving.
///
/// `ORACLE_MODEL_DIR`, when set, is the FULL model directory — i.e. the exact
/// path `model_dir(root)` returns (`<root>/models/qwen3-onnx`), holding
/// `onnx/model_int8.onnx` (or `model.onnx`) and `tokenizer.json` directly. It
/// is NOT a data root. When unset, the model is resolved under `oracle_dir`.
fn resolve_model_dir(oracle_dir: &Path) -> Result<PathBuf> {
    let model_dir = if let Ok(dir) = std::env::var("ORACLE_MODEL_DIR") {
        if dir.trim().is_empty() {
            model_download::model_dir(oracle_dir)
        } else {
            PathBuf::from(dir)
        }
    } else {
        model_download::model_dir(oracle_dir)
    };

    if !model_download::model_present_at(&model_dir, true) {
        anyhow::bail!(
            "ONNX embedding model not installed at {} — run Oracle runtime Install",
            model_dir.display()
        );
    }
    Ok(model_dir)
}

/// Build `OracleInner` from the environment (`ORACLE_DIR` required).
fn inner_from_env() -> Result<OracleInner> {
    let oracle_dir = std::env::var("ORACLE_DIR")
        .context("ORACLE_DIR is unset")?;
    if oracle_dir.trim().is_empty() {
        anyhow::bail!("ORACLE_DIR is unset");
    }
    let oracle_dir_path = PathBuf::from(&oracle_dir);
    if !oracle_dir_path.exists() {
        anyhow::bail!("ORACLE_DIR does not exist: {}", oracle_dir_path.display());
    }

    let paths = OracleDataPaths::from_root(&oracle_dir_path);

    let model_dir = resolve_model_dir(&oracle_dir_path)?;

    // The shared ONNX model lives at the runtime data root (int8 quantized).
    let pool = EmbedderPool::new(BackendChoice::Ort {
        model_dir,
        int8: true,
    });

    Ok(OracleInner::new(
        paths,
        Arc::new(pool),
    ))
}

/// Run the stdio MCP server to completion (used by the bin's `main`).
pub async fn serve_stdio() -> Result<()> {
    let inner = inner_from_env()?;
    let service = OracleMcp::new(Arc::new(inner))
        .serve(stdio())
        .await
        .context("failed to start oracle-mcp stdio server")?;
    service.waiting().await?;
    Ok(())
}
