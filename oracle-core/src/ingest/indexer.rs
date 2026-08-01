//! Chunk indexing orchestration — the top-level pipeline that ties together
//! file collection, chunking, embedding, and store writes.
//!
//! Port of `oracle/ingestion/chunk_index.py`. The per-file chunking/collection
//! primitives live in `crate::ingest::{collect, chunking, retrieval_text}`; the
//! store primitives in `crate::store::{sqlite, lance, manifest}`.
//!
//! ## Embedding abstraction
//!
//! All embedding goes through the [`TextEmbedder`] trait, which is implemented
//! by [`crate::embed::EmbedderPool`] for production and by a fake in tests.
//! The pool's `embed` is **synchronous** (single-flight, GPU/CPU saturating);
//! callers running inside an async context should wrap the entire pipeline in
//! `tokio::task::spawn_blocking` so the sync embed call does not starve the
//! async executor.
//!
//! ## RAM / GPU guards
//!
//! - **Low-RAM guard** (binding constraint on Apple Silicon): reads free system
//!   RAM via `sysinfo`; when below `min_free_gb` the pipeline sleeps-and-retries
//!   for a bounded number of cycles, then returns `paused_low_memory` if RAM
//!   does not recover. The floor **fails closed**: unusable/near-zero readings
//!   pause indexing rather than silently proceeding (Metal buffers are wired
//!   and not swappable — low free RAM freezes the machine).
//! - **GPU thermal guard**: polls `nvidia-smi` only. On macOS / Apple Silicon
//!   there is **no thermal guard** (`nvidia-smi` is absent; we do not probe
//!   powermetrics). Memory, not temperature, is the binding constraint there.
//!
//! ## Known divergences from Python
//!
//! 1. `release_embedding_memory()` (CUDA cache flush, model kept resident) has
//!    no Rust equivalent — the pool only supports full unload or none.
//! 2. `effective_chunk_batch_size` defaults to 32 without hardware probing
//!    (Python derives it from `4 × effective_embed_batch_size()`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::{active_chunk_profile_version, EMBED_DIMS};
use crate::embed::{self, CancelFlag};
use crate::ingest::chunking;
use crate::ingest::collect;
use crate::ingest::retrieval_text::{self, ChunkMeta};
use crate::store::lance::{LanceRow, LanceStore};
use crate::store::manifest::{
    self, file_signature, load_manifest, manifest_files_for_root, save_manifest,
    sync_legacy_manifest_root,
};
use crate::store::sqlite::{FileChunk, SqliteStore};

// ═══════════════════════════════════════════════════════════════════════════
// Configuration constants (defaults, mirroring oracle/config.py)
// ═══════════════════════════════════════════════════════════════════════════

/// Files committed per outer index batch. Small (4) on purpose: the UI's
/// `indexed_files / expected_files` counter only advances when a batch commits,
/// so a small batch makes progress visible early instead of sitting at 0 for
/// minutes during the first (slowest) batch. Override via ORACLE_CHUNK_BATCH_FILES.
pub const DEFAULT_BATCH_FILES: usize = 4;
pub const DEFAULT_BATCH_CHUNKS: usize = 8;
pub const DEFAULT_BATCH_CHARS: usize = 50_000;
/// Max attention cost per embed batch: `batch_len × (max_est_tokens)²`.
/// See [`embed::DEFAULT_ATTENTION_BUDGET`]. Override: `ORACLE_CHUNK_ATTENTION_BUDGET`.
pub const DEFAULT_ATTENTION_BUDGET: usize = embed::DEFAULT_ATTENTION_BUDGET;
/// Minimum free RAM (GB) before the indexer hard-pauses. Raised from 5.0 because
/// Metal F16 buffers are wired — 5 GB is not real headroom on a 64 GB host.
pub const DEFAULT_MIN_FREE_GB: f64 = 10.0;
/// Default GPU temp ceiling (°C). Only enforced when `nvidia-smi` is available;
/// on macOS / Apple Silicon this never applies (no thermal probe).
pub const DEFAULT_MAX_GPU_TEMP_C: i32 = 85;
const GPU_COOLDOWN_SECONDS: u64 = 45;
const GPU_COOLDOWN_MAX_CYCLES: usize = 20;
const GPU_RESUME_TEMP_C: i32 = 74;
const LOW_MEMORY_RETRY_SECONDS: u64 = 5;
const LOW_MEMORY_RETRY_CYCLES: usize = 6;

// ═══════════════════════════════════════════════════════════════════════════
// TextEmbedder trait — decoupled from the concrete backend
// ═══════════════════════════════════════════════════════════════════════════

/// Minimal trait for text embedding, decoupled from the concrete backend.
///
/// Implemented by [`crate::embed::EmbedderPool`] for production and by a
/// `FakeEmbedder` in tests.  `embed` must return one L2-normalized vector per
/// input text, in order.
pub trait TextEmbedder: Send + Sync {
    fn embed(
        &self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>>;
}

/// Thin adapter: delegate to `EmbedderPool::embed`.
impl TextEmbedder for crate::embed::EmbedderPool {
    fn embed(
        &self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        crate::embed::EmbedderPool::embed(self, texts, batch_size, cancel)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// IndexerConfig — grouped runtime knobs
// ═══════════════════════════════════════════════════════════════════════════

/// Runtime configuration for the indexing pipeline.
pub struct IndexerConfig {
    /// Number of files per file-batch iteration.
    pub batch_files: usize,
    /// Optional override for chunks per embed call (None → derive).
    pub batch_chunks: Option<usize>,
    /// Max aggregate chars per embed call.
    pub batch_chars: usize,
    /// Max attention cost per embed batch (`batch_len × max_est_tokens²`).
    pub attention_budget: usize,
    /// Minimum free RAM in GB before pausing (0 = disabled).
    pub min_free_gb: f64,
    /// GPU temperature ceiling in °C (None = disabled). On macOS this is
    /// effectively unused — there is no thermal probe (see module docs).
    pub max_gpu_temp_c: Option<i32>,
    /// Max file-batches (in base units) per run (None = unbounded).
    pub max_batches: Option<usize>,
    /// Force re-indexing of all files, ignoring manifest signatures.
    pub force: bool,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            batch_files: env_or_usize(&["ORACLE_CHUNK_BATCH_FILES"], DEFAULT_BATCH_FILES),
            batch_chunks: env_opt_usize("ORACLE_CHUNK_BATCH_CHUNKS"),
            batch_chars: env_or_usize(&["ORACLE_CHUNK_BATCH_CHARS"], DEFAULT_BATCH_CHARS),
            attention_budget: env_or_usize(
                &["ORACLE_CHUNK_ATTENTION_BUDGET"],
                DEFAULT_ATTENTION_BUDGET,
            ),
            min_free_gb: env_or_f64(
                &["ORACLE_CHUNK_MIN_FREE_RAM_GB", "ORACLE_CHUNK_MIN_FREE_GB"],
                DEFAULT_MIN_FREE_GB,
            ),
            max_gpu_temp_c: env_opt_i32("ORACLE_CHUNK_MAX_GPU_TEMP_C")
                .or(Some(DEFAULT_MAX_GPU_TEMP_C)),
            max_batches: None,
            force: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Status / summary types (mirror Python's status_payload shapes)
// ═══════════════════════════════════════════════════════════════════════════

/// Index status (the `status` field in the returned dict).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexStatus {
    Complete,
    PausedLowMemory,
    PausedGpuTemperature,
    PausedBatchLimit,
}

/// Summary returned by [`index_file_chunks`].
#[derive(Debug, Serialize)]
pub struct IndexResult {
    pub status: IndexStatus,
    pub root: String,
    pub sqlite_path: String,
    pub vector_path: String,
    pub manifest_path: String,
    pub scanned: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<usize>,
    pub processed: usize,
    pub chunks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_chunks: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_records: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_ram_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_temp_c: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_gpu_temp_c: Option<i32>,
}

/// Summary returned by [`sync_text_chunks`].
#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub status: String,
    pub root: String,
    pub files: usize,
    pub skipped: usize,
    pub chunks: usize,
    pub sqlite_path: String,
}

/// Summary returned by [`prune_excluded_chunks`].
#[derive(Debug, Serialize)]
pub struct PruneResult {
    pub status: String,
    pub root: String,
    pub removed_files: usize,
    pub removed_vectors: usize,
    pub removed_orphan_vectors: usize,
    pub removed_nodes: usize,
    pub removed_node_vectors: usize,
    pub removed_orphan_node_vectors: usize,
    pub manifest_removed: usize,
    pub sqlite_chunk_files: usize,
    pub sqlite_chunks: usize,
    pub vector_records: usize,
    pub sqlite_nodes: usize,
    pub node_vector_records: usize,
}

/// Status snapshot from [`chunk_index_status`].
#[derive(Debug, Serialize)]
pub struct IndexStatusSnapshot {
    pub root: String,
    pub manifest_path: String,
    pub expected_files: usize,
    pub indexed_files: usize,
    pub pending_files: usize,
    pub stale_files: usize,
    pub sqlite_chunk_files: usize,
    pub sqlite_chunks: usize,
    pub vector_records: usize,
    pub chunk_profile: String,
    pub first_pending: Vec<String>,
    pub first_stale: Vec<String>,
    pub free_gb: f64,
}

// ═══════════════════════════════════════════════════════════════════════════
// Internal helpers
// ═══════════════════════════════════════════════════════════════════════════

/// `path.strip_prefix(root).as_posix()` — POSIX-style relative file id.
fn relative_posix(path: &Path, root: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| anyhow!("path {} not under root {}", path.display(), root.display()))?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

/// UTC mtime as an ISO-8601 string (mirrors Python's `utc_mtime`).
fn utc_mtime_str(path: &Path) -> String {
    let mtime_secs = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dt: DateTime<Utc> = DateTime::from_timestamp(mtime_secs as i64, 0).unwrap_or_default();
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Convert a chunk dict (`serde_json::Value`) to a `ChunkMeta` for
/// `chunk_embedding_text`.
fn chunk_value_to_meta(chunk: &serde_json::Value) -> ChunkMeta {
    let gs = |key: &str| -> String {
        chunk
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let gi = |key: &str| -> i64 { chunk.get(key).and_then(|v| v.as_i64()).unwrap_or(0) };
    ChunkMeta {
        file_id: gs("file_id"),
        file_sorgente: gs("file_sorgente"),
        text: gs("text"),
        kind: gs("kind"),
        symbol_name: gs("symbol_name"),
        language: gs("language"),
        line_start: gi("line_start"),
        line_end: gi("line_end"),
        symbols_used: gs("symbols_used"),
        chunk_index: chunk
            .get("chunk_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        id: gs("id"),
    }
}

/// Convert a chunk dict to a `FileChunk` for SQLite.
fn chunk_value_to_file_chunk(chunk: &serde_json::Value) -> FileChunk {
    let gs = |key: &str| -> String {
        chunk
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let gi = |key: &str| -> i64 { chunk.get(key).and_then(|v| v.as_i64()).unwrap_or(0) };
    let symbols_str = gs("symbols_used");
    let symbols_used: Vec<String> = serde_json::from_str(&symbols_str).unwrap_or_default();
    FileChunk {
        id: gs("id"),
        file_id: gs("file_id"),
        chunk_index: gi("chunk_index"),
        start_char: gi("start_char"),
        end_char: gi("end_char"),
        text: gs("text"),
        file_sorgente: gs("file_sorgente"),
        ultima_modifica: gs("ultima_modifica"),
        embedding_dims: gi("embedding_dims").max(EMBED_DIMS as i64),
        kind: gs("kind"),
        symbol_name: gs("symbol_name"),
        signature: gs("signature"),
        line_start: gi("line_start"),
        line_end: gi("line_end"),
        language: gs("language"),
        symbols_used,
    }
}

/// Convert a chunk dict + vector to a `LanceRow` for LanceDB.
fn chunk_value_to_lance_row(chunk: &serde_json::Value, vector: Vec<f32>) -> LanceRow {
    let gs = |key: &str| -> String {
        chunk
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    LanceRow {
        id: gs("id"),
        label: gs("label"),
        area: gs("area"),
        cluster_semantic: gs("cluster_semantic"),
        vector,
    }
}

/// Enrich chunk dicts with fields the Rust `build_chunks_for_file` omits
/// but the Python version sets (ultima_modifica, embedding_dims, file_sorgente).
fn enrich_chunks(chunks: &mut [serde_json::Value], mtime: &str, file_id: &str) {
    for chunk in chunks {
        if let Some(obj) = chunk.as_object_mut() {
            obj.entry("ultima_modifica".to_string())
                .or_insert_with(|| serde_json::Value::String(mtime.to_string()));
            obj.entry("embedding_dims".to_string())
                .or_insert_with(|| serde_json::Value::Number(EMBED_DIMS.into()));
            obj.entry("file_sorgente".to_string())
                .or_insert_with(|| serde_json::Value::String(file_id.to_string()));
        }
    }
}

/// Conservative token estimate for attention budgeting: `ceil(chars / 3)`.
/// Over-estimating is safe; under-estimating (e.g. chars/4) is what OOM'd Macs.
fn est_tokens_from_chars(chars: usize) -> usize {
    chars.div_ceil(3)
}

/// Attention cost of a candidate batch: `batch_len × (max_est_tokens)²`.
fn batch_attention_cost(batch_len: usize, max_est_tokens: usize) -> usize {
    embed::attention_cost(batch_len, max_est_tokens)
}

/// Yield sub-batches of chunks bounded by `max_chunks`, `max_chars` of embedding
/// text, **and** an attention-cost budget
/// `batch_len × (max_est_tokens_in_batch)² ≤ attention_budget`.
///
/// A linear char budget cannot bound quadratic attention: one long chunk right-pads
/// every sequence in the batch to its length. When a single chunk exceeds the
/// budget alone it still forms its own batch (backends also hard-cap seq len).
fn chunk_batches(
    chunks: &[serde_json::Value],
    max_chunks: usize,
    max_chars: usize,
    attention_budget: usize,
) -> Vec<Vec<&serde_json::Value>> {
    let max_chunks = max_chunks.max(1);
    let max_chars = max_chars.max(1);
    let attention_budget = attention_budget.max(1);
    let mut batches = Vec::new();
    let mut batch: Vec<&serde_json::Value> = Vec::new();
    let mut batch_chars: usize = 0;
    let mut batch_max_tokens: usize = 0;

    for chunk in chunks {
        let meta = chunk_value_to_meta(chunk);
        let text_chars = retrieval_text::chunk_embedding_text(&meta, None).len();
        let cand_tokens = est_tokens_from_chars(text_chars);

        if !batch.is_empty() {
            let new_max_tokens = batch_max_tokens.max(cand_tokens);
            // Recompute cost over the whole batch with the new max — adding a
            // long chunk raises max for every sequence, not just itself.
            let new_cost = batch_attention_cost(batch.len() + 1, new_max_tokens);
            let over_chunks = batch.len() >= max_chunks;
            let over_chars = batch_chars.saturating_add(text_chars) > max_chars;
            let over_attention = new_cost > attention_budget;
            if over_chunks || over_chars || over_attention {
                batches.push(std::mem::take(&mut batch));
                batch_chars = 0;
                batch_max_tokens = 0;
            }
        }

        batch_chars = batch_chars.saturating_add(text_chars);
        batch_max_tokens = batch_max_tokens.max(cand_tokens);
        batch.push(chunk);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

/// Adaptive file-batch sizing based on free RAM.
/// Mirrors Python's `adaptive_batch_files`.
fn adaptive_batch_files(base: usize, current: usize, free_gb: f64, min_free_gb: f64) -> usize {
    if min_free_gb <= 0.0 {
        return current.max(1);
    }
    let lo = (base / 4).max(2);
    let hi = (base * 4).max(base);
    if free_gb >= 4.0 * min_free_gb {
        return ((current.max(1) * 2).min(hi)).max(1);
    }
    if free_gb < 2.0 * min_free_gb {
        return ((current.max(1) / 2).max(lo)).max(1);
    }
    current.max(1)
}

/// Effective chunk batch size (chunks per single `embed` call).
fn effective_chunk_batch_size(batch_chunks: Option<usize>) -> usize {
    if let Some(bc) = batch_chunks {
        return bc.max(1);
    }
    if let Ok(val) = std::env::var("ORACLE_CHUNK_BATCH_CHUNKS") {
        if let Ok(n) = val.trim().parse::<usize>() {
            return n.max(1);
        }
    }
    // Python: max(CHUNK_BATCH_CHUNKS, 4 * effective_embed_batch_size()).
    // Without hardware probing, use 32 (Python's MPS default: 4 × 8).
    32
}

/// Ensure output store paths are never collected.
fn is_output_path(path: &Path, output_paths: &HashSet<PathBuf>) -> bool {
    output_paths.contains(&path.to_path_buf())
}

fn output_paths_set(
    sqlite_path: &Path,
    vector_path: &Path,
    manifest_path: &Path,
) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    if let Ok(p) = sqlite_path.canonicalize() {
        set.insert(p);
    }
    if let Ok(p) = vector_path.canonicalize() {
        set.insert(p);
    }
    if let Ok(p) = manifest_path.to_path_buf().canonicalize() {
        set.insert(p);
    }
    // Also add the non-canonicalized versions as fallback
    set.insert(sqlite_path.to_path_buf());
    set.insert(vector_path.to_path_buf());
    set.insert(manifest_path.to_path_buf());
    set
}

// ═══════════════════════════════════════════════════════════════════════════
// System probes — RAM and GPU
// ═══════════════════════════════════════════════════════════════════════════

/// Free system RAM in GB (mirrors Python's `free_memory_gb`).
///
/// Prefers `available_memory`, then `free_memory`, then `total - used` when
/// total > used. Never treats a single metric failure as 0.0 free (that always
/// trips the RAM floor). Returns 0.0 only when no usable metric is available.
/// Rounded to 2 decimal places.
pub fn free_memory_gb() -> f64 {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    let mut bytes = sys.available_memory();
    if bytes == 0 {
        bytes = sys.free_memory();
    }
    if bytes == 0 {
        let total = sys.total_memory();
        let used = sys.used_memory();
        if total > used {
            bytes = total - used;
        }
    }
    let gb = (bytes as f64) / (1024.0_f64.powi(3));
    (gb * 100.0).round() / 100.0
}

/// True when the memory floor should hard-pause indexing.
///
/// **Fail closed**: a low or unusable reading raises the guard, never lowers it.
/// NaN / negative / non-finite free RAM means "cannot prove it is safe" → pause.
/// (Incident 2026-07-25: the old plausible-reading escape hatch disabled the
/// floor exactly when free fell below ~1 GB on a 64 GB Mac, then indexing resumed
/// into a Jetsam freeze.)
fn should_enforce_memory_floor(free_gb: f64, min_free_gb: f64) -> bool {
    if min_free_gb <= 0.0 {
        return false;
    }
    if !free_gb.is_finite() || free_gb < 0.0 {
        return true;
    }
    free_gb < min_free_gb
}

/// Default minimum `kern.memorystatus_level` (kernel available-memory %) on macOS.
/// Below this → pause. Conservative starting point (idle machines sit near 90+).
const DEFAULT_MACOS_MIN_MEMORYSTATUS_LEVEL: u32 = 25;

/// Pure decision: should macOS memory-pressure signals force a pause?
///
/// - `pressure`: `kern.memorystatus_vm_pressure_level` (1=normal, 2=warn, 4=critical)
/// - `level`: `kern.memorystatus_level` (kernel available-memory percentage)
/// - `min_level`: threshold for `level` (pause when `level < min_level`)
///
/// `None` inputs mean "probe unavailable" — they do **not** force a pause
/// (the GB floor covers that path separately). Failed probes must never *unlock*
/// indexing; they also must not invent a pause on their own.
pub fn macos_pressure_says_pause(
    pressure: Option<u32>,
    level: Option<u32>,
    min_level: u32,
) -> bool {
    if let Some(p) = pressure {
        // 1 = normal; anything above is warn/critical (or unknown elevated).
        if p > 1 {
            return true;
        }
    }
    if let Some(l) = level {
        if l < min_level {
            return true;
        }
    }
    false
}

/// Why the combined memory guard wants a pause (for diagnosable logs).
#[derive(Debug, Clone, PartialEq)]
enum MemoryPauseReason {
    FreeGbFloor { free_gb: f64, min_free_gb: f64 },
    MacosVmPressure { pressure: u32 },
    MacosMemorystatusLevel { level: u32, min_level: u32 },
}

impl MemoryPauseReason {
    fn log_label(&self) -> String {
        match self {
            Self::FreeGbFloor {
                free_gb,
                min_free_gb,
            } => format!("signal=free_gb_floor free_gb={free_gb} min_free_gb={min_free_gb}"),
            Self::MacosVmPressure { pressure } => {
                format!("signal=macos_vm_pressure pressure_level={pressure}")
            }
            Self::MacosMemorystatusLevel { level, min_level } => {
                format!(
                    "signal=macos_memorystatus_level memorystatus_level={level} min_level={min_level}"
                )
            }
        }
    }
}

/// Resolve `ORACLE_MACOS_MIN_MEMORYSTATUS_LEVEL` (default 25).
fn resolve_macos_min_memorystatus_level() -> u32 {
    std::env::var("ORACLE_MACOS_MIN_MEMORYSTATUS_LEVEL")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MACOS_MIN_MEMORYSTATUS_LEVEL)
}

/// `ORACLE_DISABLE_MACOS_MEMORY_PRESSURE=1` (or true/yes) skips the sysctl probe
/// for debugging. Default: probe enabled on macOS.
fn macos_memory_pressure_probe_enabled() -> bool {
    match std::env::var("ORACLE_DISABLE_MACOS_MEMORY_PRESSURE") {
        Ok(v) => {
            let t = v.trim();
            !(t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes"))
        }
        Err(_) => true,
    }
}

/// Read macOS kernel memory-pressure sysctls. Fail-closed on errors → `None`.
///
/// Uses `kern.memorystatus_vm_pressure_level` and `kern.memorystatus_level` —
/// the same signals jetsam uses, so Metal wired buffers cannot fool them the
/// way they fool `sysinfo::available_memory()`.
#[cfg(target_os = "macos")]
fn read_macos_memory_pressure() -> Option<(u32, u32)> {
    fn sysctl_u32(name: &str) -> Option<u32> {
        let cname = std::ffi::CString::new(name).ok()?;
        let mut val: u32 = 0;
        let mut len = std::mem::size_of::<u32>();
        let rc = unsafe {
            libc::sysctlbyname(
                cname.as_ptr(),
                &mut val as *mut u32 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 && len == std::mem::size_of::<u32>() {
            Some(val)
        } else {
            None
        }
    }
    let pressure = sysctl_u32("kern.memorystatus_vm_pressure_level")?;
    let level = sysctl_u32("kern.memorystatus_level")?;
    Some((pressure, level))
}

#[cfg(not(target_os = "macos"))]
fn read_macos_memory_pressure() -> Option<(u32, u32)> {
    None
}

/// Combined memory guard: GB floor **and** (on macOS) kernel pressure.
/// Always takes the more conservative outcome. A failed macOS probe returns
/// no pressure-based pause (GB floor still applies) — never unlocks indexing.
///
/// `min_free_gb <= 0` disables the entire memory guard (GB + pressure), matching
/// the existing master switch for the free-RAM floor.
fn memory_pause_reason(free_gb: f64, min_free_gb: f64) -> Option<MemoryPauseReason> {
    if min_free_gb <= 0.0 {
        return None;
    }
    if should_enforce_memory_floor(free_gb, min_free_gb) {
        return Some(MemoryPauseReason::FreeGbFloor {
            free_gb,
            min_free_gb,
        });
    }
    if !macos_memory_pressure_probe_enabled() {
        return None;
    }
    match read_macos_memory_pressure() {
        Some((pressure, level)) => {
            let min_level = resolve_macos_min_memorystatus_level();
            if !macos_pressure_says_pause(Some(pressure), Some(level), min_level) {
                return None;
            }
            // Prefer naming the pressure signal when elevated; otherwise level.
            if pressure > 1 {
                Some(MemoryPauseReason::MacosVmPressure { pressure })
            } else {
                Some(MemoryPauseReason::MacosMemorystatusLevel { level, min_level })
            }
        }
        // Probe failed: do not invent a pause; GB floor already checked above.
        None => None,
    }
}

/// GPU temperature in °C via `nvidia-smi`.
///
/// Returns `None` when `nvidia-smi` is absent or errors. On macOS / Apple Silicon
/// this is **always** `None` — there is no thermal guard on those hosts (we do
/// not call powermetrics; it needs sudo). Memory pressure is the binding limit.
pub fn gpu_temperature_c() -> Option<i32> {
    use std::process::Command;
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.trim().lines().next()?;
    first_line.trim().parse::<f32>().ok().map(|v| v as i32)
}

/// Sleep-and-retry while free RAM is below `min_free_gb`.
/// Returns the final observed free-RAM reading (never faked above the floor).
///
/// Exits only when free RAM genuinely recovers above `min_free_gb`, or when the
/// bounded retry count is exhausted. Callers must re-check
/// [`memory_pause_reason`] after return (GB floor **and** macOS pressure) and
/// treat a still-blocking result as [`IndexStatus::PausedLowMemory`].
fn wait_for_memory_recovery(min_free_gb: f64, progress: Option<&dyn Fn(&str)>) -> f64 {
    wait_for_memory_recovery_with(
        min_free_gb,
        progress,
        free_memory_gb,
        LOW_MEMORY_RETRY_SECONDS,
        LOW_MEMORY_RETRY_CYCLES,
    )
}

/// Testable core of [`wait_for_memory_recovery`]: inject free-RAM reader and
/// retry timing (use `retry_seconds = 0` in unit tests to avoid sleeping).
///
/// This loop only clears the **GB floor**. Callers always re-check the combined
/// guard ([`memory_pause_reason`]) after return so macOS pressure still pauses
/// even when free-GB looks healthy.
fn wait_for_memory_recovery_with<F>(
    min_free_gb: f64,
    progress: Option<&dyn Fn(&str)>,
    mut free_reader: F,
    retry_seconds: u64,
    retry_cycles: usize,
) -> f64
where
    F: FnMut() -> f64,
{
    let mut free_gb = free_reader();
    if free_gb.is_finite() && free_gb >= min_free_gb {
        return free_gb;
    }
    for cycle in 0..retry_cycles {
        log_progress(
            progress,
            &format!(
                "chunk-index low-memory retry free_gb={free_gb} min_free_gb={min_free_gb} \
                 sleep_seconds={retry_seconds} cycle={}/{}",
                cycle + 1,
                retry_cycles,
            ),
        );
        if retry_seconds > 0 {
            std::thread::sleep(Duration::from_secs(retry_seconds));
        }
        free_gb = free_reader();
        if free_gb.is_finite() && free_gb >= min_free_gb {
            return free_gb;
        }
    }
    // Exhausted: return the last real reading. Caller pauses with PausedLowMemory.
    free_gb
}

/// Wait for GPU cooldown. Returns the final observed temperature.
fn wait_for_gpu_cooldown(max_gpu_temp_c: i32, progress: Option<&dyn Fn(&str)>) -> Option<i32> {
    let resume_temp_c = GPU_RESUME_TEMP_C.min(max_gpu_temp_c - 1);
    let mut temp_c = gpu_temperature_c();
    for cycle in 0..GPU_COOLDOWN_MAX_CYCLES {
        if temp_c.is_none() || temp_c.unwrap() <= resume_temp_c {
            return temp_c;
        }
        log_progress(
            progress,
            &format!(
                "chunk-index gpu cooldown temp_c={} resume_temp_c={resume_temp_c} \
                 sleep_seconds={GPU_COOLDOWN_SECONDS} cycle={}/{}",
                temp_c.unwrap(),
                cycle + 1,
                GPU_COOLDOWN_MAX_CYCLES,
            ),
        );
        std::thread::sleep(Duration::from_secs(GPU_COOLDOWN_SECONDS));
        temp_c = gpu_temperature_c();
    }
    temp_c
}

/// Emit a progress message to the callback (if any) and always to stderr so
/// app logs show indexing phases even when the UI does not map a given line.
fn log_progress(progress: Option<&dyn Fn(&str)>, message: &str) {
    eprintln!("[oracle-index] {message}");
    if let Some(cb) = progress {
        cb(message);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// sync_text_chunks — SQLite-only text resync (no vectors)
// ═══════════════════════════════════════════════════════════════════════════

/// Rebuild text chunks in SQLite for files that need it, without touching
/// the vector store.  Mirrors Python's `sync_text_chunks`.
///
/// Returns a summary dict matching the Python shape:
/// `{ status, root, files, skipped, chunks, sqlite_path }`.
pub fn sync_text_chunks(
    root: &Path,
    sqlite: &SqliteStore,
    manifest_path: &Path,
    batch_files: usize,
    force: bool,
    progress: Option<&dyn Fn(&str)>,
) -> Result<SyncResult> {
    let root = root.to_path_buf();
    let files = collect::collect_text_files(&root);
    let mut manifest = load_manifest(manifest_path);
    let manifest_files_owned = manifest_files_for_root(&mut manifest, &root, false)
        .cloned()
        .unwrap_or_default();

    let total_files = files.len();
    let pending: Vec<PathBuf> = if force {
        files
    } else {
        files
            .iter()
            .filter(|path| {
                manifest::text_chunks_up_to_date(path, &root, &manifest_files_owned, sqlite)
                    .map(|up| !up)
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    };

    let skipped_files = total_files - pending.len();
    let mut processed_files = 0usize;
    let mut processed_chunks = 0usize;
    let batch_size = batch_files.max(1);

    for start in (0..pending.len()).step_by(batch_size) {
        let batch = &pending[start..(start + batch_size).min(pending.len())];
        let mut file_ids = Vec::new();
        let mut all_chunks: Vec<serde_json::Value> = Vec::new();

        for path in batch {
            let file_id = relative_posix(path, &root)?;
            let mtime = utc_mtime_str(path);
            let mut file_chunks = chunking::build_chunks_for_file(path, &root);
            enrich_chunks(&mut file_chunks, &mtime, &file_id);
            file_ids.push(file_id);
            all_chunks.extend(file_chunks);
        }

        let file_chunks_refs: Vec<FileChunk> =
            all_chunks.iter().map(chunk_value_to_file_chunk).collect();
        sqlite.replace_chunks_for_files(&file_ids, &file_chunks_refs)?;

        processed_files += batch.len();
        processed_chunks += all_chunks.len();
        log_progress(
            progress,
            &format!(
                "chunk-text-sync committed files={processed_files}/{} \
                 chunks={processed_chunks} skipped={skipped_files}",
                pending.len(),
            ),
        );
    }

    Ok(SyncResult {
        status: "complete".to_string(),
        root: root.to_string_lossy().to_string(),
        files: processed_files,
        skipped: skipped_files,
        chunks: processed_chunks,
        sqlite_path: sqlite.path().to_string_lossy().to_string(),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// prune_excluded_chunks — remove stale chunks/vectors/manifest entries
// ═══════════════════════════════════════════════════════════════════════════

/// Remove chunks, vectors, and manifest entries for files no longer collected,
/// plus orphaned vector IDs not backed by SQLite chunks.
///
/// Mirrors Python's `prune_excluded_chunks`.
///
/// `node_vectors` is the node-card vector store (optional — when `None` the
/// node-vector pruning step is skipped).
pub async fn prune_excluded_chunks(
    root: &Path,
    sqlite: &SqliteStore,
    chunk_vectors: &LanceStore,
    manifest_path: &Path,
    node_vectors: Option<&LanceStore>,
    progress: Option<&dyn Fn(&str)>,
) -> Result<PruneResult> {
    let root = root.to_path_buf();
    let expected: BTreeSet<String> = collect::collect_text_files(&root)
        .iter()
        .map(|p| relative_posix(p, &root))
        .collect::<Result<_>>()?;

    // Gather expected files from other roots too (for node-card pruning).
    let mut expected_all_roots = expected.clone();
    let mut manifest = load_manifest(manifest_path);
    {
        let roots = manifest::manifest_roots(&mut manifest);
        for (root_key, _entry) in roots.iter() {
            if root_key == &root.to_string_lossy().to_string() {
                continue;
            }
            let other_root = Path::new(root_key);
            if other_root.is_dir() {
                for path in collect::collect_text_files(other_root) {
                    if let Ok(rel) = relative_posix(&path, other_root) {
                        expected_all_roots.insert(rel);
                    }
                }
            }
        }
    }

    // ── Chunk vectors ───────────────────────────────────────────────────
    let all_chunks = sqlite.all_chunks()?;
    let existing_file_ids: BTreeSet<String> =
        all_chunks.iter().map(|c| c.file_id.clone()).collect();
    let removed_files: Vec<String> = existing_file_ids
        .difference(&expected_all_roots)
        .cloned()
        .collect();
    let removed_ids = if !removed_files.is_empty() {
        sqlite.chunk_ids_for_files(&removed_files)?
    } else {
        Vec::new()
    };
    if !removed_files.is_empty() {
        sqlite.replace_chunks_for_files(&removed_files, &[])?;
    }

    let valid_chunk_ids: BTreeSet<String> =
        sqlite.all_chunks()?.iter().map(|c| c.id.clone()).collect();
    let all_vector_rows = chunk_vectors.read_all().await?;
    let vector_ids: BTreeSet<String> = all_vector_rows.iter().map(|r| r.id.clone()).collect();
    let orphan_ids: Vec<String> = vector_ids.difference(&valid_chunk_ids).cloned().collect();
    let removed_vector_ids: Vec<String> = removed_ids
        .into_iter()
        .chain(orphan_ids.iter().cloned())
        .collect();
    let removed_vector_count = removed_vector_ids.len();
    let orphan_count = orphan_ids.len();
    if !removed_vector_ids.is_empty() {
        chunk_vectors.replace_ids(&removed_vector_ids, &[]).await?;
    }

    // ── Node cards ──────────────────────────────────────────────────────
    let all_nodes = sqlite.all_nodes()?;
    let removed_node_ids: Vec<String> = all_nodes
        .iter()
        .filter(|n| !expected_all_roots.contains(&n.file_sorgente))
        .map(|n| n.id.clone())
        .collect();
    let removed_node_count = removed_node_ids.len();
    if !removed_node_ids.is_empty() {
        sqlite.delete_nodes(&removed_node_ids)?;
    }

    // ── Node vector store ───────────────────────────────────────────────
    let mut removed_node_vector_count = 0usize;
    let mut removed_orphan_node_vector_count = 0usize;
    if let Some(nv) = node_vectors {
        let valid_node_ids: BTreeSet<String> =
            sqlite.all_nodes()?.iter().map(|n| n.id.clone()).collect();
        let all_node_vectors = nv.read_all().await?;
        let node_vector_ids: BTreeSet<String> =
            all_node_vectors.iter().map(|r| r.id.clone()).collect();
        let orphan_node_vector_ids: Vec<String> = node_vector_ids
            .difference(&valid_node_ids)
            .cloned()
            .collect();
        removed_orphan_node_vector_count = orphan_node_vector_ids.len();
        let removed_nv_ids: Vec<String> = removed_node_ids
            .iter()
            .chain(orphan_node_vector_ids.iter())
            .cloned()
            .collect();
        removed_node_vector_count = removed_nv_ids.len();
        if !removed_nv_ids.is_empty() {
            nv.replace_ids(&removed_nv_ids, &[]).await?;
        }
    }

    // ── Manifest ────────────────────────────────────────────────────────
    let mut manifest_removed = 0usize;
    {
        let manifest_files = manifest_files_for_root(&mut manifest, &root, false);
        if let Some(mf) = manifest_files {
            let keys: Vec<String> = mf.keys().cloned().collect();
            for file_id in keys {
                if !expected.contains(&file_id) {
                    mf.remove(&file_id);
                    manifest_removed += 1;
                }
            }
        }
    }
    sync_legacy_manifest_root(&mut manifest, &root);
    save_manifest(manifest_path, &manifest)?;

    let chunk_file_count = sqlite.chunk_file_count()?;
    let chunk_count = sqlite.chunk_count()?;
    let vector_count = chunk_vectors.count().await?;
    let node_count = sqlite.count()?;
    let node_vector_count = if let Some(nv) = node_vectors {
        nv.count().await?
    } else {
        0
    };

    log_progress(
        progress,
        &format!(
            "chunk-prune removed_files={} removed_vectors={} orphan_vectors={} \
             removed_nodes={} removed_node_vectors={} manifest_removed={}",
            removed_files.len(),
            removed_vector_count,
            orphan_count,
            removed_node_count,
            removed_node_vector_count,
            manifest_removed,
        ),
    );

    Ok(PruneResult {
        status: "complete".to_string(),
        root: root.to_string_lossy().to_string(),
        removed_files: removed_files.len(),
        removed_vectors: removed_vector_count,
        removed_orphan_vectors: orphan_count,
        removed_nodes: removed_node_count,
        removed_node_vectors: removed_node_vector_count,
        removed_orphan_node_vectors: removed_orphan_node_vector_count,
        manifest_removed,
        sqlite_chunk_files: chunk_file_count,
        sqlite_chunks: chunk_count,
        vector_records: vector_count,
        sqlite_nodes: node_count,
        node_vector_records: node_vector_count,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// index_file_chunks — the main embed+write pipeline
// ═══════════════════════════════════════════════════════════════════════════

/// Embed and index file chunks into SQLite + LanceDB + manifest.
///
/// Mirrors Python's `index_file_chunks`.  The embedder's `embed` method is
/// **synchronous** — callers running inside an async context should wrap this
/// function in `tokio::task::spawn_blocking`.
///
/// When the `CancelFlag` fires, the function returns a partial-progress result
/// (the same shape as `complete`) with `status = Complete` — the caller can
/// check `cancel.is_cancelled()` to distinguish a clean cancellation from a
/// full run.  (This matches Python's behavior: a cancelled run commits
/// everything processed so far.)
pub async fn index_file_chunks(
    root: &Path,
    sqlite: &SqliteStore,
    chunk_vectors: &LanceStore,
    manifest_path: &Path,
    embedder: &dyn TextEmbedder,
    cancel: &CancelFlag,
    config: &IndexerConfig,
    progress: Option<&dyn Fn(&str)>,
) -> Result<IndexResult> {
    let root = root.to_path_buf();
    let manifest_path = manifest_path.to_path_buf();
    let mut manifest = load_manifest(&manifest_path);
    {
        manifest_files_for_root(&mut manifest, &root, true);
    }

    let vector_path = chunk_vectors.path().to_path_buf();
    let sqlite_path = sqlite.path().to_path_buf();

    // ── Pre-scan RAM guard (fail-closed) ────────────────────────────────
    // Wait once for recovery; if still blocked (GB floor or macOS pressure),
    // hard-pause. Starting under jetsam pressure is what froze the host.
    if config.min_free_gb > 0.0 {
        let free_gb = free_memory_gb();
        if memory_pause_reason(free_gb, config.min_free_gb).is_some() {
            let recovered = wait_for_memory_recovery(config.min_free_gb, progress);
            if let Some(reason) = memory_pause_reason(recovered, config.min_free_gb) {
                log_progress(
                    progress,
                    &format!(
                        "chunk-index paused_low_memory free_gb={recovered} {}",
                        reason.log_label()
                    ),
                );
                return Ok(make_index_result(
                    IndexStatus::PausedLowMemory,
                    &root,
                    &sqlite_path,
                    &vector_path,
                    &manifest_path,
                    0,
                    None,
                    0,
                    0,
                    None,
                    None,
                    None,
                    recovered,
                    None,
                    None,
                ));
            }
        }
    }

    let out_paths = output_paths_set(&sqlite_path, &vector_path, &manifest_path);
    let files: Vec<PathBuf> = collect::collect_text_files(&root)
        .into_iter()
        .filter(|p| !is_output_path(p, &out_paths))
        .collect();

    // file_needs_index is vector-unaware (manifest+sqlite text only). When Lance
    // is empty (failed/paused dense run, wiped chunks.lancedb) every file would
    // otherwise be skipped forever without Force. Re-embed the whole collect set.
    let vector_count = chunk_vectors.count().await.unwrap_or(0);
    let pending: Vec<PathBuf> = {
        let manifest_files = manifest_files_for_root(&mut manifest, &root, true).unwrap();
        files
            .iter()
            .filter(|path| {
                config.force
                    || vector_count == 0
                    || manifest::file_needs_index(path, &root, manifest_files, sqlite)
                        .unwrap_or(true)
            })
            .cloned()
            .collect()
    };
    if vector_count == 0 && !config.force {
        log_progress(
            progress,
            &format!(
                "chunk-index re-embed-all reason=no_vectors scanned={}",
                files.len()
            ),
        );
    }

    log_progress(
        progress,
        &format!(
            "chunk-index start root={} scanned={} pending={} indexed={} min_free_ram_gb={}",
            root.display(),
            files.len(),
            pending.len(),
            {
                let mf = manifest_files_for_root(&mut manifest, &root, false);
                mf.map(|m| m.len()).unwrap_or(0)
            },
            config.min_free_gb,
        ),
    );

    let mut processed_files = 0usize;
    let mut processed_chunks = 0usize;
    let mut files_done_this_run = 0usize;
    let base_file_batch_size = config.batch_files.max(1);
    let mut file_batch_size = base_file_batch_size;
    let max_files_per_run = config.max_batches.map(|mb| mb * base_file_batch_size);
    let chunk_batch_size = effective_chunk_batch_size(config.batch_chunks);
    let chunk_char_budget = config.batch_chars.max(1);

    let mut pending_index = 0usize;
    let mut batch_index = 0usize;

    while pending_index < pending.len() {
        if cancel.is_cancelled() {
            break;
        }

        if let Some(max_fpr) = max_files_per_run {
            if files_done_this_run >= max_fpr {
                break;
            }
        }

        let free_gb = free_memory_gb();
        file_batch_size = adaptive_batch_files(
            base_file_batch_size,
            file_batch_size,
            free_gb,
            config.min_free_gb,
        );

        let remaining_files = match max_files_per_run {
            Some(max_fpr) => file_batch_size.min(max_fpr - files_done_this_run),
            None => file_batch_size,
        };
        let batch_paths =
            &pending[pending_index..(pending_index + remaining_files).min(pending.len())];
        if batch_paths.is_empty() {
            break;
        }

        // ── Pre-batch RAM guard (wait-and-resume) ───────────────────────
        // Fail-closed: low free-RAM or elevated macOS kernel pressure → pause.
        if memory_pause_reason(free_gb, config.min_free_gb).is_some() {
            {
                let mf = manifest_files_for_root(&mut manifest, &root, true).unwrap();
                // Touch to ensure legacy mirror is up to date before save
                let _ = mf;
            }
            sync_legacy_manifest_root(&mut manifest, &root);
            save_manifest(&manifest_path, &manifest)?;
            let recovered = wait_for_memory_recovery(config.min_free_gb, progress);
            if let Some(reason) = memory_pause_reason(recovered, config.min_free_gb) {
                log_progress(
                    progress,
                    &format!(
                        "chunk-index paused_low_memory free_gb={recovered} {}",
                        reason.log_label()
                    ),
                );
                return Ok(make_index_result(
                    IndexStatus::PausedLowMemory,
                    &root,
                    &sqlite_path,
                    &vector_path,
                    &manifest_path,
                    files.len(),
                    Some(pending.len().saturating_sub(processed_files)),
                    processed_files,
                    processed_chunks,
                    None,
                    None,
                    None,
                    recovered,
                    None,
                    None,
                ));
            }
        }

        // ── Build chunks for batch ──────────────────────────────────────
        let batch_file_ids: Vec<String> = batch_paths
            .iter()
            .map(|p| relative_posix(p, &root))
            .collect::<Result<_>>()?;

        let mut file_chunks_map: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
        let mut batch_chunks_all: Vec<serde_json::Value> = Vec::new();
        for path in batch_paths {
            let file_id = relative_posix(path, &root)?;
            let mtime = utc_mtime_str(path);
            let mut file_chunks = chunking::build_chunks_for_file(path, &root);
            enrich_chunks(&mut file_chunks, &mtime, &file_id);
            batch_chunks_all.extend(file_chunks.iter().cloned());
            file_chunks_map.insert(file_id, file_chunks);
        }

        let old_ids = sqlite.chunk_ids_for_files(&batch_file_ids)?;
        batch_index += 1;
        log_progress(
            progress,
            &format!(
                "chunk-index batch begin batch_index={} files={} chunks={} remaining_before={} free_gb={} first_file={}",
                batch_index,
                batch_paths.len(),
                batch_chunks_all.len(),
                pending.len().saturating_sub(processed_files),
                free_memory_gb(),
                batch_file_ids.first().map(String::as_str).unwrap_or(""),
            ),
        );

        // ── Embed + build vector records ────────────────────────────────
        let mut vector_records: Vec<LanceRow> = Vec::new();
        let sub_batches = chunk_batches(
            &batch_chunks_all,
            chunk_batch_size,
            chunk_char_budget,
            config.attention_budget.max(1),
        );
        let mut batch_embedded = 0usize;

        for sub_batch in &sub_batches {
            if cancel.is_cancelled() {
                break;
            }

            // GPU thermal guard
            if let Some(max_temp) = config.max_gpu_temp_c {
                if let Some(temp) = gpu_temperature_c() {
                    if temp >= max_temp {
                        sync_legacy_manifest_root(&mut manifest, &root);
                        save_manifest(&manifest_path, &manifest)?;
                        let cooled = wait_for_gpu_cooldown(max_temp, progress);
                        if let Some(t) = cooled {
                            if t >= max_temp {
                                log_progress(
                                    progress,
                                    &format!(
                                        "chunk-index paused_gpu_temperature temp_c={t} max_gpu_temp_c={max_temp}"
                                    ),
                                );
                                return Ok(make_index_result(
                                    IndexStatus::PausedGpuTemperature,
                                    &root,
                                    &sqlite_path,
                                    &vector_path,
                                    &manifest_path,
                                    files.len(),
                                    Some(pending.len().saturating_sub(processed_files)),
                                    processed_files,
                                    processed_chunks,
                                    None,
                                    None,
                                    None,
                                    free_memory_gb(),
                                    Some(t),
                                    Some(max_temp),
                                ));
                            }
                        }
                        log_progress(
                            progress,
                            &format!(
                                "chunk-index gpu cooled resume temp_c={:?} max_gpu_temp_c={max_temp}",
                                cooled
                            ),
                        );
                    }
                }
            }

            // In-sub-batch RAM guard (fail-closed hard pause)
            let free_gb = free_memory_gb();
            if memory_pause_reason(free_gb, config.min_free_gb).is_some() {
                sync_legacy_manifest_root(&mut manifest, &root);
                save_manifest(&manifest_path, &manifest)?;
                let recovered = wait_for_memory_recovery(config.min_free_gb, progress);
                if let Some(reason) = memory_pause_reason(recovered, config.min_free_gb) {
                    log_progress(
                        progress,
                        &format!(
                            "chunk-index paused_low_memory before_embed batch_files={} free_gb={recovered} {}",
                            batch_paths.len(),
                            reason.log_label()
                        ),
                    );
                    return Ok(make_index_result(
                        IndexStatus::PausedLowMemory,
                        &root,
                        &sqlite_path,
                        &vector_path,
                        &manifest_path,
                        files.len(),
                        Some(pending.len().saturating_sub(processed_files)),
                        processed_files,
                        processed_chunks,
                        None,
                        None,
                        None,
                        recovered,
                        None,
                        None,
                    ));
                }
            }

            // Compute embedding texts
            let texts: Vec<String> = sub_batch
                .iter()
                .map(|c| {
                    let meta = chunk_value_to_meta(c);
                    retrieval_text::chunk_embedding_text(&meta, None)
                })
                .collect();
            let total_chars: usize = texts.iter().map(|t| t.len()).sum();
            log_progress(
                progress,
                &format!(
                    "chunk-index embed begin files={} chunks={} chars={total_chars}",
                    batch_paths.len(),
                    texts.len(),
                ),
            );

            // Embed (sync call — see module docs). O0 measures this trait call
            // without changing batching or the cancellation contract; ONNX
            // session.run remains an internal detail of the concrete backend.
            let embed_started = Instant::now();
            let vectors = match embedder.embed(&texts, chunk_batch_size, cancel) {
                Ok(vectors) => {
                    log_progress(
                        progress,
                        &format!(
                            "chunk-index embed end chunks={} duration_ms={}",
                            texts.len(),
                            embed_started.elapsed().as_millis(),
                        ),
                    );
                    vectors
                }
                Err(error) => {
                    log_progress(
                        progress,
                        &format!(
                            "chunk-index embed error chunks={} duration_ms={}",
                            texts.len(),
                            embed_started.elapsed().as_millis(),
                        ),
                    );
                    return Err(error);
                }
            };
            if cancel.is_cancelled() {
                // Cancellation fired during this embed: DROP the batch. A
                // partial commit would write sqlite chunks for every file in
                // the batch but vectors for only some sub-batches — the
                // stores must never diverge. The whole batch re-runs on the
                // next invocation (manifest was not updated).
                vector_records.clear();
                break;
            }

            // Build vector records
            for (chunk, vector) in sub_batch.iter().zip(vectors) {
                vector_records.push(chunk_value_to_lance_row(chunk, vector));
            }

            batch_embedded += sub_batch.len();
            log_progress(
                progress,
                &format!(
                    "chunk-index progress embedded_chunks={}",
                    processed_chunks + batch_embedded
                ),
            );
        }

        // ── Commit batch to stores ──────────────────────────────────────
        if cancel.is_cancelled() {
            // Batch was abandoned mid-embed: nothing of it is committed.
            break;
        }
        let committed_file_count = batch_paths.len();
        let committed_chunk_count = batch_chunks_all.len();

        chunk_vectors.replace_ids(&old_ids, &vector_records).await?;

        let file_chunks_for_sqlite: Vec<FileChunk> = batch_chunks_all
            .iter()
            .map(chunk_value_to_file_chunk)
            .collect();
        sqlite.replace_chunks_for_files(&batch_file_ids, &file_chunks_for_sqlite)?;

        // Update manifest entries
        {
            let manifest_files = manifest_files_for_root(&mut manifest, &root, true).unwrap();
            for (path, file_id) in batch_paths.iter().zip(&batch_file_ids) {
                let chunk_count = file_chunks_map
                    .get(file_id)
                    .map(|c| c.len() as u64)
                    .unwrap_or(0);
                let sig = file_signature(path, Some(chunk_count))?;
                manifest_files.insert(file_id.clone(), sig);
            }
        }
        sync_legacy_manifest_root(&mut manifest, &root);
        save_manifest(&manifest_path, &manifest)?;

        processed_files += committed_file_count;
        processed_chunks += committed_chunk_count;
        files_done_this_run += committed_file_count;
        pending_index += committed_file_count;

        log_progress(
            progress,
            &format!(
                "chunk-index batch committed processed_files={processed_files} \
                 processed_chunks={processed_chunks} indexed_files={}",
                {
                    let mf = manifest_files_for_root(&mut manifest, &root, false);
                    mf.map(|m| m.len()).unwrap_or(0)
                }
            ),
        );
    }

    // ── Final status ────────────────────────────────────────────────────
    let status = if processed_files == pending.len() {
        IndexStatus::Complete
    } else {
        IndexStatus::PausedBatchLimit
    };

    sync_legacy_manifest_root(&mut manifest, &root);
    save_manifest(&manifest_path, &manifest)?;

    let total_files = sqlite.chunk_file_count()?;
    let total_chunks = sqlite.chunk_count()?;
    let vector_records = chunk_vectors.count().await?;
    let free_gb = free_memory_gb();

    log_progress(
        progress,
        &format!(
            "chunk-index {:?} processed_files={processed_files} processed_chunks={processed_chunks}",
            status,
        ),
    );

    Ok(make_index_result(
        status,
        &root,
        &sqlite_path,
        &vector_path,
        &manifest_path,
        files.len(),
        Some(pending.len().saturating_sub(processed_files)),
        processed_files,
        processed_chunks,
        Some(total_files),
        Some(total_chunks),
        Some(vector_records),
        free_gb,
        None,
        None,
    ))
}

// ═══════════════════════════════════════════════════════════════════════════
// chunk_index_status — read-only status snapshot
// ═══════════════════════════════════════════════════════════════════════════

/// Return a status snapshot for the chunk index.  Mirrors Python's
/// `chunk_index_status`.  Field names are snake_case (camelCase conversion
/// happens at the HTTP layer).
pub async fn chunk_index_status(
    root: &Path,
    sqlite: &SqliteStore,
    chunk_vectors: &LanceStore,
    manifest_path: &Path,
) -> Result<IndexStatusSnapshot> {
    let root = root.to_path_buf();
    let mut manifest = load_manifest(manifest_path);
    let manifest_files_owned = manifest_files_for_root(&mut manifest, &root, false)
        .cloned()
        .unwrap_or_default();

    let sqlite_path = sqlite.path().to_path_buf();
    let vector_path = chunk_vectors.path().to_path_buf();
    let out_paths = output_paths_set(&sqlite_path, &vector_path, manifest_path);

    let files: Vec<PathBuf> = collect::collect_text_files(&root)
        .into_iter()
        .filter(|p| !is_output_path(p, &out_paths))
        .collect();

    let expected: BTreeSet<String> = files
        .iter()
        .map(|p| relative_posix(p, &root))
        .collect::<Result<_>>()?;

    let indexed: BTreeSet<String> = manifest_files_owned.keys().cloned().collect();
    let mut pending: Vec<String> = expected.difference(&indexed).cloned().collect();
    pending.sort_by(|a, b| {
        let ra = collect::priority_rank(a);
        let rb = collect::priority_rank(b);
        ra.cmp(&rb).then_with(|| a.cmp(b))
    });

    let mut stale = Vec::new();
    for path in &files {
        let file_id = relative_posix(path, &root)?;
        if indexed.contains(&file_id)
            && manifest::file_needs_index(path, &root, &manifest_files_owned, sqlite)?
        {
            stale.push(file_id);
        }
    }

    let first_pending: Vec<String> = pending.iter().take(12).cloned().collect();
    let first_stale: Vec<String> = stale.iter().take(12).cloned().collect();

    Ok(IndexStatusSnapshot {
        root: root.to_string_lossy().to_string(),
        manifest_path: manifest_path.to_string_lossy().to_string(),
        expected_files: expected.len(),
        indexed_files: indexed.intersection(&expected).count(),
        pending_files: pending.len(),
        stale_files: stale.len(),
        sqlite_chunk_files: sqlite.chunk_file_count()?,
        sqlite_chunks: sqlite.chunk_count()?,
        vector_records: chunk_vectors.count().await?,
        chunk_profile: active_chunk_profile_version(None),
        first_pending,
        first_stale,
        free_gb: free_memory_gb(),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// IndexResult builder (mirrors Python's status_payload)
// ═══════════════════════════════════════════════════════════════════════════

#[allow(clippy::too_many_arguments)]
fn make_index_result(
    status: IndexStatus,
    root: &Path,
    sqlite_path: &Path,
    vector_path: &Path,
    manifest_path: &Path,
    scanned: usize,
    pending: Option<usize>,
    processed: usize,
    chunks: usize,
    total_files: Option<usize>,
    total_chunks: Option<usize>,
    vector_records: Option<usize>,
    free_gb: f64,
    gpu_temp_c: Option<i32>,
    max_gpu_temp_c: Option<i32>,
) -> IndexResult {
    IndexResult {
        status,
        root: root.to_string_lossy().to_string(),
        sqlite_path: sqlite_path.to_string_lossy().to_string(),
        vector_path: vector_path.to_string_lossy().to_string(),
        manifest_path: manifest_path.to_string_lossy().to_string(),
        scanned,
        pending,
        processed,
        chunks,
        total_files,
        total_chunks,
        vector_records,
        free_gb: Some(free_gb),
        free_ram_gb: Some(free_gb),
        gpu_temp_c,
        max_gpu_temp_c,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Env helpers
// ═══════════════════════════════════════════════════════════════════════════

fn env_or_usize(keys: &[&str], default: usize) -> usize {
    for key in keys {
        if let Ok(val) = std::env::var(key) {
            if let Ok(n) = val.trim().parse::<usize>() {
                return n;
            }
        }
    }
    default
}

fn env_or_f64(keys: &[&str], default: f64) -> f64 {
    for key in keys {
        if let Ok(val) = std::env::var(key) {
            if let Ok(n) = val.trim().parse::<f64>() {
                return n;
            }
        }
    }
    default
}

fn env_opt_usize(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
}

fn env_opt_i32(key: &str) -> Option<i32> {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok())
}

// ═══════════════════════════════════════════════════════════════════════════
// Unit tests — pure, no model / GPU / real index
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunk(id: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "file_id": "t.rs",
            "file_sorgente": "t.rs",
            "text": text,
            "kind": "text_slice",
            "symbol_name": "",
            "language": "rs",
            "line_start": 0,
            "line_end": 0,
            "symbols_used": "",
            "chunk_index": 0,
            "label": "",
            "area": "",
            "cluster_semantic": "",
            "start_char": 0,
            "end_char": text.len(),
            "ultima_modifica": "",
            "embedding_dims": 1024,
        })
    }

    fn emb_chars(chunk: &serde_json::Value) -> usize {
        let meta = chunk_value_to_meta(chunk);
        retrieval_text::chunk_embedding_text(&meta, None).len()
    }

    fn batch_cost(batch: &[&serde_json::Value]) -> usize {
        let max_tokens = batch
            .iter()
            .map(|c| est_tokens_from_chars(emb_chars(c)))
            .max()
            .unwrap_or(1);
        batch_attention_cost(batch.len(), max_tokens)
    }

    #[test]
    fn chunk_batches_long_chunk_alone_under_attention_budget() {
        // Many short chunks + ONE very long chunk must never produce a batch
        // whose len × max_tokens² exceeds the budget; the long one ends alone.
        let budget = DEFAULT_ATTENTION_BUDGET;
        let short = "fn short() {}".to_string();
        let long = "x".repeat(40_000);
        let mut owned: Vec<serde_json::Value> = (0..16)
            .map(|i| make_chunk(&format!("s{i}"), &short))
            .collect();
        owned.push(make_chunk("LONG", &long));
        for i in 16..24 {
            owned.push(make_chunk(&format!("s{i}"), &short));
        }

        let batches = chunk_batches(&owned, 32, 50_000, budget);

        for b in &batches {
            let cost = batch_cost(b);
            // A lone over-budget chunk is allowed (backends window + run alone);
            // multi-item batches must stay within budget.
            if b.len() > 1 {
                assert!(
                    cost <= budget,
                    "multi-item batch cost {cost} > budget {budget} (len={})",
                    b.len()
                );
            }
        }

        let long_batches: Vec<_> = batches
            .iter()
            .filter(|b| b.iter().any(|c| c.get("id").and_then(|v| v.as_str()) == Some("LONG")))
            .collect();
        assert_eq!(long_batches.len(), 1, "long chunk should appear in exactly one batch");
        assert_eq!(
            long_batches[0].len(),
            1,
            "long chunk must be alone (would pad every peer to its length)"
        );
    }

    #[test]
    fn chunk_batches_respects_max_chunks() {
        let owned: Vec<_> = (0..10)
            .map(|i| make_chunk(&format!("c{i}"), "hello"))
            .collect();
        // Huge char + attention budgets so only max_chunks binds.
        let batches = chunk_batches(&owned, 3, usize::MAX / 4, usize::MAX / 4);
        assert!(batches.iter().all(|b| b.len() <= 3));
        assert_eq!(batches.iter().map(|b| b.len()).sum::<usize>(), 10);
        assert_eq!(batches.len(), 4); // 3+3+3+1
    }

    #[test]
    fn chunk_batches_respects_max_chars() {
        // Two medium texts that fit alone but not together under a tight char budget.
        let a = make_chunk("a", &"a".repeat(500));
        let b = make_chunk("b", &"b".repeat(500));
        let owned = vec![a, b];
        let chars_a = emb_chars(&owned[0]);
        let chars_b = emb_chars(&owned[1]);
        // Allow each alone, not both together.
        let max_chars = chars_a.max(chars_b) + 10;
        assert!(chars_a + chars_b > max_chars);

        let batches = chunk_batches(&owned, 32, max_chars, usize::MAX / 4);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].len(), 1);
    }

    #[test]
    fn should_enforce_memory_floor_fail_closed_on_jetsam_levels() {
        // Regression: incident 2026-07-25. JetsamEvent reported ~0.08 GB free on a
        // 64 GB Mac. The old free_memory_reading_is_plausible hatch treated that as
        // "unreliable" and disabled the floor — indexing resumed and froze the host.
        // A reading this low MUST enforce (fail closed).
        let min_free = 10.0;
        let free = 0.08;
        assert!(
            should_enforce_memory_floor(free, min_free),
            "free={free} GB on 64 GB host must enforce min_free={min_free} (2026-07-25)"
        );
        // Unusable readings also enforce.
        assert!(should_enforce_memory_floor(f64::NAN, min_free));
        assert!(should_enforce_memory_floor(-1.0, min_free));
        // Disabled floor.
        assert!(!should_enforce_memory_floor(0.08, 0.0));
        // Healthy free.
        assert!(!should_enforce_memory_floor(20.0, min_free));
    }

    #[test]
    fn wait_for_memory_recovery_does_not_release_when_readings_fall() {
        // Inject a free-RAM series that keeps dropping. The wait must not exit
        // early by treating the reading as "implausible" — only recovery or
        // exhausted retries release.
        let readings = std::cell::RefCell::new(vec![0.5, 0.3, 0.1, 0.05]);
        let last = wait_for_memory_recovery_with(
            10.0,
            None,
            || {
                let mut v = readings.borrow_mut();
                if v.is_empty() {
                    0.01
                } else {
                    v.remove(0)
                }
            },
            0, // no sleep
            3, // three retry cycles after the initial reading
        );
        assert!(
            last < 10.0,
            "must return a still-low reading, not a faked value above the floor; got {last}"
        );
        assert!(
            should_enforce_memory_floor(last, 10.0),
            "caller must still pause after exhausted retries"
        );
    }

    #[test]
    fn wait_for_memory_recovery_returns_when_free_recovers() {
        let readings = std::cell::RefCell::new(vec![1.0, 2.0, 12.0]);
        let last = wait_for_memory_recovery_with(
            10.0,
            None,
            || {
                let mut v = readings.borrow_mut();
                v.remove(0)
            },
            0,
            5,
        );
        assert!(last >= 10.0, "expected recovery above floor, got {last}");
    }

    #[test]
    fn macos_pressure_says_pause_exhaustive() {
        let min_level = 25u32;
        // normal + high level → no pause
        assert!(!macos_pressure_says_pause(Some(1), Some(96), min_level));
        // warn → pause
        assert!(macos_pressure_says_pause(Some(2), Some(96), min_level));
        // critical → pause
        assert!(macos_pressure_says_pause(Some(4), Some(96), min_level));
        // probe unavailable → no pressure pause (GB floor covers separately)
        assert!(!macos_pressure_says_pause(None, None, min_level));
        // level just above threshold
        assert!(!macos_pressure_says_pause(Some(1), Some(25), min_level));
        assert!(!macos_pressure_says_pause(Some(1), Some(26), min_level));
        // level just below threshold
        assert!(macos_pressure_says_pause(Some(1), Some(24), min_level));
        assert!(macos_pressure_says_pause(Some(1), Some(0), min_level));
        // level None, pressure normal → no pause
        assert!(!macos_pressure_says_pause(Some(1), None, min_level));
        // pressure None, level low → pause
        assert!(macos_pressure_says_pause(None, Some(10), min_level));
    }

    #[test]
    fn macos_pressure_incident_2026_07_25_must_pause() {
        // JetsamEvent: free = 5601 pages (~87 MB) but inactive ≈ 8.1 GB —
        // sysinfo available_memory would report ~8 GB "free" while the machine
        // was freezing. Kernel pressure was critical; that signal alone must pause.
        let naive_free_gb = 8.0;
        let min_free_gb = 5.0;
        // Naive free-RAM floor would NOT fire:
        assert!(
            !should_enforce_memory_floor(naive_free_gb, min_free_gb),
            "regression setup: naive free_gb must look healthy"
        );
        // Kernel critical pressure must still pause:
        assert!(
            macos_pressure_says_pause(Some(4), Some(1), 25),
            "critical pressure while naive free≈8GB must pause (2026-07-25)"
        );
    }
}
