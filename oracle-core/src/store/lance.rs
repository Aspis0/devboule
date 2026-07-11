//! LanceDB-backed vector store, plus the deterministic hash-embedding fallback.
//!
//! Port of `oracle/store/lance_store.py::LanceStore`. Two backends are
//! supported, mirroring the Python original:
//!   * `.lancedb` paths → LanceDB directory with a single `"nodes"` table;
//!   * `.json` paths → a plain JSON file used as a deterministic fallback.
//!
//! The connection is cached process-wide, keyed by the path string
//! (mirroring Python's `_CONNECTION_CACHE`). Vector rows are
//! `id` / `label` / `area` / `cluster_semantic` / `vector`, where `vector`
//! is a `FixedSizeList<Float32, 1024>` (see the note in `p1-recon-spec.md` §2:
//! Python writes a variable `List<Float32>` because it brute-forces the cosine
//! in Python, but the Rust port uses LanceDB's native nearest-neighbour query,
//! which requires a fixed-size list vector column).

use anyhow::{Context, Result};
use arrow_array::{Array, ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::DistanceType;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Tokenizer for `hash_embed`, byte-identical to `lance_store.py::TOKEN_RE`.
fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9_/-]+").unwrap())
}

/// 8-byte blake2b digest, matching `hashlib.blake2b(digest_size=8)`.
fn blake2b_8(data: &[u8]) -> [u8; 8] {
    let mut h = Blake2bVar::new(8).expect("blake2b with 8-byte digest");
    h.update(data);
    let mut out = [0u8; 8];
    h.finalize_variable(&mut out)
        .expect("finalizing blake2b digest");
    out
}

/// Deterministic hash embedding — the blake2b fallback.
///
/// Port of `oracle/store/lance_store.py::embed_text`: lowercase the text, split
/// on `[A-Za-z0-9_/-]+`, for each token take the first 4 digest bytes as a
/// little-endian index (`% dims`) and the 5th byte's parity as the sign
/// (`+1.0` even / `-1.0` odd), accumulate, then L2-normalize (guarded so a
/// zero vector stays zero). Returns a unit vector of length `dims`.
pub fn hash_embed(text: &str, dims: usize) -> Vec<f32> {
    let mut vector = vec![0.0f32; dims];
    for token in token_re().find_iter(&text.to_lowercase()) {
        let digest = blake2b_8(token.as_str().as_bytes());
        let index =
            u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize % dims;
        let sign = if digest[4].is_multiple_of(2) { 1.0f32 } else { -1.0f32 };
        vector[index] += sign;
    }
    let sum_sq = vector.iter().map(|v| v * v).sum::<f32>();
    let norm = sum_sq.sqrt();
    let norm = if norm == 0.0 { 1.0 } else { norm };
    vector.iter().map(|v| v / norm).collect()
}

/// Cosine similarity as a raw dot product (vectors are pre-normalized).
/// Mirrors `lance_store.py::cosine`.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// One stored vector row (`"nodes"` table / JSON file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanceRow {
    pub id: String,
    pub label: String,
    pub area: String,
    pub cluster_semantic: String,
    pub vector: Vec<f32>,
}

/// One search/similarity hit (`id`, `label`, `area`, `cluster_semantic`, score).
#[derive(Debug, Clone)]
pub struct LanceHit {
    pub id: String,
    pub label: String,
    pub area: String,
    pub cluster_semantic: String,
    /// Cosine similarity in `[-1, 1]` (`1.0 - _distance` for the LanceDB
    /// native `DistanceType::Cosine` query).
    pub score: f32,
}

/// Backend selector, mirroring the Python `backend` attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Json,
    Lance,
}

/// Vector store. Mirrors `oracle/store/lance_store.py::LanceStore`.
pub struct LanceStore {
    path: PathBuf,
    backend: Backend,
}

type ConnCache = Mutex<HashMap<String, Arc<lancedb::Connection>>>;
static CONN_CACHE: OnceLock<ConnCache> = OnceLock::new();

fn conn_cache() -> &'static ConnCache {
    CONN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl LanceStore {
    /// Create the store, creating the parent directory eagerly. Mirrors
    /// `lance_store.py::__init__` (`.json` suffix → JSON backend).
    pub fn new(path: &Path) -> Self {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let backend = match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Backend::Json,
            _ => Backend::Lance,
        };
        LanceStore {
            path: path.to_path_buf(),
            backend,
        }
    }

    /// Path to the underlying store.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Default Arrow schema for the `"nodes"` table (1024-dim vector column).
    pub fn default_schema() -> SchemaRef {
        lance_schema(crate::config::EMBED_DIMS)
    }

    /// Get (or lazily create) the process-cached connection for this path.
    /// Mirrors the double-checked locking in `lance_store.py::_connect`.
    async fn connection(&self) -> Result<Arc<lancedb::Connection>> {
        let key = self.path.to_string_lossy().to_string();
        if let Some(c) = conn_cache().lock().unwrap().get(&key).cloned() {
            return Ok(c);
        }
        let conn = lancedb::connect(&key)
            .execute()
            .await
            .with_context(|| format!("connecting to LanceDB at {}", self.path.display()))?;
        let conn = Arc::new(conn);
        conn_cache()
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| conn.clone());
        Ok(conn)
    }

    /// Open the `"nodes"` table, or `None` if missing / on any error.
    /// Mirrors `lance_store.py::_open_lance_table` (which returns `None`
    /// on *any* exception). LanceDB's `open_table` yields
    /// `Error::TableNotFound` when the table is absent, which `Ok().ok()`
    /// collapses to `None`.
    async fn open_table(&self) -> Option<lancedb::Table> {
        if !self.path.exists() {
            return None;
        }
        let conn = self.connection().await.ok()?;
        conn.open_table("nodes").execute().await.ok()
    }

    /// Ensure the `"nodes"` table exists with the given schema.
    /// Mirrors `db.create_table("nodes", data=…, mode="overwrite")` but for an
    /// empty table (used by `ensure_table` in the spec).
    pub async fn ensure_table(&self, schema: SchemaRef) -> Result<()> {
        if self.backend == Backend::Json {
            return Ok(());
        }
        if self.open_table().await.is_some() {
            return Ok(());
        }
        let conn = self.connection().await?;
        conn.create_empty_table("nodes", schema)
            .execute()
            .await
            .with_context(|| format!("creating empty 'nodes' table in {}", self.path.display()))?;
        Ok(())
    }

    /// Build a `RecordBatch` from rows, with `vector` as `FixedSizeList<f32, dims>`.
    fn to_batch(&self, records: &[LanceRow]) -> Result<RecordBatch> {
        let dims = crate::config::EMBED_DIMS;
        let n = records.len();
        let ids: StringArray = records
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<String>>()
            .into();
        let labels: StringArray = records
            .iter()
            .map(|r| r.label.clone())
            .collect::<Vec<String>>()
            .into();
        let areas: StringArray = records
            .iter()
            .map(|r| r.area.clone())
            .collect::<Vec<String>>()
            .into();
        let clusters: StringArray = records
            .iter()
            .map(|r| r.cluster_semantic.clone())
            .collect::<Vec<String>>()
            .into();
        let mut flat: Vec<f32> = Vec::with_capacity(n * dims);
        for r in records {
            for v in &r.vector {
                flat.push(*v);
            }
        }
        let values = Float32Array::from(flat);
        let list_field = Arc::new(Field::new("item", DataType::Float32, true));
        let vector = FixedSizeListArray::try_new(list_field, dims as i32, Arc::new(values), None)
            .context("building vector column")?;
        let schema = lance_schema(dims);
        let cols: Vec<ArrayRef> = vec![
            Arc::new(ids),
            Arc::new(labels),
            Arc::new(areas),
            Arc::new(clusters),
            Arc::new(vector),
        ];
        RecordBatch::try_new(schema, cols).context("building record batch")
    }

    /// Read every row, mirroring `lance_store.py::_read`.
    pub async fn read_all(&self) -> Result<Vec<LanceRow>> {
        match self.backend {
            Backend::Json => {
                if !self.path.exists() {
                    return Ok(Vec::new());
                }
                let text = std::fs::read_to_string(&self.path)
                    .with_context(|| format!("reading json store {}", self.path.display()))?;
                let rows: Vec<LanceRow> = serde_json::from_str(&text).unwrap_or_default();
                Ok(rows)
            }
            Backend::Lance => {
                let Some(table) = self.open_table().await else {
                    return Ok(Vec::new());
                };
                let stream = table
                    .query()
                    .execute()
                    .await
                    .context("scanning nodes table")?;
                let batches: Vec<RecordBatch> = stream
                    .try_collect()
                    .await
                    .context("collecting nodes rows")?;
                Ok(batches_to_rows(&batches))
            }
        }
    }

    /// Total row count. Mirrors `lance_store.py::count` (native `count_rows`
    /// with a full-scan fallback on error).
    pub async fn count(&self) -> Result<usize> {
        match self.backend {
            Backend::Json => Ok(self.read_all().await?.len()),
            Backend::Lance => {
                let Some(table) = self.open_table().await else {
                    return Ok(0);
                };
                match table.count_rows(None).await {
                    Ok(n) => Ok(n),
                    Err(_) => Ok(self.read_all().await?.len()),
                }
            }
        }
    }

    /// Cosine-similarity search by query vector. Mirrors `lance_store.py::search`
    /// with LanceDB's native `DistanceType::Cosine` (`score = 1.0 - _distance`).
    pub async fn search(&self, query_vec: &[f32], limit: usize) -> Result<Vec<LanceHit>> {
        let limit = limit.max(1);
        match self.backend {
            Backend::Json => {
                let rows = self.read_all().await?;
                let mut hits: Vec<LanceHit> = rows
                    .iter()
                    .map(|r| LanceHit {
                        id: r.id.clone(),
                        label: r.label.clone(),
                        area: r.area.clone(),
                        cluster_semantic: r.cluster_semantic.clone(),
                        score: cosine(&r.vector, query_vec),
                    })
                    .collect();
                sort_hits(&mut hits);
                hits.truncate(limit);
                Ok(hits)
            }
            Backend::Lance => {
                let Some(table) = self.open_table().await else {
                    return Ok(Vec::new());
                };
                let stream = table
                    .query()
                    .nearest_to(query_vec.to_vec())?
                    .distance_type(DistanceType::Cosine)
                    .limit(limit)
                    .execute()
                    .await
                    .context("nearest-neighbour search")?;
                let batches: Vec<RecordBatch> = stream
                    .try_collect()
                    .await
                    .context("collecting search batches")?;
                Ok(batches_to_hits(&batches))
            }
        }
    }

    /// Cosine-similarity neighbors of a stored row, excluding the source.
    /// Mirrors `lance_store.py::similar`.
    pub async fn similar(&self, id: &str, limit: usize) -> Result<Vec<LanceHit>> {
        let limit = limit.max(1);
        let all = self.read_all().await?;
        let Some(source) = all.iter().find(|r| r.id == id).map(|r| r.vector.clone()) else {
            return Ok(Vec::new());
        };
        match self.backend {
            Backend::Json => {
                let mut hits: Vec<LanceHit> = all
                    .iter()
                    .filter(|r| r.id != id)
                    .map(|r| LanceHit {
                        id: r.id.clone(),
                        label: r.label.clone(),
                        area: r.area.clone(),
                        cluster_semantic: r.cluster_semantic.clone(),
                        score: cosine(&r.vector, &source),
                    })
                    .collect();
                sort_hits(&mut hits);
                hits.truncate(limit);
                Ok(hits)
            }
            Backend::Lance => {
                let Some(table) = self.open_table().await else {
                    return Ok(Vec::new());
                };
                let stream = table
                    .query()
                    .nearest_to(source)?
                    .distance_type(DistanceType::Cosine)
                    .limit(limit + 1)
                    .execute()
                    .await
                    .context("nearest-neighbour similar")?;
                let batches: Vec<RecordBatch> = stream
                    .try_collect()
                    .await
                    .context("collecting similar batches")?;
                let mut hits = batches_to_hits(&batches);
                hits.retain(|h| h.id != id);
                hits.truncate(limit);
                Ok(hits)
            }
        }
    }

    /// Append (or merge by id) rows. Mirrors the incremental write path of
    /// `lance_store.py::_write_all` / `upsert`.
    pub async fn add(&self, records: &[LanceRow]) -> Result<()> {
        match self.backend {
            Backend::Json => {
                let mut existing: HashMap<String, LanceRow> = self
                    .read_all()
                    .await?
                    .into_iter()
                    .map(|r| (r.id.clone(), r))
                    .collect();
                for r in records {
                    existing.insert(r.id.clone(), r.clone());
                }
                self.write_json(&existing.into_values().collect::<Vec<_>>())
            }
            Backend::Lance => {
                if records.is_empty() {
                    return Ok(());
                }
                let batch = self.to_batch(records)?;
                if let Some(table) = self.open_table().await {
                    table
                        .add(batch)
                        .execute()
                        .await
                        .context("adding rows to nodes table")?;
                } else {
                    let conn = self.connection().await?;
                    conn.create_table("nodes", batch)
                        .mode(lancedb::database::CreateTableMode::Overwrite)
                        .execute()
                        .await
                        .context("creating nodes table")?;
                }
                Ok(())
            }
        }
    }

    /// Delete the union of `delete_ids` and the incoming record ids, then add
    /// the records (creating the table if necessary). Mirrors
    /// `lance_store.py::replace_ids` (batched SQL `IN` delete, 200/batch).
    pub async fn replace_ids(&self, delete_ids: &[String], records: &[LanceRow]) -> Result<()> {
        let mut delete_set: BTreeSet<String> = delete_ids.iter().cloned().collect();
        for r in records {
            delete_set.insert(r.id.clone());
        }
        let union: Vec<String> = delete_set.into_iter().collect();

        match self.backend {
            Backend::Json => {
                let existing: HashMap<String, LanceRow> = self
                    .read_all()
                    .await?
                    .into_iter()
                    .map(|r| (r.id.clone(), r))
                    .collect();
                let filtered: Vec<LanceRow> = existing
                    .into_values()
                    .filter(|r| !union.contains(&r.id))
                    .collect();
                let mut merged: HashMap<String, LanceRow> =
                    filtered.into_iter().map(|r| (r.id.clone(), r)).collect();
                for r in records {
                    merged.insert(r.id.clone(), r.clone());
                }
                self.write_json(&merged.into_values().collect::<Vec<_>>())
            }
            Backend::Lance => {
                let _ = std::fs::create_dir_all(&self.path);
                if let Some(table) = self.open_table().await {
                    if !union.is_empty() {
                        for batch in union.chunks(200) {
                            let quoted: Vec<String> =
                                batch.iter().map(|s| quote_sql_string(s)).collect();
                            let predicate = format!("id IN ({})", quoted.join(","));
                            if table.delete(&predicate).await.is_err() {
                                // Fallback: full read + filter + overwrite write.
                                let existing: Vec<LanceRow> = self
                                    .read_all()
                                    .await?
                                    .into_iter()
                                    .filter(|r| !union.contains(&r.id))
                                    .collect();
                                let mut merged: HashMap<String, LanceRow> =
                                    existing.into_iter().map(|r| (r.id.clone(), r)).collect();
                                for r in records {
                                    merged.insert(r.id.clone(), r.clone());
                                }
                                let conn = self.connection().await?;
                                let batch =
                                    self.to_batch(&merged.into_values().collect::<Vec<_>>())?;
                                conn.create_table("nodes", batch)
                                    .mode(lancedb::database::CreateTableMode::Overwrite)
                                    .execute()
                                    .await
                                    .context("replace_ids fallback write")?;
                                return Ok(());
                            }
                        }
                    }
                }
                if records.is_empty() {
                    return Ok(());
                }
                let batch = self.to_batch(records)?;
                if let Some(table) = self.open_table().await {
                    table
                        .add(batch)
                        .execute()
                        .await
                        .context("replace_ids add")?;
                } else {
                    let conn = self.connection().await?;
                    conn.create_table("nodes", batch)
                        .mode(lancedb::database::CreateTableMode::Overwrite)
                        .execute()
                        .await
                        .context("replace_ids create")?;
                }
                Ok(())
            }
        }
    }

    /// Write the JSON backend file.
    fn write_json(&self, records: &[LanceRow]) -> Result<()> {
        let text = serde_json::to_string(records).context("serializing json store")?;
        std::fs::write(&self.path, text)
            .with_context(|| format!("writing json store {}", self.path.display()))?;
        Ok(())
    }
}

/// SQL-quote a string id by doubling single quotes (SQL-injection safe).
/// Mirrors `lance_store.py::_quote_sql_string`.
fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Sort hits by descending score then ascending id (matches the Python
/// `sort(key=lambda x: (-score, id))`).
fn sort_hits(hits: &mut [LanceHit]) {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Arrow schema for the `"nodes"` table.
fn lance_schema(dims: usize) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, true),
        Field::new("area", DataType::Utf8, true),
        Field::new("cluster_semantic", DataType::Utf8, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dims as i32,
            ),
            false,
        ),
    ]))
}

/// Read a nullable string cell.
fn str_at(arr: &StringArray, i: usize) -> Option<String> {
    if arr.is_null(i) {
        None
    } else {
        Some(arr.value(i).to_string())
    }
}

/// Read a `FixedSizeList<f32>` cell into a `Vec<f32>`.
fn vector_at(arr: &FixedSizeListArray, i: usize) -> Vec<f32> {
    let size = arr.value_length() as usize;
    let values = arr
        .values()
        .as_any()
        .downcast_ref::<Float32Array>()
        .expect("vector column is Float32");
    let start = i * size;
    (0..size).map(|j| values.value(start + j)).collect()
}

/// Convert scanned batches into `LanceRow`s.
fn batches_to_rows(batches: &[RecordBatch]) -> Vec<LanceRow> {
    let mut out = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        let ids = batch
            .column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let labels = batch
            .column_by_name("label")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let areas = batch
            .column_by_name("area")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let clusters = batch
            .column_by_name("cluster_semantic")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let vectors = batch
            .column_by_name("vector")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());
        for i in 0..n {
            let id = ids.and_then(|a| str_at(a, i)).unwrap_or_default();
            let label = labels.and_then(|a| str_at(a, i)).unwrap_or_default();
            let area = areas.and_then(|a| str_at(a, i)).unwrap_or_default();
            let cluster = clusters.and_then(|a| str_at(a, i)).unwrap_or_default();
            let vector = vectors.map(|v| vector_at(v, i)).unwrap_or_default();
            out.push(LanceRow {
                id,
                label,
                area,
                cluster_semantic: cluster,
                vector,
            });
        }
    }
    out
}

/// Convert scanned batches (with a `_distance` column) into `LanceHit`s.
/// `score = 1.0 - _distance` for `DistanceType::Cosine`.
fn batches_to_hits(batches: &[RecordBatch]) -> Vec<LanceHit> {
    let mut out = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        let ids = batch
            .column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let labels = batch
            .column_by_name("label")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let areas = batch
            .column_by_name("area")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let clusters = batch
            .column_by_name("cluster_semantic")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let dist = batch
            .column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
        for i in 0..n {
            let id = ids.and_then(|a| str_at(a, i)).unwrap_or_default();
            let label = labels.and_then(|a| str_at(a, i)).unwrap_or_default();
            let area = areas.and_then(|a| str_at(a, i)).unwrap_or_default();
            let cluster = clusters.and_then(|a| str_at(a, i)).unwrap_or_default();
            let score = dist
                .map(|a| {
                    if a.is_null(i) {
                        0.0
                    } else {
                        1.0 - a.value(i) as f64
                    }
                })
                .unwrap_or(0.0) as f32;
            out.push(LanceHit {
                id,
                label,
                area,
                cluster_semantic: cluster,
                score,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::approx_constant)] // frozen Python-parity values
    fn hash_embed_matches_python() {
        // Expected values captured from oracle/store/lance_store.py::embed_text
        // via the Python interpreter (full-precision, first non-zero entries).
        let cases: &[(&str, f32, &[(usize, f32)])] = &[
            (
                "hello world",
                0.9999999999999999,
                &[(344, -0.70710678), (679, -0.70710678)],
            ),
            (
                "fn compute(x: int) -> int",
                0.9999999999999999,
                &[
                    (150, 0.35355339),
                    (227, 0.70710678),
                    (254, 0.35355339),
                    (706, -0.35355339),
                    (842, -0.35355339),
                ],
            ),
            (
                "SELECT * FROM nodes",
                1.0,
                &[(252, -0.57735027), (271, 0.57735027), (833, 0.57735027)],
            ),
        ];
        for (text, exp_norm, nz) in cases {
            let v = hash_embed(text, 1024);
            assert_eq!(v.len(), 1024, "dim mismatch for {}", text);
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(
                (norm - exp_norm).abs() < 1e-6,
                "norm mismatch for {}: got {}, want {}",
                text,
                norm,
                exp_norm
            );
            for i in 0..1024 {
                let expected = nz
                    .iter()
                    .find(|(idx, _)| *idx == i)
                    .map(|(_, val)| *val)
                    .unwrap_or(0.0);
                assert!(
                    (v[i] - expected).abs() < 1e-6,
                    "value mismatch at dim {} for {}: got {}, want {}",
                    i,
                    text,
                    v[i],
                    expected
                );
            }
        }
    }

    #[test]
    fn hash_embed_is_unit_norm_and_zero_safe() {
        let v = hash_embed("", 1024);
        assert_eq!(v.len(), 1024);
        assert!(v.iter().all(|x| *x == 0.0), "empty text -> zero vector");
    }
}
