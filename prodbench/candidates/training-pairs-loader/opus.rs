use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single self-improvement training pair as stored on Devboule's local
/// training rail (`.aspis-training/*.jsonl`).
///
/// Each JSONL line is one pair of a `rejected` and a `chosen` completion,
/// annotated with where it came from and how it was scored. Every field is
/// `#[serde(default)]` so a line missing a key still deserializes (the missing
/// field falls back to its type default — an empty string or `false`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrainingPair {
    /// Where this pair originated (e.g. the loop/agent that produced it).
    #[serde(default)]
    pub origin: String,
    /// The gate or check that yielded this pair.
    #[serde(default)]
    pub gate: String,
    /// The rejected (worse) completion.
    #[serde(default)]
    pub rejected: String,
    /// The chosen (better) completion.
    #[serde(default)]
    pub chosen: String,
    /// The scorer that ranked the two completions.
    #[serde(default)]
    pub scorer: String,
    /// Whether this pair was produced without an LLM judge (deterministic gate).
    #[serde(default)]
    pub judge_free: bool,
}

/// Load every training pair from the `.jsonl` files in `dir`.
///
/// Each file whose name ends in `.jsonl` is read and parsed line by line. Each
/// non-empty line is deserialized as a [`TrainingPair`]; lines that are blank or
/// fail to parse are silently skipped. Files that are not `*.jsonl` are ignored.
///
/// This function never panics: a missing or unreadable directory, an unreadable
/// file, or malformed content all simply contribute no pairs and yield an empty
/// (or partial) result.
pub fn load_training_pairs(dir: &Path) -> Vec<TrainingPair> {
    let mut pairs = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return pairs,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Only consider regular `*.jsonl` files.
        if !path.is_file() {
            continue;
        }
        let is_jsonl = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));
        if !is_jsonl {
            continue;
        }

        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(pair) = serde_json::from_str::<TrainingPair>(line) {
                pairs.push(pair);
            }
        }
    }

    pairs
}
