use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleDuplicateLabel {
    pub label: String,
    #[serde(alias = "node_ids")]
    pub node_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleSnapshot {
    pub status: String,
    pub source: String,
    pub phase: String,
    #[serde(alias = "node_count")]
    pub node_count: usize,
    #[serde(alias = "edge_count")]
    pub edge_count: usize,
    #[serde(alias = "cluster_count")]
    pub cluster_count: usize,
    #[serde(alias = "duplicate_labels")]
    pub duplicate_labels: Vec<OracleDuplicateLabel>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleCoverage {
    #[serde(alias = "total_nodes")]
    pub total_nodes: usize,
    #[serde(alias = "oracle_nodes")]
    pub oracle_nodes: usize,
    #[serde(alias = "oracle_percent")]
    pub oracle_percent: f64,
}

/// A single file recorded in the chunk-index manifest. `path` is the
/// workspace-RELATIVE file id (the manifest never stores absolute paths), so
/// nothing here leaks an absolute filesystem path to the UI.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleIndexedFile {
    pub path: String,
    #[serde(default)]
    pub chunks: u32,
    #[serde(default, alias = "updated_at")]
    pub updated_at: String,
}

/// A bounded, paginated page of indexed files for the Oracle UI. Mirrors the
/// Python `GET /index/files` response (`{total, files, limit, offset}`).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleIndexedFiles {
    pub total: u32,
    pub files: Vec<OracleIndexedFile>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleRuntimeVectorStore {
    pub backend: String,
    pub path: String,
    pub records: usize,
    pub ready: bool,
}

/// Status of the CHUNK store (`chunks.lancedb` + the SQLite chunk table) — the
/// REAL dense-retrieval index that `/ask` and `/context` use. The legacy
/// node-level `vector_store` (`vectors.lancedb`) is no longer produced and is
/// typically empty, so readiness and the displayed counts must come from HERE,
/// not from `vector_store`. All fields are `#[serde(default)]` so an older
/// Python sidecar that omits the `chunk_store` block deserializes into a
/// not-ready, zero-count store rather than failing the whole payload.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleRuntimeChunkStore {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub path: String,
    /// Distinct indexed files (from the SQLite chunk table).
    #[serde(default)]
    pub files: usize,
    /// Chunk rows in the SQLite chunk table.
    #[serde(default)]
    pub records: usize,
    /// Vector rows in `chunks.lancedb`.
    #[serde(default, alias = "vector_records")]
    pub vector_records: usize,
    /// True iff the chunk store holds vectors AND the matching SQLite rows
    /// exist — i.e. dense retrieval can actually answer.
    #[serde(default)]
    pub ready: bool,
}

/// VESTIGIAL: the local Ollama chat path has been removed (answers are
/// API-only). This struct is retained only so the `OracleRuntime` wire payload
/// emitted by the Python sidecar keeps a stable shape; it is always populated
/// empty/disabled and is no longer surfaced in the UI.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleRuntimeOllama {
    pub cli: Option<String>,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub model: String,
    #[serde(default, alias = "model_available")]
    pub model_available: bool,
    #[serde(default)]
    pub models: Vec<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleRuntime {
    #[serde(alias = "vector_store")]
    pub vector_store: OracleRuntimeVectorStore,
    /// The REAL dense-retrieval index status. Authoritative for readiness and
    /// for the file/chunk counts the UI shows. Defaulted so an older sidecar
    /// payload without this block still deserializes (as not-ready).
    #[serde(default, alias = "chunk_store")]
    pub chunk_store: OracleRuntimeChunkStore,
    /// Top-level retrieval readiness mirrored from the chunk store by the Python
    /// `/runtime` endpoint. Defaulted so older payloads (which omit it) fall back
    /// to `false`; the UI also derives readiness from `chunk_store.ready`, so
    /// either signal is sufficient.
    #[serde(default)]
    pub ready: bool,
    // Vestigial: kept for wire-payload compatibility; defaults to disabled.
    #[serde(default)]
    pub ollama: OracleRuntimeOllama,
    #[serde(default, alias = "setup_commands")]
    pub setup_commands: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleResult {
    pub id: String,
    pub label: String,
    #[serde(rename = "type", alias = "node_type")]
    pub node_type: String,
    pub cluster: u32,
    pub score: f64,
    #[serde(alias = "file_source")]
    pub file_source: String,
    #[serde(alias = "function_primary")]
    pub function_primary: String,
    #[serde(alias = "dipende_da")]
    pub dependencies: Vec<String>,
    #[serde(default, alias = "chunk_id")]
    pub chunk_id: Option<String>,
    #[serde(default, alias = "chunk_index")]
    pub chunk_index: Option<usize>,
    #[serde(default, alias = "start_char")]
    pub start_char: Option<usize>,
    #[serde(default, alias = "end_char")]
    pub end_char: Option<usize>,
    #[serde(default, alias = "chunk_preview")]
    pub chunk_preview: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleCitation {
    #[serde(rename = "ref", default)]
    pub reference: String,
    #[serde(alias = "file_source")]
    pub file_source: String,
    #[serde(alias = "chunk_id")]
    pub chunk_id: String,
    #[serde(default, alias = "chunk_index")]
    pub chunk_index: Option<usize>,
    #[serde(default, alias = "start_char")]
    pub start_char: Option<usize>,
    #[serde(default, alias = "end_char")]
    pub end_char: Option<usize>,
    #[serde(default)]
    pub retrieval: String,
    #[serde(default)]
    pub score: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleAnswer {
    pub mode: String,
    pub query: String,
    pub summary: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub citations: Vec<OracleCitation>,
    #[serde(default, alias = "not_found")]
    pub not_found: bool,
    #[serde(default, alias = "suggested_path")]
    pub suggested_path: Option<String>,
    #[serde(default, alias = "answer_source")]
    pub answer_source: Option<String>,
    // The reason an answer degraded to the extractive (retrieval-only) fallback —
    // the ONLY fallback. There is no LLM-to-LLM fallback.
    #[serde(default, alias = "fallback_reason")]
    pub fallback_reason: Option<String>,
    #[serde(default, alias = "llm_provider")]
    pub llm_provider: Option<String>,
    #[serde(default, alias = "llm_model")]
    pub llm_model: Option<String>,
    pub results: Vec<OracleResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OracleNodeCard {
    pub id: String,
    pub label: String,
    pub area: String,
    #[serde(alias = "cluster_semantic")]
    pub cluster_semantic: String,
    pub funzione_primaria: String,
    pub espone_api: Vec<String>,
    #[serde(alias = "dipende_da")]
    pub dipende_da: Vec<String>,
    #[serde(alias = "used_by")]
    pub used_by: Vec<String>,
    pub simile_a: Vec<String>,
    pub tecnologie: Vec<String>,
    #[serde(alias = "file_sorgente")]
    pub file_sorgente: String,
    #[serde(alias = "ultima_modifica")]
    pub ultima_modifica: Option<String>,
    pub source: String,
    #[serde(alias = "embedding_dims")]
    pub embedding_dims: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Python `/runtime` payload (snake_case) with a populated CHUNK store
    /// but an EMPTY legacy vector_store must deserialize so that READINESS comes
    /// from the chunk store, NOT the empty vector store. This is the exact
    /// scenario from the live probe (vectors.lancedb empty, chunks.lancedb full)
    /// that previously made the UI show "not ready / 0 chunks".
    #[test]
    fn runtime_readiness_comes_from_chunk_store_not_empty_vector_store() {
        let payload = serde_json::json!({
            "vector_store": {
                "backend": "lancedb",
                "path": "oracle-data/vectors.lancedb",
                "records": 0,
                "ready": false
            },
            "chunk_store": {
                "backend": "lancedb",
                "path": "oracle-data/chunks.lancedb",
                "files": 1314,
                "records": 4177,
                "vector_records": 4177,
                "ready": true
            },
            "ready": true,
            "ollama": {"server": "removed", "model_available": false},
            "setup_commands": []
        });
        let runtime: OracleRuntime =
            serde_json::from_value(payload).expect("deserialize /runtime payload");

        // The legacy vector store is empty / not ready...
        assert!(!runtime.vector_store.ready);
        assert_eq!(runtime.vector_store.records, 0);
        // ...but the chunk store (the real index) is ready with the real counts.
        assert!(runtime.chunk_store.ready);
        assert_eq!(runtime.chunk_store.records, 4177);
        assert_eq!(runtime.chunk_store.files, 1314);
        assert_eq!(runtime.chunk_store.vector_records, 4177);
        // The top-level mirror agrees with the chunk store.
        assert!(runtime.ready);
    }

    /// An older sidecar payload WITHOUT a `chunk_store` block (or top-level
    /// `ready`) must still deserialize, defaulting the chunk store to not-ready /
    /// zero counts rather than failing the whole payload.
    #[test]
    fn runtime_without_chunk_store_defaults_to_not_ready() {
        let payload = serde_json::json!({
            "vector_store": {
                "backend": "lancedb",
                "path": "p",
                "records": 0,
                "ready": false
            },
            "ollama": {"server": "removed", "model_available": false},
            "setup_commands": []
        });
        let runtime: OracleRuntime =
            serde_json::from_value(payload).expect("deserialize legacy payload");
        assert!(!runtime.chunk_store.ready);
        assert_eq!(runtime.chunk_store.records, 0);
        assert!(!runtime.ready);
    }

    /// A genuinely empty workspace (both stores empty) is correctly not ready.
    #[test]
    fn runtime_empty_workspace_is_not_ready() {
        let payload = serde_json::json!({
            "vector_store": {"backend": "lancedb", "path": "v", "records": 0, "ready": false},
            "chunk_store": {
                "backend": "lancedb", "path": "c", "files": 0, "records": 0,
                "vector_records": 0, "ready": false
            },
            "ready": false,
            "ollama": {"server": "removed", "model_available": false},
            "setup_commands": []
        });
        let runtime: OracleRuntime =
            serde_json::from_value(payload).expect("deserialize empty payload");
        assert!(!runtime.chunk_store.ready);
        assert!(!runtime.ready);
    }
}
