use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Represents a single training pair from Devboule's self-improvement rail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrainingPair {
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub gate: String,
    #[serde(default)]
    pub rejected: String,
    #[serde(default)]
    pub chosen: String,
    #[serde(default)]
    pub scorer: String,
    #[serde(default)]
    pub judge_free: bool,
}

/// Loads all training pairs from `.jsonl` files in the specified directory.
///
/// Reads every file in `dir` whose name ends in `.jsonl`. For each file,
/// parses it line by line, deserializing each non-empty line. Malformed
/// lines or IO errors are skipped silently. If the directory is missing
/// or unreadable, an empty vector is returned.
pub fn load_training_pairs(dir: &Path) -> Vec<TrainingPair> {
    let mut pairs = Vec::new();

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return pairs,
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        let file_name = match path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => continue,
        };

        if !file_name.ends_with(".jsonl") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(pair) = serde_json::from_str::<TrainingPair>(trimmed) {
                pairs.push(pair);
            }
        }
    }

    pairs
}
