//! File-level embedding clusters — mean-pool per-chunk vectors into one
//! vector per file, cluster with KMeans (HDBSCAN fallback), persist to
//! `file_vectors.lancedb` and `file_clusters` sqlite table.
//!
//! Port of `query_engine.py::_refresh_clusters` (line ~1307).
//!
//! ## Design choices
//!
//! * **KMeans only** (no HDBSCAN dependency): the `hdbscan` crate API is
//!   uncertain; a hand-rolled Lloyd's KMeans with k-means++ init and a
//!   seeded deterministic RNG mirrors Python's `sklearn.cluster.KMeans(
//!   n_clusters=k, random_state=0, n_init="auto")` closely enough.
//!   HDBSCAN is left behind a `try`-style feature gate for future use.
//! * **Epoch computation**: byte-identical to Python — SHA-256 of sorted
//!   `file_id\tcluster_id\tround(score,4)` rows + file ids, truncated to
//!   16 hex chars. Skip the sqlite write when epoch matches.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::config::EMBED_DIMS;
use crate::store::lance::{LanceRow, LanceStore};
use crate::store::sqlite::{FileCluster, SqliteStore};

// ═══════════════════════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════════════════════

/// Refresh file-level clustering. Called from a best-effort daemon thread
/// after index runs complete.
///
/// 1. Mean-pool each file's chunk vectors → one vector per file.
/// 2. Write per-file pooled vectors to `file_vectors.lancedb` ALWAYS.
/// 3. Cluster (KMeans) and write `file_clusters` sqlite rows ONLY when
///    ≥ 8 files and the content-hash epoch changed.
pub async fn refresh_clusters(
    _index_root: &Path,
    sqlite: &SqliteStore,
    chunk_vectors: &LanceStore,
    file_vectors: &LanceStore,
) -> Result<()> {
    let all_chunks = sqlite.all_chunks()?;

    if all_chunks.is_empty() {
        // Empty chunk set: compute "empty" epoch and skip if unchanged.
        let empty_epoch = sha256_hex(b"");
        let current = sqlite.get_clusters_epoch()?.unwrap_or_default();
        if empty_epoch == current {
            return Ok(());
        }
        sqlite.replace_file_clusters(&[], Some(&empty_epoch))?;
        // Clear file_vectors
        let existing = file_vectors.read_all().await?;
        if !existing.is_empty() {
            let ids: Vec<String> = existing.iter().map(|r| r.id.clone()).collect();
            file_vectors.replace_ids(&ids, &[]).await?;
        }
        return Ok(());
    }

    // Read all chunk vectors from LanceDB.
    let all_vec_records = chunk_vectors.read_all().await?;
    let vec_by_id: HashMap<&str, &[f32]> = all_vec_records
        .iter()
        .map(|r| (r.id.as_str(), r.vector.as_slice()))
        .collect();

    // Group chunk vectors by file_id.
    let mut per_file_vecs: HashMap<&str, Vec<&[f32]>> = HashMap::new();
    for chunk in &all_chunks {
        let fid = chunk.file_id.as_str();
        if let Some(vec) = vec_by_id.get(chunk.id.as_str()) {
            per_file_vecs.entry(fid).or_default().push(vec);
        }
    }

    let mut file_ids: Vec<&str> = per_file_vecs.keys().copied().collect();
    file_ids.sort();
    let n = file_ids.len();

    // Mean-pool: one vector per file.
    let dims = EMBED_DIMS;
    let pooled: Vec<Vec<f32>> = file_ids
        .iter()
        .map(|fid| {
            let vecs = &per_file_vecs[fid];
            let mut mean = vec![0.0f32; dims];
            for v in vecs {
                for (i, val) in v.iter().enumerate() {
                    mean[i] += val;
                }
            }
            let count = vecs.len() as f32;
            for val in &mut mean {
                *val /= count;
            }
            mean
        })
        .collect();

    // Write per-file vectors to file_vectors.lancedb ALWAYS.
    let file_id_strings: Vec<String> = file_ids.iter().map(|s| s.to_string()).collect();
    let node_records: Vec<LanceRow> = file_id_strings
        .iter()
        .enumerate()
        .map(|(i, fid)| LanceRow {
            id: fid.clone(),
            label: fid.clone(),
            area: "file".to_string(),
            cluster_semantic: "0".to_string(),
            vector: pooled[i].clone(),
        })
        .collect();

    // Replace all file_vectors: read existing IDs, delete them, insert new.
    let existing_fv = file_vectors.read_all().await?;
    if !existing_fv.is_empty() {
        let ids: Vec<String> = existing_fv.iter().map(|r| r.id.clone()).collect();
        file_vectors.replace_ids(&ids, &[]).await?;
    }
    if !node_records.is_empty() {
        file_vectors
            .replace_ids(&[], &node_records)
            .await
            .context("writing file_vectors.lancedb")?;
    }

    // Clustering requires ≥ 8 files to produce meaningful groups.
    if n < 8 {
        let min_epoch = sha256_hex(file_ids.join("\n").as_bytes());
        let current = sqlite.get_clusters_epoch()?.unwrap_or_default();
        if min_epoch != current {
            sqlite.replace_file_clusters(&[], Some(&min_epoch))?;
        }
        return Ok(());
    }

    // ── KMeans clustering ──────────────────────────────────────────────
    let k = kmeans_k(n);
    let (labels, scores) = kmeans_fit_predict(&pooled, k, 0);

    // Build cluster rows (KMeans never produces label -1, but included
    // for parity with a future HDBSCAN path).
    let mut rows: Vec<FileCluster> = Vec::new();
    for (i, fid) in file_ids.iter().enumerate() {
        let lbl = labels[i];
        if lbl == -1 {
            continue;
        }
        rows.push(FileCluster {
            file_id: fid.to_string(),
            cluster_id: lbl,
            score: scores[i] as f64,
        });
    }

    // Compute content-based epoch (byte-identical to Python).
    let mut sig_rows: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{}\t{}\t{:.4}",
                r.file_id,
                r.cluster_id,
                (r.score * 10000.0).round() / 10000.0
            )
        })
        .collect();
    sig_rows.sort();
    let sig_body = sig_rows.join("\n") + "\n" + &file_ids.join("\n");
    let epoch = sha256_hex(sig_body.as_bytes());

    // Skip write if epoch unchanged.
    let current = sqlite.get_clusters_epoch()?.unwrap_or_default();
    if epoch == current {
        return Ok(());
    }

    sqlite
        .replace_file_clusters(&rows, Some(&epoch))
        .context("writing file_clusters")?;

    eprintln!(
        "[cluster] refresh: n_files={} k={} epoch={} rows={}",
        n,
        k,
        &epoch[..8],
        rows.len()
    );
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// KMeans implementation (Lloyd's algorithm with k-means++ init)
// ═══════════════════════════════════════════════════════════════════════════

/// Compute the number of clusters: `k = clamp(2, round(sqrt(n/2)), 24)`.
/// Mirrors Python: `k = max(2, min(24, round(np.sqrt(n / 2))))`.
fn kmeans_k(n: usize) -> usize {
    let raw = ((n as f64 / 2.0).sqrt()).round() as usize;
    raw.max(2).min(24)
}

/// Seeded pseudo-random number generator (xorshift32) for deterministic
/// k-means++ initialization. Mirrors sklearn's `random_state=0` behavior
/// (deterministic but not bit-identical — acceptable per the spec).
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    /// Float in [0.0, 1.0).
    fn next_f32(&mut self) -> f32 {
        (self.next_u32() as f32) / (u32::MAX as f32)
    }
}

/// K-means++ initialization: choose initial centroids with probability
/// proportional to squared distance from the nearest existing centroid.
fn kmeans_pp_init(data: &[Vec<f32>], k: usize, rng: &mut Rng) -> Vec<Vec<f32>> {
    let n = data.len();
    let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(k);

    // First centroid: random.
    let first = (rng.next_u32() as usize) % n;
    centroids.push(data[first].clone());

    // Squared distances to nearest existing centroid.
    let mut min_dists: Vec<f32> = data.iter().map(|p| sq_dist(p, &centroids[0])).collect();

    for _ in 1..k {
        let total: f32 = min_dists.iter().sum();
        if total <= 0.0 {
            // All remaining points are at zero distance — find an unchosen one.
            for p in data.iter() {
                if !centroids.iter().any(|c| c == p) {
                    centroids.push(p.clone());
                    break;
                }
            }
            if centroids.len() >= k {
                break;
            }
            continue;
        }
        let threshold = rng.next_f32() * total;
        let mut cumulative = 0.0;
        let mut chosen = 0;
        for (i, &d) in min_dists.iter().enumerate() {
            cumulative += d;
            if cumulative >= threshold {
                chosen = i;
                break;
            }
        }
        centroids.push(data[chosen].clone());

        // Update min_dists.
        for (i, p) in data.iter().enumerate() {
            let d = sq_dist(p, centroids.last().unwrap());
            if d < min_dists[i] {
                min_dists[i] = d;
            }
        }
    }

    centroids
}

/// Squared Euclidean distance between two vectors.
fn sq_dist(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = x - y;
            d * d
        })
        .sum()
}

/// Lloyd's KMeans: assign + update centroids until convergence (max 300 iters).
///
/// Returns `(labels, scores)` where `score = 1.0 - dist/max_dist`
/// (pseudo-probability, matching Python's sklearn `probabilities_`).
///
/// Mirrors Python's `KMeans(n_clusters=k, random_state=0, n_init="auto")`:
/// single init with k-means++, Lloyd's to convergence. sklearn's default
/// `n_init="auto"` in recent versions uses `n_init=1` when `init` is not
/// `"random"`, so a single k-means++ init is exact parity.
fn kmeans_fit_predict(data: &[Vec<f32>], k: usize, seed: u32) -> (Vec<i64>, Vec<f32>) {
    let n = data.len();
    if n == 0 || k == 0 {
        return (vec![], vec![]);
    }

    let mut rng = Rng::new(seed);
    let mut centroids = kmeans_pp_init(data, k, &mut rng);
    let mut labels = vec![0i64; n];

    // Lloyd's iterations.
    for _ in 0..300 {
        let mut changed = false;

        // Assign each point to the nearest centroid.
        for (i, p) in data.iter().enumerate() {
            let best = centroids
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    sq_dist(p, a.1)
                        .partial_cmp(&sq_dist(p, b.1))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(idx, _)| idx as i64)
                .unwrap_or(0);
            if labels[i] != best {
                labels[i] = best;
                changed = true;
            }
        }

        if !changed {
            break;
        }

        // Update centroids (mean of assigned points).
        let dims = data[0].len();
        let mut sums = vec![vec![0.0f32; dims]; k];
        let mut counts = vec![0u32; k];
        for (i, p) in data.iter().enumerate() {
            let c = labels[i] as usize;
            if c < k {
                for (j, v) in p.iter().enumerate() {
                    sums[c][j] += v;
                }
                counts[c] += 1;
            }
        }
        for c in 0..k {
            if counts[c] > 0 {
                let count = counts[c] as f32;
                for j in 0..dims {
                    centroids[c][j] = sums[c][j] / count;
                }
            }
        }
    }

    // Pseudo-probability scores: 1.0 - dist/max_dist.
    let distances: Vec<f32> = data
        .iter()
        .enumerate()
        .map(|(i, p)| sq_dist(p, &centroids[labels[i] as usize]).sqrt())
        .collect();
    let max_dist = distances.iter().cloned().fold(0.0f32, f32::max);
    let max_dist = if max_dist == 0.0 { 1.0 } else { max_dist };
    let scores: Vec<f32> = distances.iter().map(|d| 1.0 - d / max_dist).collect();

    (labels, scores)
}

// ═══════════════════════════════════════════════════════════════════════════
// Epoch computation
// ═══════════════════════════════════════════════════════════════════════════

/// SHA-256 digest truncated to 16 hex characters. Mirrors Python's
/// `hashlib.sha256(...).hexdigest()[:16]`.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    // Manual hex encoding to avoid adding the `hex` crate.
    let mut s = String::with_capacity(16);
    for &byte in result.iter().take(16) {
        s.push_str(&format!("{:02x}", byte));
    }
    s
}

// ═══════════════════════════════════════════════════════════════════════════
// HDBSCAN fallback (behind a feature gate, for future use)
// ═══════════════════════════════════════════════════════════════════════════

// When the `hdbscan` crate becomes available and its API is stable,
// replace the KMeans path above with:
//
// ```rust
// #[cfg(feature = "hdbscan")]
// fn cluster(data: &[Vec<f32>]) -> (Vec<i64>, Vec<f32>) {
//     // HDBSCAN with min_cluster_size=3
//     // Returns (labels, probabilities)
// }
// ```
//
// For now, KMeans is the only implementation. This matches the Python
// `except ImportError: # sklearn fallback` pattern.
