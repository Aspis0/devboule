//! Golden-byte-parity tests for `oracle_core::ingest::retrieval_text`.
//!
//! Loads frozen fixtures produced by the Python pipeline and asserts
//! byte-equal output from the Rust port.

use std::collections::HashMap;
use std::fs;

use serde::Deserialize;

use oracle_core::ingest::retrieval_text::{
    chunk_embedding_text, classify_domains, classify_source_kind, query_embedding_text, ChunkMeta,
};

// ── Fixtures ──────────────────────────────────────────────────────────────

fn fixture_path(relative: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    format!("{}/{}", manifest, relative)
}

fn load_string(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e))
}

// ── Deserialization helpers ───────────────────────────────────────────────

/// A raw chunk as it appears in `golden/fixtures/chunks.json`.
#[derive(Debug, Clone, Deserialize)]
struct RawChunk {
    file_id: String,
    file_sorgente: String,
    text: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    symbol_name: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    line_start: i64,
    #[serde(default)]
    line_end: i64,
    #[serde(default)]
    symbols_used: String,
    #[serde(default)]
    chunk_index: usize,
    #[serde(default)]
    id: String,
}

impl From<RawChunk> for ChunkMeta {
    fn from(r: RawChunk) -> Self {
        ChunkMeta {
            file_id: r.file_id,
            file_sorgente: r.file_sorgente,
            text: r.text,
            kind: r.kind,
            symbol_name: r.symbol_name,
            language: r.language,
            line_start: r.line_start,
            line_end: r.line_end,
            symbols_used: r.symbols_used,
            chunk_index: r.chunk_index,
            id: r.id,
        }
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddingTexts {
    chunks: HashMap<String, String>,
    queries: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ClassifyExpected {
    domains: Vec<String>,
    source_kind: String,
}

// ── Diff helper ───────────────────────────────────────────────────────────

/// On byte mismatch, print the first divergence point with 80-char context
/// from both expected and actual.
fn assert_byte_eq(expected: &str, actual: &str, label: &str) {
    if expected == actual {
        return;
    }
    let eb = expected.as_bytes();
    let ab = actual.as_bytes();
    let len = eb.len().min(ab.len());
    let mut diff_pos = len; // default: first mismatch at min-len
    for i in 0..len {
        if eb[i] != ab[i] {
            diff_pos = i;
            break;
        }
    }
    let ctx_start = diff_pos.saturating_sub(40);
    let ctx_end = (diff_pos + 40).min(eb.len());
    let ctx_start_a = diff_pos.saturating_sub(40);
    let ctx_end_a = (diff_pos + 40).min(ab.len());

    let exp_ctx = String::from_utf8_lossy(&eb[ctx_start..ctx_end]);
    let act_ctx = String::from_utf8_lossy(&ab[ctx_start_a..ctx_end_a]);

    panic!(
        "BYTE DIVERGENCE in {} at byte offset {}:\n\
         Expected len={}, Actual len={}\n\
         Expected context [...{}...]: {:?}\n\
         Actual   context [...{}...]: {:?}\n\
         Expected byte: 0x{:02x} ({})\n\
         Actual   byte: 0x{:02x} ({})",
        label,
        diff_pos,
        eb.len(),
        ab.len(),
        ctx_start,
        exp_ctx,
        ctx_start_a,
        act_ctx,
        eb.get(diff_pos).copied().unwrap_or(0),
        eb.get(diff_pos)
            .map(|b| String::from_utf8_lossy(&[*b]).to_string())
            .unwrap_or_default(),
        ab.get(diff_pos).copied().unwrap_or(0),
        ab.get(diff_pos)
            .map(|b| String::from_utf8_lossy(&[*b]).to_string())
            .unwrap_or_default(),
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────

/// Golden test: every chunk's `chunk_embedding_text` output must match
/// `embedding_texts.json::chunks` byte-for-byte.
#[test]
fn golden_chunk_embedding_texts() {
    let chunks_json = load_string(&fixture_path("golden/fixtures/chunks.json"));
    let emb_json = load_string(&fixture_path("golden/fixtures/embedding_texts.json"));

    let raw_chunks: HashMap<String, Vec<RawChunk>> =
        serde_json::from_str(&chunks_json).expect("Failed to parse chunks.json");
    let emb: EmbeddingTexts =
        serde_json::from_str(&emb_json).expect("Failed to parse embedding_texts.json");

    let mut tested = 0;
    let mut missing = Vec::new();

    for file_chunks in raw_chunks.values() {
        for raw in file_chunks {
            let chunk_id = raw.id.clone();
            let expected = match emb.chunks.get(&chunk_id) {
                Some(s) => s,
                None => {
                    missing.push(chunk_id);
                    continue;
                }
            };
            let meta: ChunkMeta = ChunkMeta::from((*raw).clone());
            let actual = chunk_embedding_text(&meta, Some("semantic-prefix-v2"));
            assert_byte_eq(expected, &actual, &chunk_id);
            tested += 1;
        }
    }

    assert!(
        missing.is_empty(),
        "Chunks missing from embedding_texts.json: {:?}",
        missing
    );
    assert!(tested > 0, "No chunks were tested — check fixture loading");
}

/// Golden test: every query's `query_embedding_text` output must match
/// `embedding_texts.json::queries` byte-for-byte.
#[test]
fn golden_query_embedding_texts() {
    let queries_json = load_string(&fixture_path("golden/queries.json"));
    let emb_json = load_string(&fixture_path("golden/fixtures/embedding_texts.json"));

    let queries: Vec<String> =
        serde_json::from_str(&queries_json).expect("Failed to parse queries.json");
    let emb: EmbeddingTexts =
        serde_json::from_str(&emb_json).expect("Failed to parse embedding_texts.json");

    for query in &queries {
        let expected = emb
            .queries
            .get(query.as_str())
            .unwrap_or_else(|| panic!("Query missing from embedding_texts.json: {:?}", query));
        let actual = query_embedding_text(query, Some("semantic-prefix-v2"));
        assert_byte_eq(expected, &actual, &format!("query: {}", query));
    }
}

/// Golden test: `classify_domains` + `classify_source_kind` for every
/// collected file must match `classify.json` (domains sorted, source_kind exact).
/// Reads full file text from the golden corpus to match how the fixture was generated.
#[test]
fn golden_classify() {
    let chunks_json = load_string(&fixture_path("golden/fixtures/chunks.json"));
    let classify_json = load_string(&fixture_path("golden/fixtures/classify.json"));

    let raw_chunks: HashMap<String, Vec<RawChunk>> =
        serde_json::from_str(&chunks_json).expect("Failed to parse chunks.json");
    let expected_map: HashMap<String, ClassifyExpected> =
        serde_json::from_str(&classify_json).expect("Failed to parse classify.json");

    let corpus_root = fixture_path("golden/corpus");

    // Collect unique file IDs (keys of the chunks map).
    let mut file_ids: Vec<String> = raw_chunks.keys().cloned().collect();
    file_ids.sort();

    for file_id in &file_ids {
        // Read the full source file from the corpus.
        let source_path = format!("{}/{}", corpus_root, file_id);
        let text = match fs::read_to_string(&source_path) {
            Ok(t) => t,
            Err(_) => {
                // Fallback: concatenate chunk texts (imperfect but won't match fixture).
                let chunks = &raw_chunks[file_id];
                chunks
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };

        // classify_source_kind
        let actual_kind = classify_source_kind(file_id);
        if let Some(expected) = expected_map.get(file_id) {
            assert_eq!(
                expected.source_kind, actual_kind,
                "classify_source_kind mismatch for {}",
                file_id
            );
        }

        // classify_domains (insertion order, as returned by the function)
        let actual_domains = classify_domains(file_id, &text);
        if let Some(expected) = expected_map.get(file_id) {
            // The fixture stores domains in sorted order.
            // Our function returns them in insertion order.
            // Compare as sorted sets to verify membership.
            let mut expected_sorted = expected.domains.clone();
            expected_sorted.sort();
            let mut actual_sorted = actual_domains.clone();
            actual_sorted.sort();
            assert_eq!(
                expected_sorted, actual_sorted,
                "classify_domains mismatch for {}",
                file_id
            );
        }
    }
}
