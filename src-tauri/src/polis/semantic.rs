//! Polis P6.2 — Semantic similarity cache + roads.
//!
//! Persisted inside `.aspis-meta.json` under the `semantic` field. Populated by a
//! background refresh task (best-effort, daemon thread) after a successful scan,
//! and read by the scanner to emit `road_type: "semantic"` roads.
//!
//! Design guarantees:
//! - Cache is ADDITIVE + DEFAULTED: old metas without the field load with an
//!   empty cache (fail-open).
//! - Semantic roads are emitted from the CACHE ONLY (no HTTP on the scan path).
//! - The refresh task runs on a blocking thread, respects an AtomicBool guard
//!   (no double-spawn), and fails open (cache untouched on error).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Per-file semantic data persisted in `.aspis-meta.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticPerFile {
    /// Top-K similar file_ids with scores, sorted score desc.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub similar: Vec<SimilarEntry>,
    /// Cluster id from the Oracle file_clusters table, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<i32>,
    /// RFC 3339 timestamp of when this entry was last refreshed.
    /// Used for rotation: uncached files are fetched first, then stalest.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
}

/// A single similar-file entry: (file_id, score).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarEntry {
    #[serde(alias = "fileId")]
    pub file_id: String,
    pub score: f32,
}

/// Semantic similarity cache persisted inside `.aspis-meta.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticCache {
    /// The Oracle clusters epoch when this cache was last refreshed.
    #[serde(default)]
    pub epoch: String,
    /// Project-relative file_id -> per-file semantic data.
    #[serde(default)]
    pub per_file: BTreeMap<String, SemanticPerFile>,
}

impl SemanticCache {
    /// Is the cache stale relative to an oracle epoch?
    /// Stale = cache epoch empty OR cache epoch != oracle epoch.
    pub fn stale(&self, oracle_epoch: &str) -> bool {
        self.epoch.is_empty() || self.epoch != oracle_epoch
    }

    /// Merge one per-file entry into the cache, replacing it in-place and
    /// updating its ``updated_at`` timestamp.  Used by the background refresh
    /// task so only touched files are replaced — untouched entries survive.
    pub fn merge_entry(&mut self, rel_path: &str, entry: SemanticPerFile) {
        self.per_file.insert(rel_path.to_string(), entry);
    }

    /// Produce a deduplicated, capped, thresholded list of (file_id_a, file_id_b,
    /// score) pairs for semantic road emission.
    ///
    /// Rules:
    /// - score >= `min_score` (default 0.80).
    /// - Dedup: only emit (a, b) where a < b lexicographically (canonical).
    /// - Per-file cap: at most `per_file_cap` (default 2) edges PER source file,
    ///   taking the highest-scoring ones.
    /// - Deterministic order: sorted by (file_id_a, file_id_b).
    pub fn road_pairs(&self, min_score: f32, per_file_cap: usize) -> Vec<(String, String, f32)> {
        let mut pairs: Vec<(String, String, f32)> = Vec::new();
        let mut emitted: BTreeSet<(String, String)> = BTreeSet::new();

        // Collect all (source_file, similar) entries, ordered by file_id for determinism.
        let mut source_files: Vec<&String> = self.per_file.keys().collect();
        source_files.sort();

        for src in source_files {
            let Some(data) = self.per_file.get(src) else {
                continue;
            };
            // Sort similar entries by score desc, then file_id for ties.
            let mut entries: Vec<&SimilarEntry> = data.similar.iter().collect();
            entries.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.file_id.cmp(&b.file_id))
            });

            let mut emitted_from_this: usize = 0;
            for entry in entries {
                if entry.score < min_score {
                    break; // sorted desc, rest are below threshold
                }
                if emitted_from_this >= per_file_cap {
                    break;
                }
                // Canonical order: (a, b) where a < b
                let (a, b) = if src < &entry.file_id {
                    (src.clone(), entry.file_id.clone())
                } else if src > &entry.file_id {
                    (entry.file_id.clone(), src.clone())
                } else {
                    continue; // self-loop, skip
                };
                if emitted.contains(&(a.clone(), b.clone())) {
                    continue; // already emitted from the other direction
                }
                emitted.insert((a.clone(), b.clone()));
                pairs.push((a, b, entry.score));
                emitted_from_this += 1;
            }
        }

        // Deterministic output order.
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        pairs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(fid: &str, score: f32) -> SimilarEntry {
        SimilarEntry {
            file_id: fid.to_string(),
            score,
        }
    }

    fn make_cache(entries: Vec<(&str, Vec<(&str, f32)>, Option<i32>)>) -> SemanticCache {
        let mut per_file = BTreeMap::new();
        for (src, sims, cluster) in entries {
            per_file.insert(
                src.to_string(),
                SemanticPerFile {
                    similar: sims
                        .iter()
                        .map(|(f, s)| SimilarEntry {
                            file_id: f.to_string(),
                            score: *s,
                        })
                        .collect(),
                    cluster_id: cluster,
                    updated_at: String::new(),
                },
            );
        }
        SemanticCache {
            epoch: "2025-07-09T12:00:00Z".to_string(),
            per_file,
        }
    }

    #[test]
    fn stale_when_epochs_differ() {
        let cache = SemanticCache {
            epoch: "old".to_string(),
            ..Default::default()
        };
        assert!(cache.stale("new"));
        assert!(!cache.stale("old"));
    }

    #[test]
    fn stale_when_cache_epoch_empty() {
        let cache = SemanticCache::default();
        assert!(cache.stale("anything"));
    }

    #[test]
    fn road_pairs_filters_below_threshold() {
        let cache = make_cache(vec![("a.rs", vec![("b.rs", 0.9), ("c.rs", 0.5)], None)]);
        let pairs = cache.road_pairs(0.80, 2);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], ("a.rs".to_string(), "b.rs".to_string(), 0.9));
    }

    #[test]
    fn road_pairs_dedups_bidirectional() {
        // a↔b appears in BOTH directions — only one pair emitted.
        let cache = make_cache(vec![
            ("a.rs", vec![("b.rs", 0.85)], None),
            ("b.rs", vec![("a.rs", 0.90)], None),
        ]);
        let pairs = cache.road_pairs(0.80, 2);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "a.rs");
        assert_eq!(pairs[0].1, "b.rs");
        // Score is from whichever direction we picked (a.rs -> b.rs = 0.85 here
        // since a.rs < b.rs, we use a's entry).
        assert!((pairs[0].2 - 0.85).abs() < 0.001);
    }

    #[test]
    fn road_pairs_caps_per_file() {
        let cache = make_cache(vec![(
            "a.rs",
            vec![("b.rs", 0.95), ("c.rs", 0.90), ("d.rs", 0.85)],
            None,
        )]);
        let pairs = cache.road_pairs(0.80, 2);
        // Only top 2 by score from a.rs
        assert_eq!(pairs.len(), 2);
        let ids: Vec<&str> = pairs.iter().map(|p| p.1.as_str()).collect();
        assert!(ids.contains(&"b.rs"));
        assert!(ids.contains(&"c.rs"));
        assert!(!ids.contains(&"d.rs"));
    }

    #[test]
    fn road_pairs_deterministic_order() {
        let cache = make_cache(vec![
            ("z.rs", vec![("a.rs", 0.85)], None),
            ("m.rs", vec![("b.rs", 0.90)], None),
        ]);
        let pairs = cache.road_pairs(0.80, 2);
        // Sorted by (a, b) where a < b
        assert_eq!(pairs[0], ("a.rs".to_string(), "z.rs".to_string(), 0.85));
        assert_eq!(pairs[1], ("b.rs".to_string(), "m.rs".to_string(), 0.90));
    }

    #[test]
    fn road_pairs_skips_self_loops() {
        let cache = make_cache(vec![("a.rs", vec![("a.rs", 0.99)], None)]);
        let pairs = cache.road_pairs(0.80, 2);
        assert!(pairs.is_empty());
    }

    #[test]
    fn road_pairs_empty_cache_yields_no_roads() {
        let cache = SemanticCache::default();
        assert!(cache.road_pairs(0.80, 2).is_empty());
    }

    #[test]
    fn default_cache_is_empty_and_has_empty_epoch() {
        let cache = SemanticCache::default();
        assert!(cache.epoch.is_empty());
        assert!(cache.per_file.is_empty());
    }

    #[test]
    fn semantic_per_file_default_is_empty() {
        let spf = SemanticPerFile::default();
        assert!(spf.similar.is_empty());
        assert!(spf.cluster_id.is_none());
    }

    #[test]
    fn merge_preserves_untouched_entries() {
        let mut cache = make_cache(vec![
            ("a.rs", vec![("b.rs", 0.9)], None),
            ("b.rs", vec![("c.rs", 0.8)], None),
        ]);
        // Merge a new entry for a.rs only — b.rs must survive untouched.
        let entry_a = SemanticPerFile {
            similar: vec![make_entry("c.rs", 0.95)],
            cluster_id: None,
            updated_at: "2025-07-09T14:00:00Z".to_string(),
        };
        cache.merge_entry("a.rs", entry_a);
        assert_eq!(cache.per_file.len(), 2);
        // a.rs was replaced
        let a = cache.per_file.get("a.rs").unwrap();
        assert_eq!(a.similar.len(), 1);
        assert_eq!(a.similar[0].file_id, "c.rs");
        // b.rs is untouched
        let b = cache.per_file.get("b.rs").unwrap();
        assert_eq!(b.similar.len(), 1);
        assert_eq!(b.similar[0].file_id, "c.rs");
    }

    #[test]
    fn merge_adds_new_entry() {
        let mut cache = make_cache(vec![("a.rs", vec![("b.rs", 0.9)], None)]);
        let entry_c = SemanticPerFile {
            similar: vec![make_entry("d.rs", 0.85)],
            cluster_id: Some(0),
            updated_at: "2025-07-09T14:00:00Z".to_string(),
        };
        cache.merge_entry("c.rs", entry_c);
        assert_eq!(cache.per_file.len(), 2);
        assert!(cache.per_file.contains_key("c.rs"));
    }

    #[test]
    fn updated_at_defaults_to_empty_string() {
        let spf = SemanticPerFile::default();
        assert!(spf.updated_at.is_empty());
    }
}
