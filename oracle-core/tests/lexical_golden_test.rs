//! Golden-verified parity test for the lexical scoring stack.
//!
//! Loads `golden/fixtures/chunks.json` and `golden/fixtures/lexical.json`,
//! then asserts every query × chunk score, terms, expansions, and top-10
//! order match the Python dump exactly.

use std::collections::{HashMap, HashSet};
use std::fs;

use oracle_core::query::lexical::{
    lexical_chunk_score, query_terms, semantic_expansions, ScoredChunk,
};

// ---------------------------------------------------------------------------
// Fixture types
// ---------------------------------------------------------------------------

/// Structure of chunks.json: file_id → list of chunk dicts.
type ChunksFixture = HashMap<String, Vec<ScoredChunk>>;

/// Per-query entry in lexical.json.
#[derive(serde::Deserialize)]
struct LexicalEntry {
    terms: Vec<String>,
    semantic_expansions: Vec<String>,
    chunk_scores: HashMap<String, f64>,
    top10_chunk_ids: Vec<String>,
}

/// The full lexical.json fixture: query string → entry.
type LexicalFixture = HashMap<String, LexicalEntry>;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_chunks() -> ChunksFixture {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{}/golden/fixtures/chunks.json", manifest_dir);
    let data = fs::read_to_string(&path).expect("failed to read chunks.json");
    serde_json::from_str(&data).expect("failed to parse chunks.json")
}

fn load_lexical_fixture() -> LexicalFixture {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{}/golden/fixtures/lexical.json", manifest_dir);
    let data = fs::read_to_string(&path).expect("failed to read lexical.json");
    serde_json::from_str(&data).expect("failed to parse lexical.json")
}

/// Flatten all chunks from the file-grouped fixture into a single Vec, sorted
/// by (file_id, chunk_index) to mirror the Python dump_golden.py order.
fn flatten_chunks(chunks: &ChunksFixture) -> Vec<ScoredChunk> {
    let mut flat: Vec<ScoredChunk> = Vec::new();
    let mut sorted_keys: Vec<&String> = chunks.keys().collect();
    sorted_keys.sort();
    for key in sorted_keys {
        flat.extend(chunks[key].iter().cloned());
    }
    flat
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn golden_terms_match() {
    let fixture = load_lexical_fixture();
    for (query, entry) in &fixture {
        let got_terms = query_terms(query);
        let want_terms: HashSet<String> = entry.terms.iter().cloned().collect();
        assert_eq!(
            got_terms,
            want_terms,
            "terms mismatch for query {:?}: got {:?}, want {:?}",
            query,
            {
                let mut v: Vec<_> = got_terms.iter().cloned().collect();
                v.sort();
                v
            },
            &entry.terms
        );
    }
}

#[test]
fn golden_expansions_match() {
    let fixture = load_lexical_fixture();
    for (query, entry) in &fixture {
        let terms = query_terms(query);
        let got_exp = semantic_expansions(&terms);
        let want_exp: HashSet<String> = entry.semantic_expansions.iter().cloned().collect();
        assert_eq!(
            got_exp,
            want_exp,
            "expansions mismatch for query {:?}: got {:?}, want {:?}",
            query,
            {
                let mut v: Vec<_> = got_exp.iter().cloned().collect();
                v.sort();
                v
            },
            &entry.semantic_expansions
        );
    }
}

#[test]
fn golden_chunk_scores_match() {
    let chunks_fixture = load_chunks();
    let flat_chunks = flatten_chunks(&chunks_fixture);
    let lexical_fixture = load_lexical_fixture();

    // Build a fast lookup: chunk_id → ScoredChunk
    let chunk_map: HashMap<&str, &ScoredChunk> =
        flat_chunks.iter().map(|c| (c.id.as_str(), c)).collect();

    let mut tolerance_needed = false;

    for (query, entry) in &lexical_fixture {
        let terms = query_terms(query);

        for (chunk_id, want_score) in &entry.chunk_scores {
            let chunk = match chunk_map.get(chunk_id.as_str()) {
                Some(c) => c,
                None => {
                    panic!(
                        "chunk_id {:?} not found in chunks.json for query {:?}",
                        chunk_id, query
                    );
                }
            };
            let got_score = lexical_chunk_score(query, &terms, chunk);

            // Try exact f64 comparison first
            if got_score != *want_score {
                // Fall back to tolerance
                let diff = (got_score - want_score).abs();
                if diff > 1e-12 {
                    panic!(
                        "score mismatch for query {:?}, chunk {:?}: got {:.12}, want {:.12} (diff {:.2e})",
                        query, chunk_id, got_score, want_score, diff
                    );
                }
                tolerance_needed = true;
            }
        }
    }

    if tolerance_needed {
        eprintln!("NOTE: some scores required tolerance (abs diff < 1e-12) rather than exact f64 comparison.");
    }
}

#[test]
fn golden_top10_order_match() {
    let chunks_fixture = load_chunks();
    let flat_chunks = flatten_chunks(&chunks_fixture);
    let lexical_fixture = load_lexical_fixture();

    // Build a fast lookup: chunk_id → ScoredChunk
    let _chunk_map: HashMap<&str, &ScoredChunk> =
        flat_chunks.iter().map(|c| (c.id.as_str(), c)).collect();

    for (query, entry) in &lexical_fixture {
        let terms = query_terms(query);

        // Compute scores for all chunks, matching the golden fixture's approach:
        // score > 0, then sort by (-score, chunk_id)
        let mut scored: Vec<(String, f64)> = Vec::new();
        for chunk in &flat_chunks {
            let score = lexical_chunk_score(query, &terms, chunk);
            if score > 0.0 {
                // Round to 6 decimal places to match the Python fixture
                let rounded = (score * 1_000_000.0).round() / 1_000_000.0;
                scored.push((chunk.id.clone(), rounded));
            }
        }
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let got_top10: Vec<&str> = scored.iter().take(10).map(|(id, _)| id.as_str()).collect();

        assert_eq!(
            got_top10,
            entry
                .top10_chunk_ids
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            "top-10 mismatch for query {:?}:\n  got:  {:?}\n  want: {:?}",
            query,
            got_top10,
            entry.top10_chunk_ids
        );
    }
}
