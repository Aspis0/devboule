//! Chunk-index manifest IO and up-to-date decision logic.
//!
//! Port of the manifest helpers in `oracle/ingestion/chunk_index.py`
//! (`load_manifest`, `save_manifest`, `manifest_roots`,
//! `strip_verbatim_prefix`, `manifest_files_for_root`, `file_signature`,
//! `file_needs_index`, `text_chunks_up_to_date`). On-disk structure mirrors
//! the Python manifest exactly (top-level `version`, `root`, `roots`, `files`).

use crate::config::active_chunk_profile_version;
use crate::store::sqlite::SqliteStore;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Windows verbatim/extended-length UNC prefix (`\\?\UNC\`).
/// Python source: `value.startswith("\\\\?\\UNC\\")`.
const VERBATIM_UNC: &str = "\\\\?\\UNC\\";
/// Windows verbatim/extended-length prefix (`\\?\`).
/// Python source: `value.startswith("\\\\?\\")`.
const VERBATIM: &str = "\\\\?\\";

/// One per-file manifest entry.
///
/// Mirrors `chunk_index.py::file_signature` output (with `chunks` /
/// `chunk_profile` present only after indexing).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ManifestFileEntry {
    pub size: i64,
    pub mtime_ns: i64,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_profile: Option<String>,
}

/// One root entry: `{ "files": { file_id: entry, … } }`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RootEntry {
    #[serde(default)]
    pub files: HashMap<String, ManifestFileEntry>,
}

/// The full chunk-index manifest.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Manifest {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub roots: HashMap<String, RootEntry>,
    #[serde(default)]
    pub files: HashMap<String, ManifestFileEntry>,
}

/// Strip the Windows extended-length / verbatim prefix (`\\?\` and `\\?\UNC\`)
/// so the same workspace has a single canonical manifest identity. Mirrors
/// `chunk_index.py::strip_verbatim_prefix` (no-op on non-Windows paths).
pub fn strip_verbatim_prefix(value: &str) -> String {
    if let Some(rest) = value.strip_prefix(VERBATIM_UNC) {
        // Python: `"\\\\" + value[len("\\\\?\\UNC\\"):]`
        return format!("\\\\{rest}");
    }
    if let Some(rest) = value.strip_prefix(VERBATIM) {
        return rest.to_string();
    }
    value.to_string()
}

/// Load the manifest, returning `{"files": {}}` on a missing or malformed file.
/// Mirrors `chunk_index.py::load_manifest`.
pub fn load_manifest(path: &Path) -> Manifest {
    if !path.exists() {
        return Manifest {
            files: HashMap::new(),
            ..Default::default()
        };
    }
    match std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Manifest>(&t).ok())
    {
        Some(m) => m,
        None => Manifest {
            files: HashMap::new(),
            ..Default::default()
        },
    }
}

/// Atomically write the manifest (tmp file + rename). Mirrors
/// `chunk_index.py::save_manifest` (`indent=2`, `ensure_ascii=False`).
pub fn save_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| anyhow!("cannot build tmp path for {}", path.display()))?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let text = serde_json::to_string_pretty(manifest).context("serializing manifest")?;
    std::fs::write(&tmp, text)
        .with_context(|| format!("writing manifest tmp {}", tmp.display()))?;
    // Equivalent to Python's `os.replace` on every platform: on Windows the
    // std implementation calls MoveFileExW with MOVEFILE_REPLACE_EXISTING,
    // so an existing manifest is atomically overwritten (verified in
    // library/std/src/sys/fs/windows.rs).
    std::fs::rename(&tmp, path).with_context(|| format!("renaming manifest {}", path.display()))?;
    Ok(())
}

/// Migrate the legacy single-root format into `roots` and force `version = 2`.
/// Returns the `roots` map (mutably). Mirrors `chunk_index.py::manifest_roots`.
pub fn manifest_roots(manifest: &mut Manifest) -> &mut HashMap<String, RootEntry> {
    let legacy_root = manifest.root.clone().unwrap_or_default();
    let legacy_files = manifest.files.clone();
    if !legacy_root.is_empty() && !manifest.roots.contains_key(&legacy_root) {
        manifest
            .roots
            .entry(legacy_root)
            .or_insert_with(|| RootEntry {
                files: legacy_files,
            });
    }
    manifest.version = 2;
    &mut manifest.roots
}

/// Get (creating if requested) the per-file entry map for a root, applying the
/// verbatim-prefix duplicate-pruning and legacy-mirror side effects. Mirrors
/// `chunk_index.py::manifest_files_for_root`.
///
/// Returns `None` when the root has no entry and `create` is false — Python
/// returns a DETACHED `{}` there and mutates nothing (the dup-prune above,
/// however, mutates in both implementations, exactly like Python).
///
/// NOTE on the legacy mirror: Python ALIASES `manifest["files"]` to the live
/// per-root dict, so later mutations show through automatically; Rust clones,
/// so the mirror goes stale after further mutations — callers must run
/// [`sync_legacy_manifest_root`] before [`save_manifest`].
pub fn manifest_files_for_root<'a>(
    manifest: &'a mut Manifest,
    root: &Path,
    create: bool,
) -> Option<&'a mut HashMap<String, ManifestFileEntry>> {
    let root_key = strip_verbatim_prefix(&root.to_string_lossy());

    // Prune a stale verbatim-prefixed duplicate of THIS root.
    let dup_keys: Vec<String> = manifest
        .roots
        .keys()
        .filter(|k| *k != &root_key && strip_verbatim_prefix(k) == root_key)
        .cloned()
        .collect();
    for existing_key in dup_keys {
        if let Some(duplicate) = manifest.roots.remove(&existing_key) {
            let canonical = manifest.roots.entry(root_key.clone()).or_default();
            for (fid, rec) in duplicate.files {
                canonical.files.entry(fid).or_insert(rec);
            }
        }
    }

    if !manifest.roots.contains_key(&root_key) {
        if !create {
            return None;
        }
        manifest
            .roots
            .insert(root_key.clone(), RootEntry::default());
    }

    let files_snapshot = manifest.roots.get(&root_key).unwrap().files.clone();
    manifest.root = Some(root_key.clone());
    manifest.files = files_snapshot;
    Some(&mut manifest.roots.get_mut(&root_key).unwrap().files)
}

/// Keep the legacy `root` / `files` mirror in sync (used after mutations).
/// Mirrors `chunk_index.py::sync_legacy_manifest_root`.
pub fn sync_legacy_manifest_root(manifest: &mut Manifest, root: &Path) {
    let files = manifest_files_for_root(manifest, root, true)
        .expect("create=true always yields an entry")
        .clone();
    manifest.root = Some(strip_verbatim_prefix(&root.to_string_lossy()));
    manifest.files = files;
}

/// Current file signature (`size`, `mtime_ns`, `updated_at`, and optionally
/// `chunks` / `chunk_profile`). Mirrors `chunk_index.py::file_signature`.
pub fn file_signature(path: &Path, chunks: Option<u64>) -> Result<ManifestFileEntry> {
    let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let size = meta.len() as i64;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    // Python `datetime.isoformat()`: 6-digit microseconds when non-zero, no
    // fractional part at all when the microsecond field is exactly 0.
    let now = Utc::now();
    let updated_at = if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
    };
    let mut entry = ManifestFileEntry {
        size,
        mtime_ns,
        updated_at,
        chunks: None,
        chunk_profile: None,
    };
    if let Some(c) = chunks {
        entry.chunks = Some(c);
        entry.chunk_profile = Some(active_chunk_profile_version(None));
    }
    Ok(entry)
}

/// `path.relative_to(root).as_posix()` — POSIX-style relative file id.
/// Python raises `ValueError` when `path` is not under `root`; mirror that
/// with an error instead of silently keying the manifest on an absolute path.
fn relative_posix(path: &Path, root: &Path) -> Result<String> {
    let rel = path.strip_prefix(root).map_err(|_| {
        anyhow!(
            "path {} is not under root {}",
            path.display(),
            root.display()
        )
    })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

/// Whether a file needs re-indexing. Mirrors `chunk_index.py::file_needs_index`.
///
/// **Vector-unaware**: when the signature and chunk profile match and SQLite
/// already has text chunks for the file, this returns `false` even if Lance
/// vectors are missing (e.g. after a backend switch that wiped `chunks.lancedb`,
/// or a prior run that paused before embed). Callers that need vectors must
/// pass `force=true` on the index job (UI "Force re-index") so the pipeline
/// re-embeds regardless of this check.
pub fn file_needs_index(
    path: &Path,
    root: &Path,
    manifest_files: &HashMap<String, ManifestFileEntry>,
    sqlite: &SqliteStore,
) -> Result<bool> {
    let file_id = relative_posix(path, root)?;
    let current = file_signature(path, None)?;
    let Some(previous) = manifest_files.get(&file_id) else {
        return Ok(true);
    };
    if previous.size != current.size || previous.mtime_ns != current.mtime_ns {
        return Ok(true);
    }
    let active = active_chunk_profile_version(None);
    if previous.chunk_profile.as_deref() != Some(active.as_str()) {
        return Ok(true);
    }
    if previous.chunks == Some(0) {
        return Ok(false);
    }
    // Text-only freshness: empty SQLite chunks → needs index; present chunks
    // → skip (does NOT inspect Lance vectors).
    Ok(sqlite.chunks_for_file(&file_id)?.is_empty())
}

/// Whether a file's text chunks are already current. Mirrors
/// `chunk_index.py::text_chunks_up_to_date` (the inverted negation of
/// `file_needs_index`).
pub fn text_chunks_up_to_date(
    path: &Path,
    root: &Path,
    manifest_files: &HashMap<String, ManifestFileEntry>,
    sqlite: &SqliteStore,
) -> Result<bool> {
    let file_id = relative_posix(path, root)?;
    let Some(previous) = manifest_files.get(&file_id) else {
        return Ok(false);
    };
    let current = file_signature(path, None)?;
    if previous.size != current.size || previous.mtime_ns != current.mtime_ns {
        return Ok(false);
    }
    let active = active_chunk_profile_version(None);
    if previous.chunk_profile.as_deref() != Some(active.as_str()) {
        return Ok(false);
    }
    if previous.chunks == Some(0) {
        return Ok(true);
    }
    Ok(!sqlite.chunks_for_file(&file_id)?.is_empty())
}
