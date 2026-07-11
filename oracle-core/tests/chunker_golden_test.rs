//! Golden test — byte-parity verification of ingest::collect + ingest::chunking
//! against the fixtures produced by `golden/dump_golden.py`.

use oracle_core::ingest::chunking;
use oracle_core::ingest::collect;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("golden")
}

fn corpus_dir() -> PathBuf {
    golden_dir().join("corpus")
}

fn fixtures_dir() -> PathBuf {
    golden_dir().join("fixtures")
}

fn load_json(path: &Path) -> serde_json::Value {
    let data =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    serde_json::from_str(&data).unwrap_or_else(|e| panic!("parse {}: {}", path.display(), e))
}

// ── Test: collect.json ───────────────────────────────────────────────────────

#[test]
fn golden_collect_matches_fixture() {
    let root = corpus_dir();
    let fixture = load_json(&fixtures_dir().join("collect.json"));
    let expected: Vec<String> = fixture
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let collected = collect::collect_text_files(&root);
    let got: Vec<String> = collected
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
                .to_string()
        })
        .collect();

    assert_eq!(
        got.len(),
        expected.len(),
        "collected {} files, expected {}",
        got.len(),
        expected.len()
    );
    for (i, (g, e)) in got.iter().zip(expected.iter()).enumerate() {
        assert_eq!(
            g, e,
            "file mismatch at index {}: got {:?}, expected {:?}",
            i, g, e
        );
    }
}

// ── Test: collect_priority.json ──────────────────────────────────────────────

#[test]
fn golden_collect_priority_matches_fixture() {
    let root = corpus_dir();
    let fixture = load_json(&fixtures_dir().join("collect_priority.json"));
    let expected: HashMap<String, usize> = fixture
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_u64().unwrap() as usize))
        .collect();

    let collected = collect::collect_text_files(&root);
    for path in &collected {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let rank = collect::priority_rank(&rel);
        assert_eq!(
            rank,
            *expected
                .get(&rel)
                .unwrap_or_else(|| panic!("unexpected file in collection: {}", rel)),
            "priority mismatch for {}",
            rel
        );
    }
}

// ── Test: chunks.json (deep equality, field-by-field) ────────────────────────

#[test]
fn golden_chunks_match_fixture() {
    let root = corpus_dir();
    let fixture = load_json(&fixtures_dir().join("chunks.json"));
    let fixture_obj = fixture.as_object().expect("chunks.json must be an object");

    let collected = collect::collect_text_files(&root);

    // Build chunks for every collected file
    let mut all_chunks: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for path in &collected {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let chunks = chunking::build_chunks_for_file(path, &root);
        // Strip volatile fields for comparison
        let clean: Vec<serde_json::Value> = chunks
            .into_iter()
            .map(|c| {
                let obj = c.as_object().unwrap();
                let cleaned: serde_json::Map<String, serde_json::Value> = obj
                    .iter()
                    .filter(|(k, _)| *k != "ultima_modifica" && *k != "embedding_dims")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                serde_json::Value::Object(cleaned)
            })
            .collect();
        all_chunks.insert(rel, clean);
    }

    // Assert every file in fixture matches our output
    for (file_id, expected_chunks) in fixture_obj {
        let got_chunks = all_chunks
            .get(file_id)
            .unwrap_or_else(|| panic!("file {} in fixture but not in collected output", file_id));

        let expected_arr = expected_chunks.as_array().unwrap();
        assert_eq!(
            got_chunks.len(),
            expected_arr.len(),
            "chunk count mismatch for {}: got {}, expected {}",
            file_id,
            got_chunks.len(),
            expected_arr.len()
        );

        for (ci, (got, exp)) in got_chunks.iter().zip(expected_arr.iter()).enumerate() {
            let got_obj = got.as_object().unwrap();
            let exp_obj = exp.as_object().unwrap();

            for (key, exp_val) in exp_obj {
                let got_val = got_obj.get(key).unwrap_or_else(|| {
                    panic!("missing field '{}' in chunk {} of {}", key, ci, file_id)
                });

                if got_val != exp_val {
                    panic!(
                        "field '{}' mismatch in {} chunk {}: got {}, expected {}",
                        key,
                        file_id,
                        ci,
                        serde_json::to_string(got_val).unwrap(),
                        serde_json::to_string(exp_val).unwrap()
                    );
                }
            }

            // Check no extra fields (except volatile ones stripped above)
            for key in got_obj.keys() {
                if key == "ultima_modifica" || key == "embedding_dims" {
                    continue;
                }
                assert!(
                    exp_obj.contains_key(key),
                    "extra field '{}' in chunk {} of {}",
                    key,
                    ci,
                    file_id
                );
            }
        }
    }
}
