//! Query engine orchestration — Rust port of `oracle/server/query_engine.py`.
//!
//! Ports the QUERY ORCHESTRATION layer (context merge, ask ranking, similar
//! node lookup, cluster reads, health/snapshot).  The LLM answer path is
//! delegated to the injected [`ContextAnswerer`] trait (P5 scope).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::ingest::retrieval_text;
use crate::query::lexical::{self, ScoredChunk};
use crate::store::lance::{hash_embed, LanceHit, LanceStore};
use crate::store::sqlite::{FileChunk, NodeCard, SqliteStore};

// ═══════════════════════════════════════════════════════════════════════════
// Traits
// ═══════════════════════════════════════════════════════════════════════════

/// LLM answer synthesis trait (P5 scope).
pub trait ContextAnswerer: Send + Sync {
    fn answer(&self, query: &str, context_chunks: &[ContextChunk]) -> Result<AnswerPayload>;
}

/// Query embedding trait — decouples the engine from any specific embedder.
pub trait QueryEmbedder: Send + Sync {
    fn embed_query(&self, text: &str, dims: usize) -> Result<Vec<f32>>;
}

// ═══════════════════════════════════════════════════════════════════════════
// HashQueryEmbedder — deterministic blake2b fallback
// ═══════════════════════════════════════════════════════════════════════════

/// Hash-based query embedder, mirroring the `ORACLE_QUERY_EMBEDDER=hash` /
/// `require_real_embedder` gate logic from Python
/// `lance_store.py::embed_query_text`.
pub struct HashQueryEmbedder;

impl QueryEmbedder for HashQueryEmbedder {
    fn embed_query(&self, text: &str, dims: usize) -> Result<Vec<f32>> {
        // When require_real_embedder is set, hash embeddings are garbage
        // (they retrieve random results).  Match Python's hard block.
        if config::require_real_embedder() {
            anyhow::bail!(
                "Qwen embedding model is unavailable. \
                 Run Oracle doctor / check the runtime install."
            );
        }
        let prefixed = retrieval_text::query_embedding_text(text, None);
        Ok(hash_embed(&prefixed, dims))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Response types — snake_case JSON matches Python dicts EXACTLY
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContextChunk {
    pub chunk_id: String,
    pub file_source: String,
    pub chunk_index: usize,
    pub start_char: usize,
    pub end_char: usize,
    pub score: f64,
    pub retrieval: String,
    pub text: String,
    pub last_modified: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: usize,
    pub line_end: usize,
    pub symbols_used: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Citation {
    #[serde(rename = "ref")]
    pub ref_id: String,
    pub file_source: String,
    pub chunk_id: String,
    pub chunk_index: Option<i64>,
    pub start_char: Option<i64>,
    pub end_char: Option<i64>,
    pub retrieval: String,
    pub score: f64,
}

/// Payload returned by a [`ContextAnswerer`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AnswerPayload {
    pub answer: String,
    pub citations: Vec<Citation>,
    pub not_found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AskResponse {
    pub mode: String,
    pub query: String,
    pub summary: String,
    pub answer: String,
    pub citations: Vec<Citation>,
    pub not_found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_model: Option<String>,
    pub results: Vec<ResultEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grouped: Option<Vec<GroupEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResultEntry {
    pub id: String,
    pub label: String,
    pub node_type: String,
    pub cluster: i64,
    pub score: f64,
    pub file_source: String,
    pub function_primary: String,
    pub dependencies: Vec<String>,
    pub chunk_id: Option<String>,
    pub chunk_index: Option<i64>,
    pub start_char: Option<i64>,
    pub end_char: Option<i64>,
    pub chunk_preview: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: i64,
    pub line_end: i64,
    pub symbols_used: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GroupEntry {
    pub file: String,
    pub total_score: f64,
    pub chunks: Vec<GroupChunk>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub symbol_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub signature: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    pub line_start: i64,
    pub line_end: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GroupChunk {
    pub chunk_id: String,
    pub score: f64,
    pub retrieval: String,
    pub start_char: usize,
    pub end_char: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HealthResponse {
    pub status: String,
    pub phase: String,
    pub nodes: usize,
    pub vector_records: usize,
    pub chunk_files: usize,
    pub chunk_records: usize,
    pub chunk_vector_records: usize,
    pub chunk_profile: String,
    pub query_profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SnapshotResponse {
    pub status: String,
    pub source: String,
    pub phase: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub cluster_count: usize,
    pub duplicate_labels: Vec<DuplicateGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DuplicateGroup {
    pub label: String,
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterResponse {
    pub epoch: String,
    pub clusters: Vec<ClusterInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    #[serde(rename = "clusterId")]
    pub cluster_id: i64,
    pub size: usize,
    #[serde(rename = "sampleFiles")]
    pub sample_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMemberResponse {
    #[serde(rename = "clusterId")]
    pub cluster_id: i64,
    pub members: Vec<ClusterMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMember {
    #[serde(rename = "fileId")]
    pub file_id: String,
    pub score: f64,
}

// ═══════════════════════════════════════════════════════════════════════════
// Internal types
// ═══════════════════════════════════════════════════════════════════════════

/// Unified chunk preview used inside `_result` logic — mirrors the dict that
/// Python builds from both FileChunk and `chunk_payload_to_store_shape`.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
struct ChunkPreview {
    id: String,
    file_id: String,
    file_sorgente: String,
    chunk_index: i64,
    start_char: i64,
    end_char: i64,
    text: String,
    kind: String,
    symbol_name: String,
    signature: String,
    language: String,
    line_start: i64,
    line_end: i64,
    symbols_used: Vec<String>,
}

impl From<&FileChunk> for ChunkPreview {
    fn from(c: &FileChunk) -> Self {
        Self {
            id: c.id.clone(),
            file_id: c.file_id.clone(),
            file_sorgente: c.file_sorgente.clone(),
            chunk_index: c.chunk_index,
            start_char: c.start_char,
            end_char: c.end_char,
            text: c.text.clone(),
            kind: c.kind.clone(),
            symbol_name: c.symbol_name.clone(),
            signature: c.signature.clone(),
            language: c.language.clone(),
            line_start: c.line_start,
            line_end: c.line_end,
            symbols_used: c.symbols_used.clone(),
        }
    }
}

impl From<&ContextChunk> for ChunkPreview {
    fn from(c: &ContextChunk) -> Self {
        Self {
            id: c.chunk_id.clone(),
            file_id: c.file_source.clone(),
            file_sorgente: c.file_source.clone(),
            chunk_index: c.chunk_index as i64,
            start_char: c.start_char as i64,
            end_char: c.end_char as i64,
            text: c.text.clone(),
            kind: c.kind.clone(),
            symbol_name: c.symbol_name.clone(),
            signature: c.signature.clone(),
            language: c.language.clone(),
            line_start: c.line_start as i64,
            line_end: c.line_end as i64,
            symbols_used: c.symbols_used.clone(),
        }
    }
}

/// Intermediate row entry during ask() ranking.
struct RowEntry {
    id: String,
    score: f64,
    chunk: Option<ChunkPreview>,
}

// ═══════════════════════════════════════════════════════════════════════════
// QueryEngine
// ═══════════════════════════════════════════════════════════════════════════

pub struct QueryEngine {
    pub sqlite: SqliteStore,
    pub vectors: LanceStore,
    pub chunk_vectors: Option<LanceStore>,
    pub file_vectors: Option<LanceStore>,
}

impl QueryEngine {
    /// Build the engine.  Mirrors `routes.py::make_engine` constructor wiring.
    pub fn new(
        sqlite: SqliteStore,
        vectors: LanceStore,
        chunk_vectors: Option<LanceStore>,
        file_vectors: Option<LanceStore>,
    ) -> Self {
        Self {
            sqlite,
            vectors,
            chunk_vectors,
            file_vectors,
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// Embed the query text through the provided embedder, using the store's
    /// canonical dimension.
    async fn embed_query(&self, embedder: &dyn QueryEmbedder, query: &str) -> Result<Vec<f32>> {
        let dims = self.dims().await;
        embedder.embed_query(query, dims)
    }

    /// Canonical embedding dimensionality, read from the chunk-vector store
    /// when available, otherwise from `config::EMBED_DIMS`.
    async fn dims(&self) -> usize {
        if let Some(ref cv) = self.chunk_vectors {
            if let Ok(count) = cv.count().await {
                if count > 0 {
                    if let Ok(rows) = cv.read_all().await {
                        if let Some(first) = rows.first() {
                            return first.vector.len();
                        }
                    }
                }
            }
        }
        config::EMBED_DIMS
    }

    /// Best per-file chunk scores and previews from the chunk-vector store.
    async fn chunk_scores(
        &self,
        query_vec: &[f32],
        limit: usize,
        allowed_file_ids: Option<&HashSet<String>>,
    ) -> Result<(HashMap<String, f64>, HashMap<String, ChunkPreview>)> {
        let Some(ref cv) = self.chunk_vectors else {
            return Ok((HashMap::new(), HashMap::new()));
        };
        let hits = cv.search(query_vec, limit).await?;
        let mut scores: HashMap<String, f64> = HashMap::new();
        let mut previews: HashMap<String, ChunkPreview> = HashMap::new();
        for hit in hits {
            if let Some(chunk) = self.sqlite.get_chunk(&hit.id)? {
                if allowed_file_ids.is_none_or(|ids| ids.contains(&chunk.file_id)) {
                    let score = hit.score as f64;
                    if score > scores.get(&chunk.file_id).copied().unwrap_or(-1.0) {
                        scores.insert(chunk.file_id.clone(), score);
                        previews.insert(chunk.file_id.clone(), ChunkPreview::from(&chunk));
                    }
                }
            }
        }
        Ok((scores, previews))
    }

    /// `self._result(row)` from Python — build a [`ResultEntry`] from a row
    /// with an optional chunk preview.
    fn build_result(&self, row: &RowEntry) -> ResultEntry {
        let card = self.sqlite.get_node(&row.id).ok().flatten();
        match card {
            Some(card) => self.result_from_card(&card, row),
            None => self.result_from_chunk_only(row),
        }
    }

    fn result_from_card(&self, card: &NodeCard, row: &RowEntry) -> ResultEntry {
        let cp = row.chunk.as_ref();
        ResultEntry {
            id: card.id.clone(),
            label: card.label.clone(),
            node_type: "file".to_string(),
            cluster: parse_cluster(&card.cluster_semantic),
            score: row.score,
            file_source: card.file_sorgente.clone(),
            function_primary: card.funzione_primaria.clone(),
            dependencies: card.dipende_da.clone(),
            chunk_id: cp.map(|c| c.id.clone()),
            chunk_index: cp.map(|c| c.chunk_index),
            start_char: cp.map(|c| c.start_char),
            end_char: cp.map(|c| c.end_char),
            chunk_preview: cp.map(|c| summarize_chunk(&c.text)).unwrap_or_default(),
            kind: "file".to_string(),
            symbol_name: card.label.clone(),
            signature: String::new(),
            language: String::new(),
            line_start: 0,
            line_end: 0,
            symbols_used: card.dipende_da.clone(),
        }
    }

    fn result_from_chunk_only(&self, row: &RowEntry) -> ResultEntry {
        let cp = row.chunk.as_ref();
        let empty_cp = ChunkPreview::default();
        let c = cp.unwrap_or(&empty_cp);
        let chunk_kind = if c.kind.is_empty() {
            "text_slice".to_string()
        } else {
            c.kind.clone()
        };
        let label = if !c.symbol_name.is_empty() {
            c.symbol_name.clone()
        } else {
            row.id.rsplit('/').next().unwrap_or(&row.id).to_string()
        };
        ResultEntry {
            id: row.id.clone(),
            label,
            node_type: "chunk".to_string(),
            cluster: 0,
            score: row.score,
            file_source: if !c.file_sorgente.is_empty() {
                c.file_sorgente.clone()
            } else {
                row.id.clone()
            },
            function_primary: summarize_chunk(&c.text),
            dependencies: vec![],
            chunk_id: Some(c.id.clone()),
            chunk_index: Some(c.chunk_index),
            start_char: Some(c.start_char),
            end_char: Some(c.end_char),
            chunk_preview: summarize_chunk(&c.text),
            kind: chunk_kind,
            symbol_name: c.symbol_name.clone(),
            signature: c.signature.clone(),
            language: c.language.clone(),
            line_start: c.line_start,
            line_end: c.line_end,
            symbols_used: c.symbols_used.clone(),
        }
    }

    // ── public API ───────────────────────────────────────────────────────

    /// Dense + lexical context retrieval.
    ///
    /// `embedder` is REQUIRED: Python always embeds the query with its
    /// built-in model when the dense path runs (the only Python skip
    /// conditions are `prefer_lexical` and a missing chunk store, mirrored
    /// here). Callers that genuinely want lexical-only pass
    /// `prefer_lexical = true`, never a dummy embedder.
    pub async fn context(
        &self,
        query: &str,
        limit: usize,
        embedder: &dyn QueryEmbedder,
        allowed_file_ids: Option<&HashSet<String>>,
        prefer_lexical: bool,
        kind: Option<&str>,
        language: Option<&str>,
        symbols: Option<&[String]>,
        imports: Option<&[String]>,
        module: Option<&str>,
    ) -> Result<Vec<ContextChunk>> {
        let mut combined: HashMap<String, ContextChunk> = HashMap::new();

        // ── Dense path ───────────────────────────────────────────────
        if !prefer_lexical {
            if let Some(ref chunk_vectors) = self.chunk_vectors {
                let query_vec = self.embed_query(embedder, query).await?;
                let hits = chunk_vectors.search(&query_vec, limit.max(1)).await?;
                for hit in hits {
                    if let Some(chunk) = self.sqlite.get_chunk(&hit.id)? {
                        if allowed_file_ids.is_none_or(|ids| ids.contains(&chunk.file_id))
                            && chunk_matches_filters(
                                &chunk, kind, language, symbols, imports, module,
                            )
                        {
                            let ctx =
                                ContextChunk::from_file_chunk(&chunk, hit.score as f64, "dense");
                            combined.insert(chunk.id.clone(), ctx);
                        }
                    }
                }
            }
        }

        // ── Lexical path ─────────────────────────────────────────────
        let all_chunks = self.sqlite.all_chunks()?;
        let filtered: Vec<FileChunk> = all_chunks
            .into_iter()
            .filter(|c| allowed_file_ids.is_none_or(|ids| ids.contains(&c.file_id)))
            .filter(|c| chunk_matches_filters(c, kind, language, symbols, imports, module))
            .collect();
        let scored: Vec<ScoredChunk> = filtered.iter().map(file_chunk_to_scored).collect();
        let lexical_limit = (limit * 3).max(limit);
        let lexical_results = lexical::lexical_chunk_context(query, &scored, lexical_limit);

        // Build a map from FileChunk id → FileChunk for fast lookup
        let chunk_by_id: HashMap<String, &FileChunk> =
            filtered.iter().map(|c| (c.id.clone(), c)).collect();

        for lr in &lexical_results {
            if let Some(existing) = combined.get_mut(&lr.chunk_id) {
                existing.score = existing.score.max(lr.score);
                existing.retrieval = "dense+lexical".to_string();
            } else if let Some(fc) = chunk_by_id.get(&lr.chunk_id) {
                let ctx = ContextChunk::from_file_chunk(fc, lr.score, "lexical");
                combined.insert(lr.chunk_id.clone(), ctx);
            }
        }

        let mut rows: Vec<ContextChunk> = combined.into_values().collect();
        rows.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file_source.cmp(&b.file_source))
                .then_with(|| a.chunk_index.cmp(&b.chunk_index))
        });
        rows.truncate(limit.max(1));
        Ok(rows)
    }

    /// Full ask pipeline: context + file-level ranking + answer synthesis.
    pub async fn ask(
        &self,
        query: &str,
        limit: usize,
        embedder: &dyn QueryEmbedder,
        answerer: Option<&dyn ContextAnswerer>,
        allowed_file_ids: Option<&HashSet<String>>,
        prefer_lexical: bool,
        kind: Option<&str>,
        language: Option<&str>,
        symbols: Option<&[String]>,
        imports: Option<&[String]>,
        module: Option<&str>,
        group_by_file: bool,
    ) -> Result<AskResponse> {
        let context_chunks = self
            .context(
                query,
                limit.max(1),
                embedder,
                allowed_file_ids,
                prefer_lexical,
                kind,
                language,
                symbols,
                imports,
                module,
            )
            .await?;

        // ── Answer synthesis ─────────────────────────────────────────
        let generated = if let Some(answerer) = answerer {
            answerer.answer(query, &context_chunks)?
        } else {
            degraded_answer(query, &context_chunks)
        };

        // ── Node-card vector scores ──────────────────────────────────
        let vector_scores: HashMap<String, f64> = if prefer_lexical {
            HashMap::new()
        } else {
            let count = self.vectors.count().await?;
            let search_limit = count.max(limit);
            let qv = self.embed_query(embedder, query).await?;
            let hits = self.vectors.search(&qv, search_limit).await?;
            hits.into_iter().map(|h| (h.id, h.score as f64)).collect()
        };

        // ── Chunk scores (best per file) ────────────────────────────
        let (chunk_scores_map, chunk_preview_map) = if prefer_lexical {
            (HashMap::new(), HashMap::new())
        } else {
            let qv = self.embed_query(embedder, query).await?;
            self.chunk_scores(&qv, (30).max(limit * 8), allowed_file_ids)
                .await?
        };

        // ── Build row entries ────────────────────────────────────────
        let mut rows: Vec<RowEntry> = Vec::new();
        let all_nodes = self.sqlite.all_nodes()?;
        for card in &all_nodes {
            if allowed_file_ids.is_some()
                && !allowed_file_ids.unwrap().contains(&card.id)
                && !allowed_file_ids.unwrap().contains(&card.file_sorgente)
            {
                continue;
            }
            let lexical = lexical_score_node(query, card);
            let vector = vector_scores.get(&card.id).copied().unwrap_or(0.0);
            let chunk = chunk_scores_map.get(&card.id).copied().unwrap_or(0.0);
            let score = lexical + (vector * 0.25) + (chunk * 2.5);
            if score > 0.0 {
                rows.push(RowEntry {
                    id: card.id.clone(),
                    score,
                    chunk: chunk_preview_map.get(&card.id).cloned(),
                });
            }
        }

        let mut known: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();

        // Synthetic rows for chunk-only files (no matching node card)
        for (file_id, score) in &chunk_scores_map {
            if !known.contains(file_id) {
                rows.push(RowEntry {
                    id: file_id.clone(),
                    score: score * 2.5,
                    chunk: chunk_preview_map.get(file_id).cloned(),
                });
                known.insert(file_id.clone());
            }
        }

        // Synthetic rows for context-only files
        for cc in &context_chunks {
            if !known.contains(&cc.file_source) {
                let cp = ChunkPreview::from(cc);
                rows.push(RowEntry {
                    id: cc.file_source.clone(),
                    score: cc.score * 2.5,
                    chunk: Some(cp),
                });
                known.insert(cc.file_source.clone());
            }
        }

        rows.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });

        let effective_limit = limit.max(1);
        let results: Vec<ResultEntry> = rows
            .iter()
            .take(effective_limit)
            .map(|r| self.build_result(r))
            .collect();

        let grouped = if group_by_file {
            Some(group_by_file_fn(&context_chunks, &results))
        } else {
            None
        };

        // ── Summary logic ───────────────────────────────────────────
        // When no answerer is available, always build the labels-based
        // summary (the 'else' branch from Python).  When an answerer IS
        // provided, mirror the Python logic: use the answer text when
        // not_found or citations exist, otherwise labels-based.
        let labels: String = results
            .iter()
            .take(3)
            .map(|r| r.label.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        let summary =
            if answerer.is_some() && (generated.not_found || !generated.citations.is_empty()) {
                generated.answer.clone()
            } else if !labels.is_empty() {
                format!("Grounded Oracle matches: {}.", labels)
            } else {
                "No Oracle matches found.".to_string()
            };

        Ok(AskResponse {
            mode: "oracle-qwen-local".to_string(),
            query: query.to_string(),
            summary,
            answer: generated.answer,
            citations: generated.citations,
            not_found: generated.not_found,
            suggested_path: generated.suggested_path,
            answer_source: generated.answer_source,
            fallback_reason: generated.fallback_reason,
            llm_provider: generated.llm_provider,
            llm_model: generated.llm_model,
            results,
            grouped,
        })
    }

    /// Similar nodes: node-card store first, file_vectors fallback.
    pub async fn similar(&self, node_id: &str, limit: usize) -> Result<Vec<ResultEntry>> {
        let hits = self.vectors.similar(node_id, limit).await?;
        if !hits.is_empty() {
            return Ok(hits
                .into_iter()
                .map(|h| self.hit_to_result_entry(h))
                .collect());
        }
        if let Some(ref fv) = self.file_vectors {
            let hits = fv.similar(node_id, limit).await?;
            return Ok(hits
                .into_iter()
                .map(|h| self.hit_to_result_entry(h))
                .collect());
        }
        Ok(vec![])
    }

    fn hit_to_result_entry(&self, hit: LanceHit) -> ResultEntry {
        let card = self.sqlite.get_node(&hit.id).ok().flatten();
        match card {
            Some(card) => ResultEntry {
                id: card.id.clone(),
                label: card.label.clone(),
                node_type: "file".to_string(),
                cluster: parse_cluster(&card.cluster_semantic),
                score: hit.score as f64,
                file_source: card.file_sorgente.clone(),
                function_primary: card.funzione_primaria.clone(),
                dependencies: card.dipende_da.clone(),
                chunk_id: None,
                chunk_index: None,
                start_char: None,
                end_char: None,
                chunk_preview: String::new(),
                kind: "file".to_string(),
                symbol_name: card.label.clone(),
                signature: String::new(),
                language: String::new(),
                line_start: 0,
                line_end: 0,
                symbols_used: card.dipende_da.clone(),
            },
            None => ResultEntry {
                id: hit.id.clone(),
                label: hit.label.clone(),
                node_type: "chunk".to_string(),
                cluster: 0,
                score: hit.score as f64,
                file_source: hit.id.clone(),
                // Python: summarize_chunk({}) fallback message for card-less rows.
                function_primary: "Chunk-level match from the full-file Oracle index.".to_string(),
                dependencies: vec![],
                chunk_id: None,
                chunk_index: None,
                start_char: None,
                end_char: None,
                chunk_preview: "Chunk-level match from the full-file Oracle index.".to_string(),
                kind: "text_slice".to_string(),
                symbol_name: hit.label.clone(),
                signature: String::new(),
                language: String::new(),
                line_start: 0,
                line_end: 0,
                symbols_used: vec![],
            },
        }
    }

    /// Node card lookup.
    pub fn node(&self, node_id: &str) -> Result<NodeCard> {
        self.sqlite
            .get_node(node_id)?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", node_id))
    }

    /// Cluster members by name.
    pub fn cluster(&self, name: &str) -> Result<Vec<NodeCard>> {
        self.sqlite.by_cluster(name)
    }

    /// Area members by name.
    pub fn area(&self, name: &str) -> Result<Vec<NodeCard>> {
        self.sqlite.by_area(name)
    }

    /// Duplicate label groups.
    pub fn duplicates(&self) -> Result<Vec<Vec<String>>> {
        let all_nodes = self.sqlite.all_nodes()?;
        let mut by_label: HashMap<String, Vec<&NodeCard>> = HashMap::new();
        for node in &all_nodes {
            by_label.entry(node.label.clone()).or_default().push(node);
        }
        let mut groups: Vec<Vec<String>> = Vec::new();
        for nodes in by_label.values() {
            let areas: HashSet<&str> = nodes.iter().map(|n| n.area.as_str()).collect();
            if nodes.len() > 1 && areas.len() > 1 {
                let mut ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
                ids.sort();
                groups.push(ids);
            }
        }
        groups.sort_by(|a, b| a[0].cmp(&b[0]));
        Ok(groups)
    }

    /// Health probe.
    pub async fn health(&self) -> Result<HealthResponse> {
        let nodes = self.sqlite.all_nodes()?;
        let vector_records = self.vectors.count().await?;
        let chunk_files = self.sqlite.chunk_file_count()?;
        let chunk_records = self.sqlite.chunk_count()?;
        let chunk_vector_records = match self.chunk_vectors {
            Some(ref cv) => cv.count().await?,
            None => 0,
        };
        let last_updated = nodes
            .iter()
            .map(|n| n.ultima_modifica.as_str())
            .max()
            .map(|s| s.to_string());
        let last_updated = last_updated.filter(|s| !s.is_empty());

        Ok(HealthResponse {
            status: "ready".to_string(),
            phase: "phase1".to_string(),
            nodes: nodes.len(),
            vector_records,
            chunk_files,
            chunk_records,
            chunk_vector_records,
            chunk_profile: config::active_chunk_profile_version(None),
            query_profile: active_query_profile(),
            last_updated,
        })
    }

    /// Snapshot probe.
    pub async fn snapshot(&self) -> Result<SnapshotResponse> {
        let health = self.health().await?;
        let dup_groups = self.duplicates()?;
        let dup_labels: Vec<DuplicateGroup> = dup_groups
            .iter()
            .filter_map(|ids| {
                self.sqlite
                    .get_node(&ids[0])
                    .ok()
                    .flatten()
                    .map(|card| DuplicateGroup {
                        label: card.label,
                        node_ids: ids.clone(),
                    })
            })
            .collect();
        let all_nodes = self.sqlite.all_nodes()?;
        let cluster_count: usize = all_nodes
            .iter()
            .map(|n| n.cluster_semantic.clone())
            .collect::<HashSet<_>>()
            .len();

        Ok(SnapshotResponse {
            status: health.status,
            source: "rust-oracle".to_string(),
            phase: "phase1".to_string(),
            node_count: health.nodes,
            edge_count: 0,
            cluster_count,
            duplicate_labels: dup_labels,
        })
    }

    /// Clusters response (epoch + cluster list).
    pub fn clusters_response(&self) -> Result<ClusterResponse> {
        let rows = self.sqlite.get_file_clusters()?;
        let epoch = self.sqlite.get_clusters_epoch()?.unwrap_or_default();
        let mut by_cluster: BTreeMap<i64, Vec<&crate::store::sqlite::FileCluster>> =
            BTreeMap::new();
        for row in &rows {
            by_cluster.entry(row.cluster_id).or_default().push(row);
        }
        let clusters_list: Vec<ClusterInfo> = by_cluster
            .into_iter()
            .map(|(cid, members)| {
                let sample: Vec<String> =
                    members.iter().take(3).map(|m| m.file_id.clone()).collect();
                ClusterInfo {
                    cluster_id: cid,
                    size: members.len(),
                    sample_files: sample,
                }
            })
            .collect();
        Ok(ClusterResponse {
            epoch,
            clusters: clusters_list,
        })
    }

    /// Members of a specific cluster.
    pub fn cluster_members(&self, cluster_id: i64) -> Result<ClusterMemberResponse> {
        let members = self.sqlite.get_cluster_members(cluster_id)?;
        Ok(ClusterMemberResponse {
            cluster_id,
            members: members
                .into_iter()
                .map(|m| ClusterMember {
                    file_id: m.file_id,
                    score: m.score,
                })
                .collect(),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Node-card lexical scoring (NOT chunk scoring — different algorithm)
// ═══════════════════════════════════════════════════════════════════════════

fn node_card_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-z0-9_/-]+").unwrap())
}

/// Extract terms for node-card scoring.  Unlike [`lexical::query_terms`],
/// this does NOT filter stopwords — matching the Python original exactly.
fn node_card_terms(query: &str) -> HashSet<String> {
    let lower = query.to_lowercase();
    node_card_token_re()
        .find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .filter(|term| term.len() >= 3)
        .collect()
}

/// Node-card lexical score — mirrors `query_engine.py::lexical_score` exactly.
fn lexical_score_node(query: &str, card: &NodeCard) -> f64 {
    let terms = node_card_terms(query);
    if terms.is_empty() {
        return 0.0;
    }
    let searchable = [
        card.id.as_str(),
        card.label.as_str(),
        card.area.as_str(),
        card.cluster_semantic.as_str(),
        card.funzione_primaria.as_str(),
        &card.dipende_da.join(" "),
        &card.tecnologie.join(" "),
    ]
    .join(" ")
    .to_lowercase();
    let card_id_lower = card.id.to_lowercase();
    let card_label_lower = card.label.to_lowercase();
    let card_area_lower = card.area.to_lowercase();

    let mut score = 0.0_f64;
    for term in &terms {
        if searchable.contains(term.as_str()) {
            score += 1.0;
        }
        if card_id_lower.contains(term.as_str()) || card_label_lower.contains(term.as_str()) {
            score += 1.5;
        }
        if card_area_lower.contains(term.as_str()) {
            score += 0.75;
        }
    }

    // Provider backend query boost
    if is_provider_backend_query(&terms) && card.id.starts_with("src-tauri/src/backend/") {
        score += 6.0;
        if card.id.ends_with("providers.rs") || card.id.ends_with("commands.rs") {
            score += 3.0;
        }
        if card.id.ends_with("providers.rs")
            && terms.iter().any(|t| {
                matches!(
                    t.as_str(),
                    "container" | "containers" | "cpu" | "serverless" | "scaleway"
                )
            })
        {
            score += 2.0;
        }
    }

    // Frontend view query boost
    if is_frontend_view_query(&terms) && card.id.starts_with("src/components/views/") {
        score += 5.0;
        if terms.iter().any(|t| {
            matches!(
                t.as_str(),
                "oracle" | "graph" | "budget" | "compute" | "secrets" | "providers"
            )
        }) {
            score += 2.0;
        }
    }

    score
}

fn is_provider_backend_query(terms: &HashSet<String>) -> bool {
    let provider_terms: &[&str] = &[
        "cloudflare",
        "scaleway",
        "worker",
        "workers",
        "serverless",
        "gpu",
    ];
    let operation_terms: &[&str] = &[
        "secret",
        "secrets",
        "rotation",
        "rotate",
        "token",
        "inventory",
        "sync",
    ];
    let resource_terms: &[&str] = &[
        "container",
        "containers",
        "cpu",
        "function",
        "functions",
        "vm",
        "instance",
        "instances",
    ];
    let has_provider = provider_terms.iter().any(|t| terms.contains(*t));
    let has_op_or_resource = operation_terms
        .iter()
        .chain(resource_terms.iter())
        .any(|t| terms.contains(*t));
    has_provider && has_op_or_resource
}

fn is_frontend_view_query(terms: &HashSet<String>) -> bool {
    [
        "page",
        "view",
        "screen",
        "frontend",
        "ui",
        "implemented",
        "where",
    ]
    .iter()
    .any(|t| terms.contains(*t))
}

// ═══════════════════════════════════════════════════════════════════════════
// Filter helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Check if a chunk matches structural pre-filters.
/// Mirrors `QueryEngine._chunk_matches_filters`.
fn chunk_matches_filters(
    chunk: &FileChunk,
    kind: Option<&str>,
    language: Option<&str>,
    symbols: Option<&[String]>,
    imports: Option<&[String]>,
    module: Option<&str>,
) -> bool {
    if let Some(filter_kind) = kind {
        let chunk_kind = chunk.kind.to_lowercase();
        if !chunk_kind.is_empty() && chunk_kind != filter_kind.to_lowercase() {
            return false;
        }
    }
    if let Some(filter_lang) = language {
        let chunk_lang = chunk.language.to_lowercase();
        if !chunk_lang.is_empty() && chunk_lang != filter_lang.to_lowercase() {
            return false;
        }
    }
    if let Some(syms) = symbols {
        if !syms.is_empty() {
            let text_lower = chunk.text.to_lowercase();
            let sym_lower = chunk.symbol_name.to_lowercase();
            let symbols_used_lower: Vec<String> = chunk
                .symbols_used
                .iter()
                .map(|s| s.to_lowercase())
                .collect();
            let any_match = syms.iter().any(|s| {
                let sl = s.to_lowercase();
                text_lower.contains(&sl) || sl == sym_lower || symbols_used_lower.contains(&sl)
            });
            if !any_match {
                return false;
            }
        }
    }
    if let Some(imps) = imports {
        if !imps.is_empty() {
            let syms_str = chunk.symbols_used.join(" ").to_lowercase();
            let text_lower = chunk.text.to_lowercase();
            let any_match = imps.iter().any(|imp| {
                let il = imp.to_lowercase();
                syms_str.contains(&il) || text_lower.contains(&il)
            });
            if !any_match {
                return false;
            }
        }
    }
    if let Some(filter_module) = module {
        // Python: `chunk.get("file_id") or chunk.get("file_sorgente") or ""`.
        let file_id = if chunk.file_id.is_empty() {
            chunk.file_sorgente.to_lowercase()
        } else {
            chunk.file_id.to_lowercase()
        };
        if !file_id.contains(&filter_module.to_lowercase()) {
            return false;
        }
    }
    true
}

// ═══════════════════════════════════════════════════════════════════════════
// Group-by-file
// ═══════════════════════════════════════════════════════════════════════════

/// Group context chunks and results by file.
/// Mirrors `QueryEngine._group_by_file`.
fn group_by_file_fn(context_chunks: &[ContextChunk], results: &[ResultEntry]) -> Vec<GroupEntry> {
    let mut by_file: HashMap<String, GroupEntry> = HashMap::new();

    for chunk in context_chunks {
        let f = &chunk.file_source;
        if f.is_empty() {
            continue;
        }
        let entry = by_file.entry(f.clone()).or_insert_with(|| GroupEntry {
            file: f.clone(),
            total_score: 0.0,
            chunks: Vec::new(),
            kind: String::new(),
            symbol_name: String::new(),
            signature: String::new(),
            language: String::new(),
            line_start: 0,
            line_end: 0,
        });
        entry.total_score += chunk.score;
        let text_truncated = if chunk.text.len() > 500 {
            chunk.text[..500].to_string()
        } else {
            chunk.text.clone()
        };
        entry.chunks.push(GroupChunk {
            chunk_id: chunk.chunk_id.clone(),
            score: chunk.score,
            retrieval: chunk.retrieval.clone(),
            start_char: chunk.start_char,
            end_char: chunk.end_char,
            text: text_truncated,
        });
    }

    // Augment with result metadata
    for r in results {
        let f = &r.file_source;
        if let Some(entry) = by_file.get_mut(f) {
            if entry.kind.is_empty() {
                entry.kind = r.kind.clone();
                entry.symbol_name = r.symbol_name.clone();
                entry.signature = r.signature.clone();
                entry.language = r.language.clone();
                entry.line_start = r.line_start;
                entry.line_end = r.line_end;
            }
        }
    }

    let mut grouped: Vec<GroupEntry> = by_file.into_values().collect();
    grouped.sort_by(|a, b| {
        b.total_score
            .partial_cmp(&a.total_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    grouped
}

// ═══════════════════════════════════════════════════════════════════════════
// Conversion helpers
// ═══════════════════════════════════════════════════════════════════════════

impl ContextChunk {
    fn from_file_chunk(chunk: &FileChunk, score: f64, retrieval: &str) -> Self {
        Self {
            chunk_id: chunk.id.clone(),
            file_source: chunk.file_sorgente.clone(),
            chunk_index: chunk.chunk_index as usize,
            start_char: chunk.start_char as usize,
            end_char: chunk.end_char as usize,
            score,
            retrieval: retrieval.to_string(),
            text: chunk.text.clone(),
            last_modified: chunk.ultima_modifica.clone(),
            kind: chunk.kind.clone(),
            symbol_name: chunk.symbol_name.clone(),
            signature: chunk.signature.clone(),
            language: chunk.language.clone(),
            line_start: chunk.line_start as usize,
            line_end: chunk.line_end as usize,
            symbols_used: chunk.symbols_used.clone(),
        }
    }
}

fn file_chunk_to_scored(c: &FileChunk) -> ScoredChunk {
    ScoredChunk {
        id: c.id.clone(),
        file_id: c.file_id.clone(),
        file_sorgente: c.file_sorgente.clone(),
        text: c.text.clone(),
        chunk_index: c.chunk_index as usize,
        start_char: c.start_char as usize,
        end_char: c.end_char as usize,
        kind: c.kind.clone(),
        symbol_name: c.symbol_name.clone(),
        signature: c.signature.clone(),
        language: c.language.clone(),
        line_start: c.line_start as usize,
        line_end: c.line_end as usize,
        symbols_used: serde_json::to_string(&c.symbols_used).unwrap_or_default(),
        area: String::new(),
        cluster_semantic: String::new(),
        label: String::new(),
    }
}

fn summarize_chunk(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "Chunk-level match from the full-file Oracle index.".to_string();
    }
    let cleaned: String = trimmed.split_whitespace().collect::<Vec<&str>>().join(" ");
    if cleaned.len() <= 420 {
        cleaned
    } else {
        cleaned[..420].to_string()
    }
}

fn parse_cluster(value: &str) -> i64 {
    value.parse::<i64>().unwrap_or(0)
}

/// Degraded answer when no ContextAnswerer is available.
fn degraded_answer(_query: &str, _context: &[ContextChunk]) -> AnswerPayload {
    AnswerPayload {
        answer: "not found in corpus.".to_string(),
        citations: Vec::new(),
        not_found: true,
        suggested_path: None,
        answer_source: Some("not_found".to_string()),
        fallback_reason: Some("LLM answerer not available at this layer.".to_string()),
        llm_provider: None,
        llm_model: None,
    }
}

/// `active_query_profile()` — mirrors
/// `oracle/ingestion/retrieval_text.py::active_query_profile`.
fn active_query_profile() -> String {
    let raw = env::var("ORACLE_QUERY_PROFILE").unwrap_or_else(|_| {
        env::var("ORACLE_EMBED_PROFILE").unwrap_or_else(|_| "semantic-prefix-v2".to_string())
    });
    let profile = raw.trim().to_lowercase();
    if config::SEMANTIC_PROFILE_NAMES.contains(&profile.as_str()) {
        "semantic-prefix-v2".to_string()
    } else {
        "raw".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cluster() {
        assert_eq!(parse_cluster("42"), 42);
        assert_eq!(parse_cluster("abc"), 0);
        assert_eq!(parse_cluster(""), 0);
    }

    #[test]
    fn test_summarize_chunk() {
        assert_eq!(
            summarize_chunk(""),
            "Chunk-level match from the full-file Oracle index."
        );
        assert_eq!(summarize_chunk("  hello  "), "hello");
        let long = "x".repeat(500);
        assert_eq!(summarize_chunk(&long).len(), 420);
    }

    #[test]
    fn test_node_card_terms_includes_stopwords() {
        let terms = node_card_terms("What is the architecture of this project?");
        // "the" (3 chars) is a stopword but node_card_terms does NOT filter it
        assert!(terms.contains("the"));
        assert!(terms.contains("what"));
        assert!(terms.contains("architecture"));
        assert!(terms.contains("project"));
    }

    #[test]
    fn test_provider_backend_query() {
        let terms: HashSet<String> = ["cloudflare", "worker", "secret"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(is_provider_backend_query(&terms));

        let terms2: HashSet<String> = ["cloudflare", "hello"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(!is_provider_backend_query(&terms2));
    }

    #[test]
    fn test_frontend_view_query() {
        let terms: HashSet<String> = ["view", "page"].iter().map(|s| s.to_string()).collect();
        assert!(is_frontend_view_query(&terms));

        let terms2: HashSet<String> = ["hello", "world"].iter().map(|s| s.to_string()).collect();
        assert!(!is_frontend_view_query(&terms2));
    }
}
