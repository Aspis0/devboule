//! Oracle store-layer configuration constants and path resolution.
//!
//! Mirrors the subset of `oracle/config.py` consumed by the store layer
//! (`SQLiteStore`, `LanceStore`, the chunk-index manifest) plus the
//! chunk-profile version logic from `oracle/ingestion/retrieval_text.py`.

use std::env;
use std::path::{Path, PathBuf};

/// Embedding dimensionality shared by the vector stores and the hash fallback.
/// Mirrors `oracle/config.py::EMBED_DIMS`.
pub const EMBED_DIMS: usize = 1024;

/// Store directory / file names (relative to the Oracle data dir).
/// Mirrors the `*_PATH` constants in `oracle/config.py`.
pub const VECTORS_DIR: &str = "vectors.lancedb";
pub const CHUNKS_DIR: &str = "chunks.lancedb";
pub const FILE_VECTORS_DIR: &str = "file_vectors.lancedb";
pub const METADATA_SQLITE: &str = "metadata.sqlite";
pub const CHUNK_MANIFEST: &str = "chunk-index-manifest.json";

/// Env var selecting the real (Qwen) query embedder vs. the hash fallback.
/// Mirrors `ORACLE_QUERY_EMBEDDER` usage in `oracle/store/lance_store.py`.
pub const ENV_QUERY_EMBEDDER: &str = "ORACLE_QUERY_EMBEDDER";

/// Env var forcing the real (Qwen) embedder; blocks the hash fallback.
/// Mirrors `ORACLE_REQUIRE_REAL_EMBEDDER` in `oracle/ingestion/embedder.py`.
pub const ENV_REQUIRE_REAL_EMBEDDER: &str = "ORACLE_REQUIRE_REAL_EMBEDDER";

/// Default Oracle data directory name (relative to the workspace root).
/// Mirrors `ORACLE_DIR = Path(os.getenv("ORACLE_DIR", "oracle-data"))`.
pub const DEFAULT_ORACLE_DIR: &str = "oracle-data";

// ── Chunk-profile version constants ────────────────────────────────────────
// Byte-identical to `oracle/ingestion/retrieval_text.py`.

/// Raw (non-semantic-prefix) chunk-profile version string.
pub const RAW_CHUNK_PROFILE_VERSION: &str = "adaptive-qwen3-2026-05-28";

/// Semantic-prefix chunk-profile version string (the default).
pub const SEMANTIC_PREFIX_PROFILE_VERSION: &str = "semantic-prefix-qwen3-2026-06-02-c2500";

/// Profile names that normalize to the semantic-prefix profile.
/// Mirrors `SEMANTIC_PROFILE_NAMES` in `oracle/ingestion/retrieval_text.py`.
pub const SEMANTIC_PROFILE_NAMES: &[&str] =
    &["semantic-prefix-v2", "semantic_prefix_v2", "semantic", "v2"];

/// Mirrors `oracle/ingestion/retrieval_text.py::normalize_profile`.
fn normalize_profile(value: &str) -> String {
    let profile = value.trim().to_lowercase();
    if SEMANTIC_PROFILE_NAMES.contains(&profile.as_str()) {
        "semantic-prefix-v2".to_string()
    } else {
        "raw".to_string()
    }
}

/// Mirrors `oracle/ingestion/retrieval_text.py::active_embed_profile`
/// (defaults to `"semantic-prefix-v2"` when `ORACLE_EMBED_PROFILE` is unset).
fn active_embed_profile() -> String {
    let raw = env::var("ORACLE_EMBED_PROFILE").unwrap_or_else(|_| "semantic-prefix-v2".to_string());
    normalize_profile(&raw)
}

/// Active chunk-profile version string.
///
/// Mirrors `oracle/ingestion/retrieval_text.py::active_chunk_profile_version`.
/// With no `profile` override and the default `ORACLE_EMBED_PROFILE`
/// (`"semantic-prefix-v2"`) this returns exactly
/// `"semantic-prefix-qwen3-2026-06-02-c2500"`.
pub fn active_chunk_profile_version(profile: Option<&str>) -> String {
    let effective = match profile {
        Some(p) => normalize_profile(p),
        None => active_embed_profile(),
    };
    if effective == "semantic-prefix-v2" {
        SEMANTIC_PREFIX_PROFILE_VERSION.to_string()
    } else {
        RAW_CHUNK_PROFILE_VERSION.to_string()
    }
}

/// Whether the real (Qwen) embedder is hard-required.
///
/// Mirrors `oracle/ingestion/embedder.py::require_real_embedder`
/// (`ORACLE_REQUIRE_REAL_EMBEDDER` in `{"1","true","yes"}`).
pub fn require_real_embedder() -> bool {
    match env::var(ENV_REQUIRE_REAL_EMBEDDER) {
        Ok(v) => matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

/// Whether the hash query embedder is explicitly selected.
///
/// Mirrors the `ORACLE_QUERY_EMBEDDER=hash` debug knob in
/// `oracle/store/lance_store.py::embed_query_text`.
pub fn query_embedder_is_hash() -> bool {
    matches!(
        env::var(ENV_QUERY_EMBEDDER).as_deref(),
        Ok("hash") | Ok("HASH")
    )
}

/// Canonical set of store paths under a workspace root.
///
/// Mirrors the `*_PATH` constants in `oracle/config.py`, resolved beneath
/// `<root>/oracle-data/` (or the `ORACLE_DIR` env override when set to an
/// absolute path). Each field is the exact path used by the corresponding
/// store module.
#[derive(Debug, Clone)]
pub struct OracleDataPaths {
    /// The Oracle data directory (`<root>/oracle-data`).
    pub root: PathBuf,
    /// Node/CKG vector store (`vectors.lancedb`).
    pub vectors: PathBuf,
    /// Chunk vector store (`chunks.lancedb`).
    pub chunks: PathBuf,
    /// Per-file vector store (`file_vectors.lancedb`).
    pub file_vectors: PathBuf,
    /// Metadata SQLite database (`metadata.sqlite`).
    pub metadata: PathBuf,
    /// Chunk-index manifest (`chunk-index-manifest.json`).
    pub manifest: PathBuf,
}

impl OracleDataPaths {
    /// Compute every store path beneath `<root>/oracle-data/`.
    ///
    /// Mirrors `oracle/config.py`: the data dir is `ORACLE_DIR` when that env
    /// var is an absolute path, otherwise `<root>/<ORACLE_DIR>`. Each sub-path
    /// honors its own env override (`LANCE_DB_PATH`, `CHUNK_DB_PATH`, …) when
    /// set, otherwise falls back to the conventional name under the data dir.
    pub fn from_root(root: &Path) -> Self {
        let data_dir = match env::var("ORACLE_DIR") {
            Ok(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
            Ok(dir) => root.join(dir),
            Err(_) => root.join(DEFAULT_ORACLE_DIR),
        };
        OracleDataPaths {
            vectors: env_or(&["LANCE_DB_PATH"], data_dir.join(VECTORS_DIR)),
            chunks: env_or(&["CHUNK_DB_PATH"], data_dir.join(CHUNKS_DIR)),
            file_vectors: env_or(&["FILE_VECTORS_DB_PATH"], data_dir.join(FILE_VECTORS_DIR)),
            metadata: env_or(&["SQLITE_PATH"], data_dir.join(METADATA_SQLITE)),
            manifest: env_or(&["CHUNK_MANIFEST_PATH"], data_dir.join(CHUNK_MANIFEST)),
            root: data_dir,
        }
    }
}

/// Resolve `path` from the first set env var, else `default`.
fn env_or(keys: &[&str], default: PathBuf) -> PathBuf {
    for key in keys {
        if let Ok(v) = env::var(key) {
            if !v.is_empty() {
                return PathBuf::from(v);
            }
        }
    }
    default
}
