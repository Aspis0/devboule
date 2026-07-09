//! Augure sin ledger — persisted, per-file shard storage.
//!
//! Storage shape (mirrors the censor house pattern):
//!   `<project_root>/.aspis-polis/sins/<sha256(rel_path)>.json` per file shard,
//!   plus `_project.json` (reserved for future project-level sins;
//!   today all sins are per-file — this shard is only exercised by tests).
//!
//! Each shard carries its own `<shard>.lock` sidecar and is written atomically
//! (write-to-temp-then-rename) under a global write mutex, matching the idiom
//! used by `meta_store::META_WRITE_LOCK`. The shard shape:
//!
//! ```ignore
//! { "relPath": "...", "contentHash": "...", "updatedAt": "...", "sins": [...] }
//! ```
//!
//! Semantics:
//!   - Same content_hash: keep stored dispositions; new sins added as Open;
//!     sins present in store but absent from fresh set at SAME hash → marked
//!     `Fixed` (the checker observed the condition gone); never deleted.
//!   - Different content_hash: fresh evaluation REPLACES the shard (ignores
//!     reset — Censor semantics, deliberate carry-nothing-over).
//!   - `Fixed` records older than 30 days are pruned on write (keep bounded).
//!
//! The public API:
//!   - [`upsert_scan_results`] — scan-time merge.
//!   - [`dispose`] — set a sin's disposition (human can only set Open/Ignored).
//!   - [`load_open_sins`] / [`load_all_sins`] — read-only walks, tolerant of
//!     corrupt shards (skip + continue).
//!
//! All writes serialize through `SIN_WRITE_LOCK` (global mutex).

use super::{Disposition, SinRecord};
use fs2::FileExt;

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Top-level directory under the project root holding the sin shards.
pub const POLIS_DIR: &str = ".aspis-polis";

/// Subdirectory under `.aspis-polis/` holding the per-file sin shards.
pub const SINS_SUBDIR: &str = "sins";

/// Special shard name reserved for future project-level sins.
/// Today all sins are per-file; this shard is only exercised by tests.
const PROJECT_SHARD_NAME: &str = "_project";

/// Fixed sins older than this are pruned from shards on write.
const FIXED_PRUNE_AGE_DAYS: i64 = 30;

/// Spin-lock parameters matching the censor ledger pattern.
const LOCK_ATTEMPTS: u32 = 100;
const LOCK_SPIN_INTERVAL: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// Global write mutex (same idiom as META_WRITE_LOCK)
// ---------------------------------------------------------------------------

/// Process-level serialization lock for ALL `.aspis-polis/sins/` writes.
///
/// Ensures concurrent writes from the scanner terminal save and the D8 dispose
/// command cannot interleave on the same shard. The lock is held only across the
/// (fast, synchronous) read→merge→atomic-write section. It is NEVER held across
/// any IO that could block for more than a few milliseconds (the actual scan
/// runs UNLOCKED; only the final ledger commit takes the lock).
static SIN_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Per-process monotonic counter for unique temp/backup file names.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Shard on-disk shape
// ---------------------------------------------------------------------------

/// One per-file shard stored at `<root>/.aspis-polis/sins/<sha256(rel)>.json`.
///
/// All fields carry `#[serde(default)]` for forward-compat: a shard written by a
/// newer build must still deserialize on an older build without panicking.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PolisSinShard {
    #[serde(default)]
    pub rel_path: String,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub sins: Vec<SinRecord>,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// The `.aspis-polis/sins/` directory under a project root.
fn sins_dir(root: &Path) -> PathBuf {
    root.join(POLIS_DIR).join(SINS_SUBDIR)
}

/// Normalize a project-relative path for shard hashing: backslashes → forward
/// slashes, consecutive slashes collapsed. Byte-identical for POSIX and Windows
/// forms of the same logical path.
fn normalize_rel_path(file_rel_path: &str) -> String {
    let slashed = file_rel_path.replace('\\', "/");
    let mut out = String::with_capacity(slashed.len());
    let mut prev_slash = false;
    for ch in slashed.chars() {
        if ch == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        out.push(ch);
    }
    out
}

/// Validate a rel path — reject absolute paths, `..` traversal, and
/// `-`-leading components (argv-injection guard, mirroring censor ledger).
fn validate_rel_path(rel: &str) -> io::Result<()> {
    let normalized = rel.replace('\\', "/");
    // Windows drive paths (C:\..., C:/...) rejected on all hosts.
    let mut head = normalized.bytes();
    if let (Some(first), Some(b':')) = (head.next(), head.next()) {
        if first.is_ascii_alphabetic() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("polis sin rel path must be relative, got absolute: {rel}"),
            ));
        }
    }
    let path = Path::new(&normalized);
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("polis sin rel path must not contain '..': {rel}"),
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("polis sin rel path must be relative, got absolute: {rel}"),
                ));
            }
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                for piece in name.split(['/', '\\']) {
                    if piece.starts_with('-') {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "polis sin rel path component must not start with '-': {rel}"
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Shard path for one file:
/// `<root>/.aspis-polis/sins/<sha256(normalized_rel)>.json`
///
/// The special shard `_project.json` is used for the empty rel_path case
/// (reserved for future project-level sins).
fn shard_path(root: &Path, file_rel_path: &str) -> io::Result<PathBuf> {
    if file_rel_path.is_empty() {
        return Ok(sins_dir(root).join(format!("{PROJECT_SHARD_NAME}.json")));
    }
    validate_rel_path(file_rel_path)?;
    let normalized = normalize_rel_path(file_rel_path);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let name = hex::encode(hasher.finalize());
    Ok(sins_dir(root).join(format!("{name}.json")))
}

// ---------------------------------------------------------------------------
// Locking (matching the censor house pattern)
// ---------------------------------------------------------------------------

/// RAII lock on a shard's `<shard>.lock` sidecar (fs2 exclusive).
struct ShardLock {
    _file: File,
}

fn lock_shard(shard_path: &Path) -> io::Result<ShardLock> {
    if let Some(parent) = shard_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = shard_path.with_extension("json.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)?;
    for attempt in 0..LOCK_ATTEMPTS {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(ShardLock { _file: file }),
            Err(_) if attempt + 1 < LOCK_ATTEMPTS => thread::sleep(LOCK_SPIN_INTERVAL),
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!(
                        "could not acquire polis sin shard lock: {}: {e}",
                        lock_path.display()
                    ),
                ));
            }
        }
    }
    unreachable!("lock_shard loop always returns for LOCK_ATTEMPTS > 0")
}

// ---------------------------------------------------------------------------
// Shard IO
// ---------------------------------------------------------------------------

/// Read a shard at `path`. `Ok(None)` for genuinely absent file.
/// A present-but-unparseable file returns `Err(InvalidData)` — the write path
/// aborts rather than overwriting unreadable prior data.
fn read_shard_at(path: &Path) -> io::Result<Option<PolisSinShard>> {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<PolisSinShard>(&content) {
            Ok(shard) => Ok(Some(shard)),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "corrupt polis sin shard (unparseable JSON): {}",
                    path.display()
                ),
            )),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Atomic shard write (lock already held). Write to temp sibling, then rename.
fn write_shard_locked(path: &Path, shard: &PolisSinShard) -> io::Result<()> {
    let content = serde_json::to_string_pretty(shard)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let stamp = format!(
        "{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let temp_path = path.with_extension(format!("json.{stamp}.tmp"));

    if let Err(e) = fs::write(&temp_path, content) {
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }
    // Atomic rename (same volume — guaranteed by sibling temp).
    if let Err(e) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Prune old Fixed sins
// ---------------------------------------------------------------------------

/// Remove `Fixed` records whose `updated_at` is older than `FIXED_PRUNE_AGE_DAYS`
/// days from `now`.
fn prune_old_fixed(shard: &mut PolisSinShard, now: &chrono::DateTime<chrono::Utc>) {
    let cutoff = *now - chrono::Duration::days(FIXED_PRUNE_AGE_DAYS);
    shard.sins.retain(|sin| {
        if sin.disposition != Disposition::Fixed {
            return true;
        }
        // If we can't parse the timestamp, keep the record (conservative).
        match chrono::DateTime::parse_from_rfc3339(&sin.updated_at) {
            Ok(dt) => dt >= cutoff,
            Err(_) => true,
        }
    });
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Merge a scan result into the persisted sin ledger.
///
/// `root` — the project root.
/// `per_file` — slice of `(rel_path, current_content_hash, fresh_sins)` for
///   each file that was scanned. Files NOT in this list are left untouched.
///
/// Semantics:
///   - Same `content_hash` as stored: keep dispositions for sins with matching
///     `id`; new sins added as `Open`; sins in the store but absent from the
///     fresh set at the SAME hash → marked `Fixed` (checker observed it gone).
///   - Different `content_hash`: shard replaced entirely with fresh sins (all
///     `Open`) — ignors reset on content change.
///   - `Fixed` records older than 30 days are pruned.
///
/// TODO(P1.2): sweep_orphans(root, known_rel_paths) — deleted/renamed files
/// leave shards behind forever. A periodic sweep keyed off the scanner's
/// current file set is needed to bound the shard directory.
pub fn upsert_scan_results(
    root: &Path,
    per_file: &[(String, String, Vec<SinRecord>)],
) -> io::Result<Vec<SinRecord>> {
    let _guard = SIN_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();
    let mut all_merged: Vec<SinRecord> = Vec::new();

    for (rel_path, new_hash, fresh_sins) in per_file {
        let path = shard_path(root, rel_path)?;
        let _shard_lock = lock_shard(&path)?;

        let existing = match read_shard_at(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "polis augure: refusing to overwrite unreadable shard: {}",
                    path.display()
                );
                return Err(e);
            }
        };

        let merged = merge_sins(existing, fresh_sins, new_hash, rel_path, &now_str, &now);
        all_merged.extend(merged.sins.clone());
        write_shard_locked(&path, &merged)?;
    }

    Ok(all_merged)
}

/// Pure merge of fresh sins into an existing shard.
fn merge_sins(
    existing: Option<PolisSinShard>,
    fresh_sins: &[SinRecord],
    new_hash: &str,
    rel_path: &str,
    now_str: &str,
    now: &chrono::DateTime<chrono::Utc>,
) -> PolisSinShard {
    // Build a map of old sins by id (only those at the SAME content hash).
    // Sins at a different hash are dropped entirely.
    let mut old_by_id: HashMap<String, SinRecord> = HashMap::new();
    if let Some(ref old_shard) = existing {
        if old_shard.content_hash == new_hash {
            for sin in &old_shard.sins {
                old_by_id.insert(sin.id.clone(), sin.clone());
            }
        }
    }

    // Dedup fresh sins by id (last-wins), mirroring censor supersede_inner.
    // A checker that emits the same id twice must not produce duplicate records.
    let mut order: Vec<&str> = Vec::with_capacity(fresh_sins.len());
    let mut deduped: HashMap<&str, &SinRecord> = HashMap::new();
    for s in fresh_sins {
        if !deduped.contains_key(s.id.as_str()) {
            order.push(s.id.as_str());
        }
        deduped.insert(s.id.as_str(), s);
    }

    let mut merged: Vec<SinRecord> = Vec::new();
    let mut emitted_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 1) Merge fresh sins: preserve disposition from old if same id exists.
    for id in &order {
        let fresh = deduped[id];
        let mut sin = fresh.clone();
        if let Some(old) = old_by_id.get(fresh.id.as_str()) {
            // Same id at same hash: carry over disposition. By definition the
            // disposition hasn't changed (it's a carry-over), so keep the old
            // updated_at — the re-detection is not a state change.
            sin.disposition = old.disposition;
            sin.created_at = old.created_at.clone();
            sin.updated_at = old.updated_at.clone();
        } else {
            // New sin: always enters as Open (the scanner only reports observed
            // conditions — Fixed is minted exclusively by the absence rule below),
            // with ledger-owned timestamps.
            sin.disposition = Disposition::Open;
            sin.created_at = now_str.to_string();
            sin.updated_at = now_str.to_string();
        }
        // Stamp content_hash to the current hash.
        sin.content_hash = new_hash.to_string();
        emitted_ids.insert(sin.id.clone());
        merged.push(sin);
    }

    // 2) Sins present in old store but absent from fresh set at the SAME hash
    //    → mark Fixed (the checker observed the condition gone).
    if existing.is_some()
        && existing.as_ref().map(|s| s.content_hash.as_str()) == Some(new_hash)
    {
        for (id, mut old_sin) in old_by_id {
            if !emitted_ids.contains(&id) {
                if old_sin.disposition == Disposition::Fixed {
                    // Already fixed previously — keep it.
                    merged.push(old_sin);
                } else {
                    // Not re-emitted → the condition is gone → mark Fixed.
                    old_sin.disposition = Disposition::Fixed;
                    old_sin.updated_at = now_str.to_string();
                    merged.push(old_sin);
                }
            }
        }
    }

    let mut shard = PolisSinShard {
        rel_path: rel_path.to_string(),
        content_hash: new_hash.to_string(),
        updated_at: now_str.to_string(),
        sins: merged,
    };

    // 3) Prune old Fixed records.
    prune_old_fixed(&mut shard, now);

    shard
}

/// Set a sin's disposition. Returns `Ok(true)` if found and updated,
/// `Ok(false)` if the sin was not found in any shard.
///
/// When `rel_path_hint` is `Some`, the target shard is addressed directly
/// (one lock, no directory scan). When `None`, the sins directory is scanned
/// lock-free first to locate the sin; only the matching shard is then locked
/// for the read-modify-write. Corrupt shards encountered during the scan are
/// tracked and surfaced as an `Err` when the sin was not found in any healthy
/// shard (a corrupt shard that might have contained the sin must not silently
/// produce `Ok(false)`).
///
/// **Human callers can only set `Open` or `Ignored`.** Attempting to set
/// `Fixed` returns an `Err` with a descriptive message — the checker, not
/// the coder, is the arbiter of fixed (D8 rule).
pub fn dispose(
    root: &Path,
    rel_path_hint: Option<&str>,
    sin_id: &str,
    disposition: Disposition,
) -> Result<bool, String> {
    if disposition == Disposition::Fixed {
        return Err(
            "Cannot manually set a sin to Fixed — the checker, not the coder, is the arbiter of fixed.".to_string(),
        );
    }

    // Fast path: caller knows exactly which shard to touch.
    if let Some(rel) = rel_path_hint {
        let shard_path = shard_path(root, rel)
            .map_err(|e| format!("invalid rel_path_hint: {e}"))?;

        let _guard = SIN_WRITE_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _shard_lock =
            lock_shard(&shard_path).map_err(|e| format!("could not lock shard: {e}"))?;

        let mut shard = match read_shard_at(&shard_path) {
            Ok(Some(s)) => s,
            Ok(None) => return Ok(false),
            Err(e) => return Err(format!("could not read shard: {e}")),
        };

        let now_str = chrono::Utc::now().to_rfc3339();
        let found = shard.sins.iter_mut().any(|sin| {
            if sin.id == sin_id {
                sin.disposition = disposition;
                sin.updated_at = now_str.clone();
                true
            } else {
                false
            }
        });

        if found {
            write_shard_locked(&shard_path, &shard)
                .map_err(|e| format!("Failed to write shard after dispose: {e}"))?;
            return Ok(true);
        }
        return Ok(false);
    }

    // Fallback: scan the sins directory to locate the sin.
    // Phase 1 (no lock): read-only candidate search.
    let dir = sins_dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("Failed to read sins directory: {e}")),
    };

    let mut found_shard: Option<PathBuf> = None;
    let mut corrupt_count: usize = 0;
    let mut first_corrupt: Option<PathBuf> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match read_shard_at(&path) {
            Ok(Some(shard)) => {
                if shard.sins.iter().any(|sin| sin.id == sin_id) {
                    found_shard = Some(path);
                    break;
                }
            }
            Ok(None) => {}
            Err(_) => {
                corrupt_count += 1;
                if first_corrupt.is_none() {
                    first_corrupt = Some(path.clone());
                }
            }
        }
    }

    // Phase 2 (under lock): if a candidate was found, lock, re-verify, and commit.
    if let Some(shard_path) = found_shard {
        let _guard = SIN_WRITE_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _shard_lock =
            lock_shard(&shard_path).map_err(|e| format!("could not lock shard: {e}"))?;

        // Re-verify the sin is still present under the lock (TOCTOU guard).
        let mut shard = match read_shard_at(&shard_path) {
            Ok(Some(s)) => s,
            Ok(None) => return Ok(false),
            Err(e) => return Err(format!("could not read shard under lock: {e}")),
        };

        let now_str = chrono::Utc::now().to_rfc3339();
        let found = shard.sins.iter_mut().any(|sin| {
            if sin.id == sin_id {
                sin.disposition = disposition;
                sin.updated_at = now_str.clone();
                true
            } else {
                false
            }
        });

        if found {
            write_shard_locked(&shard_path, &shard)
                .map_err(|e| format!("Failed to write shard after dispose: {e}"))?;
            return Ok(true);
        }
        return Ok(false);
    }

    // Not found in any healthy shard.
    if corrupt_count > 0 {
        let first = first_corrupt.unwrap();
        return Err(format!(
            "sin not found; {corrupt_count} corrupt shard(s) skipped: {}",
            first.display()
        ));
    }
    Ok(false)
}

/// Load all sins with disposition `Open` from the ledger.
/// Tolerant of corrupt shards (skip + continue).
pub fn load_open_sins(root: &Path) -> Vec<SinRecord> {
    load_sins(root, |sin| sin.disposition == Disposition::Open)
}

/// Load ALL sins from the ledger regardless of disposition.
/// Tolerant of corrupt shards (skip + continue).
pub fn load_all_sins(root: &Path) -> Vec<SinRecord> {
    load_sins(root, |_| true)
}

/// Walk the sins directory and collect sins matching a predicate.
/// Corrupt shards are skipped (logged to stderr).
fn load_sins(root: &Path, predicate: impl Fn(&SinRecord) -> bool) -> Vec<SinRecord> {
    let dir = sins_dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("polis augure: error reading sins directory: {e}");
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match read_shard_at(&path) {
            Ok(Some(shard)) => {
                for sin in shard.sins {
                    if predicate(&sin) {
                        out.push(sin);
                    }
                }
            }
            Ok(None) => {} // genuinely absent (race between read_dir and read)
            Err(e) => {
                eprintln!(
                    "polis augure: skipping corrupt shard during load: {}",
                    e
                );
            }
        }
    }
    out
}

/// Check whether a shard exists for `rel_path` (cheap existence test, no lock).
pub fn has_shard(root: &Path, rel_path: &str) -> bool {
    match shard_path(root, rel_path) {
        Ok(p) => p.exists(),
        Err(_) => false,
    }
}

/// Remove shards whose `relPath` is not in the `known` set.
/// `_project.json` is exempt (reserved for future project-level sins).
/// Called after `upsert_scan_results` to bound the shard directory.
///
/// Acquires `SIN_WRITE_LOCK` around the whole read-decide-delete section
/// so a concurrent `dispose` cannot land on a candidate as it is deleted.
/// Each candidate shard is locked individually before reading+deleting.
///
/// NOTE: a file whose extension is disabled in the in-game File-Types menu
/// is absent from `known` → its shard is swept → disposition history is gone
/// if the extension is later re-enabled. This is deliberate and consistent
/// with `meta_store::retain_paths` (the meta store prunes unknown files too).
pub fn sweep_orphans(root: &Path, known: &std::collections::HashSet<String>) -> io::Result<()> {
    let _guard = SIN_WRITE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    let dir = sins_dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // Never remove _project.json.
        if path
            .file_stem()
            .and_then(|s| s.to_str())
            == Some(PROJECT_SHARD_NAME)
        {
            continue;
        }
        let _shard_lock = lock_shard(&path)?;
        match read_shard_at(&path) {
            Ok(Some(shard)) => {
                if shard.rel_path.is_empty() || !known.contains(&shard.rel_path) {
                    let _ = fs::remove_file(&path);
                    let _ = fs::remove_file(path.with_extension("json.lock"));
                }
            }
            Ok(None) | Err(_) => {
                // Absent or corrupt: remove it (corrupt shards with no
                // corresponding source file are dead weight).
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("json.lock"));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{compute_sin_id, SinRecord, Disposition as SDisposition};

    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    /// Unique temp dir per test.
    fn unique_temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aspis-polis-augure-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mk_sin(id: &str, evidence: &str, rule_id: &str, line: Option<u32>, hash: &str) -> SinRecord {
        SinRecord {
            id: id.to_string(),
            rel_path: "src/test.rs".to_string(),
            rule_id: rule_id.to_string(),
            line,
            severity: "inferno".to_string(),
            description: format!("desc for {id}"),
            evidence: evidence.to_string(),
            content_hash: hash.to_string(),
            disposition: SDisposition::Open,
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            updated_at: "2026-01-01T00:00:00+00:00".to_string(),
        }
    }

    fn mk_sin_with_disposition(
        id: &str,
        evidence: &str,
        rule_id: &str,
        line: Option<u32>,
        hash: &str,
        disposition: SDisposition,
    ) -> SinRecord {
        let mut s = mk_sin(id, evidence, rule_id, line, hash);
        s.disposition = disposition;
        s
    }

    fn mk_sin_det(id: &str, hash: &str) -> SinRecord {
        mk_sin(id, "some evidence", "secret", Some(1), hash)
    }

    // =========================================================================
    // Test 1: Round-trip — upsert → load_open_sins returns them
    // =========================================================================

    #[test]
    fn round_trip_upsert_then_load() {
        let dir = unique_temp_root("roundtrip");
        let root = dir.as_path();

        let s1 = mk_sin_det("id-aaa", "hash1");
        let s2 = mk_sin_det("id-bbb", "hash1");
        let per_file = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![s1, s2])];

        upsert_scan_results(root, &per_file).unwrap();
        let loaded = load_open_sins(root);
        assert_eq!(loaded.len(), 2);
        let ids: Vec<&str> = loaded.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"id-aaa"));
        assert!(ids.contains(&"id-bbb"));

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Test 2: Disposition survival — upsert, dispose(Ignored), re-upsert same
    //         hash + same sin set → still Ignored
    // =========================================================================

    #[test]
    fn disposition_survives_re_upsert_same_hash() {
        let dir = unique_temp_root("dispo-survive");
        let root = dir.as_path();

        let s1 = mk_sin_det("id-keep", "hash1");
        let per_file = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![s1])];

        // First upsert.
        upsert_scan_results(root, &per_file).unwrap();

        // Dispose → Ignored.
        let found = dispose(root, None, "id-keep", SDisposition::Ignored).unwrap();
        assert!(found, "must find id-keep");

        // Record updated_at after dispose.
        let after_dispose = load_all_sins(root);
        let dispo_updated = after_dispose
            .iter()
            .find(|s| s.id == "id-keep")
            .unwrap()
            .updated_at
            .clone();

        // Re-upsert SAME hash with same sin.
        let s1_again = mk_sin_det("id-keep", "hash1");
        let per_file2 = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![s1_again])];
        upsert_scan_results(root, &per_file2).unwrap();

        // Should still be Ignored (disposition survived).
        let all = load_all_sins(root);
        let the_sin = all.iter().find(|s| s.id == "id-keep").unwrap();
        assert_eq!(the_sin.disposition, SDisposition::Ignored);
        // Re-upsert at same hash with same disposition must NOT bump updated_at
        // (carry-over is not a state change).
        assert_eq!(
            the_sin.updated_at, dispo_updated,
            "re-upsert carry-over must not bump updated_at"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Test 3: Fixed-by-absence — upsert 2 sins, re-upsert same hash with only
    //         1 → the missing one is Fixed, not deleted
    // =========================================================================

    #[test]
    fn fixed_by_absence_same_hash() {
        let dir = unique_temp_root("fixed-absence");
        let root = dir.as_path();

        let a = mk_sin_det("id-a", "hash1");
        let b = mk_sin_det("id-b", "hash1");
        let per_file = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![a, b])];
        upsert_scan_results(root, &per_file).unwrap();

        // Re-upsert same hash with only id-a.
        let a2 = mk_sin_det("id-a", "hash1");
        let per_file2 = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![a2])];
        upsert_scan_results(root, &per_file2).unwrap();

        let all = load_all_sins(root);
        assert_eq!(all.len(), 2, "both sins should still be present");

        let sin_a = all.iter().find(|s| s.id == "id-a").unwrap();
        assert_eq!(sin_a.disposition, SDisposition::Open);

        let sin_b = all.iter().find(|s| s.id == "id-b").unwrap();
        assert_eq!(
            sin_b.disposition,
            SDisposition::Fixed,
            "absent sin should be marked Fixed, not deleted"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Test 4: Hash change resets — dispose(Ignored), re-upsert with NEW
    //         content_hash and same logical sin → Open again
    // =========================================================================

    #[test]
    fn hash_change_resets_disposition() {
        let dir = unique_temp_root("hash-reset");
        let root = dir.as_path();

        let s1 = mk_sin_det("id-reset", "hash1");
        let per_file = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![s1])];
        upsert_scan_results(root, &per_file).unwrap();

        // Dispose → Ignored.
        dispose(root, None, "id-reset", SDisposition::Ignored).unwrap();

        // Re-upsert with NEW hash, same sin.
        let s2 = mk_sin_det("id-reset", "hash2");
        let per_file2 = vec![("src/test.rs".to_string(), "hash2".to_string(), vec![s2])];
        upsert_scan_results(root, &per_file2).unwrap();

        let all = load_all_sins(root);
        let the_sin = all.iter().find(|s| s.id == "id-reset").unwrap();
        assert_eq!(
            the_sin.disposition,
            SDisposition::Open,
            "hash change must reset disposition to Open"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Test 5: Human cannot set Fixed — dispose(..., Fixed) errors
    // =========================================================================

    #[test]
    fn human_cannot_set_fixed() {
        let dir = unique_temp_root("no-fixed");
        let root = dir.as_path();

        let s1 = mk_sin_det("id-no-fix", "hash1");
        let per_file = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![s1])];
        upsert_scan_results(root, &per_file).unwrap();

        let result = dispose(root, None, "id-no-fix", SDisposition::Fixed);
        assert!(result.is_err(), "human must not be able to set Fixed");
        assert!(
            result.unwrap_err().contains("arbiter of fixed"),
            "error message must explain why Fixed is rejected"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Test 6: Corrupt shard tolerated — write garbage JSON to one shard,
    //         load_open_sins still returns the others
    // =========================================================================

    #[test]
    fn corrupt_shard_tolerated_on_load() {
        let dir = unique_temp_root("corrupt-tolerate");
        let root = dir.as_path();

        // Write one healthy shard.
        let s1 = mk_sin_det("id-healthy", "hash1");
        let per_file = vec![("src/healthy.rs".to_string(), "hash1".to_string(), vec![s1])];
        upsert_scan_results(root, &per_file).unwrap();

        // Manually write garbage JSON as a corrupt shard.
        let corrupt_dir = sins_dir(root);
        fs::create_dir_all(&corrupt_dir).unwrap();
        let corrupt_path = corrupt_dir.join("corrupt.json");
        fs::write(&corrupt_path, "{ not valid json !!! ").unwrap();

        // Load should still return the healthy sin, skipping the corrupt one.
        let open = load_open_sins(root);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, "id-healthy");

        // load_all_sins also tolerant.
        let all = load_all_sins(root);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "id-healthy");

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Test 7: id determinism + evidence_key normalization
    //         (case/whitespace changes in evidence → same id)
    // =========================================================================

    #[test]
    fn id_determinism_and_evidence_normalization() {
        let dir = unique_temp_root("id-determ");
        let root = dir.as_path();

        // First upsert with evidence "secret at line 1".
        let id1 = compute_sin_id("src/a.rs", "secret", Some(1), "secret at line 1");
        let s1 = mk_sin(&id1, "secret at line 1", "secret", Some(1), "hash1");
        let per_file = vec![("src/a.rs".to_string(), "hash1".to_string(), vec![s1.clone()])];
        upsert_scan_results(root, &per_file).unwrap();

        // Second upsert with different evidence casing/whitespace → same id.
        let s2 = mk_sin(&id1, "SECRET AT LINE 1  ", "secret", Some(1), "hash1");
        let per_file2 = vec![("src/a.rs".to_string(), "hash1".to_string(), vec![s2])];
        upsert_scan_results(root, &per_file2).unwrap();

        let all = load_all_sins(root);
        assert_eq!(all.len(), 1, "same id should dedup, not duplicate");
        assert_eq!(all[0].id, id1);

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: dispose on non-existent sin returns false
    // =========================================================================

    #[test]
    fn dispose_non_existent_returns_false() {
        let dir = unique_temp_root("dispose-missing");
        let root = dir.as_path();

        let found = dispose(root, None, "nonexistent-id", SDisposition::Ignored).unwrap();
        assert!(!found);

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: Ignored → Open transition (un-ignore) works
    // =========================================================================

    #[test]
    fn unignore_transitions_to_open() {
        let dir = unique_temp_root("unignore");
        let root = dir.as_path();

        let s1 = mk_sin_det("id-unignore", "hash1");
        let per_file = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![s1])];
        upsert_scan_results(root, &per_file).unwrap();

        // Ignore it.
        dispose(root, None, "id-unignore", SDisposition::Ignored).unwrap();
        let all = load_all_sins(root);
        assert_eq!(
            all.iter().find(|s| s.id == "id-unignore").unwrap().disposition,
            SDisposition::Ignored
        );

        // Un-ignore: set back to Open.
        let found = dispose(root, None, "id-unignore", SDisposition::Open).unwrap();
        assert!(found);
        let all = load_all_sins(root);
        assert_eq!(
            all.iter().find(|s| s.id == "id-unignore").unwrap().disposition,
            SDisposition::Open
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: shard_path normalization (backslash → slash, collapse)
    // =========================================================================

    #[test]
    fn shard_path_normalizes_backslash_and_collapses_slashes() {
        let root = Path::new("/proj");
        let a = shard_path(root, "src\\main.rs").unwrap();
        let b = shard_path(root, "src//main.rs").unwrap();
        let c = shard_path(root, "src/main.rs").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn shard_path_rejects_bad_rel_paths() {
        let root = Path::new("/proj");
        assert!(shard_path(root, "../escape").is_err());
        assert!(shard_path(root, "/abs").is_err());
        assert!(shard_path(root, "-leading.rs").is_err());
    }

    // =========================================================================
    // Additional: project shard (_project.json) for empty rel_path
    // =========================================================================

    #[test]
    fn project_shard_for_empty_rel_path() {
        let root = Path::new("/proj");
        let p = shard_path(root, "").unwrap();
        assert!(p.ends_with("_project.json"));
    }

    // =========================================================================
    // Additional: fixed pruned after 30 days
    // =========================================================================

    #[test]
    fn old_fixed_sins_are_pruned() {
        let dir = unique_temp_root("prune");
        let root = dir.as_path();

        // The public API stamps `updated_at` itself (ledger-owned timestamps), so
        // an old Fixed record cannot be injected through `upsert_scan_results`.
        // Seed the on-disk shard DIRECTLY to simulate a ledger that has carried a
        // Fixed sin for years, then upsert and assert the prune-on-write fired.
        let mut old_sin = mk_sin_det("id-old-fixed", "hash1");
        old_sin.updated_at = "2020-01-01T00:00:00+00:00".to_string();
        old_sin.disposition = SDisposition::Fixed;
        let shard = PolisSinShard {
            rel_path: "src/test.rs".to_string(),
            content_hash: "hash1".to_string(),
            updated_at: "2020-01-01T00:00:00+00:00".to_string(),
            sins: vec![old_sin],
        };
        let path = shard_path(root, "src/test.rs").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string(&shard).unwrap()).unwrap();

        // Upsert a fresh sin at the same hash: the stale Fixed record (2020) is
        // older than the prune window and must be dropped on write.
        let new_sin = mk_sin_det("id-new", "hash1");
        let per_file2 = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![new_sin])];
        upsert_scan_results(root, &per_file2).unwrap();

        let all = load_all_sins(root);
        assert_eq!(all.len(), 1, "old Fixed sin should be pruned");
        assert_eq!(all[0].id, "id-new");
        assert_eq!(all[0].disposition, SDisposition::Open, "fresh sin enters Open");

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: empty sins directory → empty load
    // =========================================================================

    #[test]
    fn load_from_empty_dir_returns_empty() {
        let dir = unique_temp_root("empty-load");
        let root = dir.as_path();

        // Don't create any shards — load should return empty without errors.
        let open = load_open_sins(root);
        assert!(open.is_empty());
        let all = load_all_sins(root);
        assert!(all.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: dispose with rel_path_hint goes straight to the shard
    // =========================================================================

    #[test]
    fn dispose_with_hint_path_targets_correct_shard() {
        let dir = unique_temp_root("dispose-hint");
        let root = dir.as_path();

        // Write two sins into two different files.
        let s1 = mk_sin_det("id-hint-a", "hash1");
        let s2 = mk_sin_det("id-hint-b", "hash1");
        upsert_scan_results(
            root,
            &[
                ("src/a.rs".to_string(), "hash1".to_string(), vec![s1]),
                ("src/b.rs".to_string(), "hash1".to_string(), vec![s2]),
            ],
        )
        .unwrap();

        // Hint the correct file → found.
        let found = dispose(root, Some("src/a.rs"), "id-hint-a", SDisposition::Ignored).unwrap();
        assert!(found);

        // Hint the WRONG file → not found there → Ok(false), not Err.
        let not_found =
            dispose(root, Some("src/b.rs"), "id-hint-a", SDisposition::Ignored).unwrap();
        assert!(!not_found);

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: corrupt shard + dispose of a sin that lived there → Err
    // =========================================================================

    #[test]
    fn dispose_corrupt_shard_errors_when_sin_not_found() {
        let dir = unique_temp_root("dispose-corrupt");
        let root = dir.as_path();

        // Write a healthy shard for a different file.
        let s1 = mk_sin_det("id-other", "hash1");
        upsert_scan_results(
            root,
            &[("src/other.rs".to_string(), "hash1".to_string(), vec![s1])],
        )
        .unwrap();

        // Write a corrupt shard that would have contained the sin.
        let corrupt_path = shard_path(root, "src/corrupt.rs").unwrap();
        fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
        fs::write(&corrupt_path, "{ not valid json !!! ").unwrap();

        // dispose of a sin that could reasonably live in the corrupt shard.
        let result = dispose(root, None, "id-missing", SDisposition::Ignored);
        assert!(result.is_err(), "corrupt shards must produce Err, not Ok(false)");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("corrupt"),
            "error must mention corrupt shards, got: {msg}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: fresh sins with duplicate ids are deduped (last-wins)
    // =========================================================================

    #[test]
    fn merge_sins_dedups_duplicate_ids_last_wins() {
        let dir = unique_temp_root("dedup");
        let root = dir.as_path();

        let mut s1 = mk_sin_det("id-dup", "hash1");
        s1.evidence = "first evidence".to_string();
        let mut s2 = mk_sin_det("id-dup", "hash1");
        s2.evidence = "last evidence".to_string();

        let per_file = vec![("src/test.rs".to_string(), "hash1".to_string(), vec![s1, s2])];
        upsert_scan_results(root, &per_file).unwrap();

        let all = load_all_sins(root);
        assert_eq!(all.len(), 1, "duplicate ids must collapse to one record");
        assert_eq!(
            all[0].evidence, "last evidence",
            "last-wins dedup: the second sin's evidence must survive"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // =========================================================================
    // Additional: sweep_orphans removes deleted-file shards, keeps live ones
    // =========================================================================

    #[test]
    fn sweep_orphans_removes_deleted_keeps_live_and_project() {
        let dir = unique_temp_root("sweep");
        let root = dir.as_path();

        let live = mk_sin_det("id-live", "hash1");
        upsert_scan_results(
            root,
            &[("src/live.rs".to_string(), "hash1".to_string(), vec![live])],
        )
        .unwrap();

        let mut deleted_sin = mk_sin_det("id-gone", "hash1");
        deleted_sin.rel_path = "src/deleted.rs".to_string();
        let shard = PolisSinShard {
            rel_path: "src/deleted.rs".to_string(),
            content_hash: "hash1".to_string(),
            updated_at: "t1".to_string(),
            sins: vec![deleted_sin],
        };
        let orphan_path = shard_path(root, "src/deleted.rs").unwrap();
        fs::create_dir_all(orphan_path.parent().unwrap()).unwrap();
        fs::write(&orphan_path, serde_json::to_string(&shard).unwrap()).unwrap();

        let project_path = shard_path(root, "").unwrap();
        fs::write(&project_path, "{}").unwrap();

        let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
        known.insert("src/live.rs".to_string());

        sweep_orphans(root, &known).unwrap();

        assert!(shard_path(root, "src/live.rs").unwrap().exists());
        assert!(!orphan_path.exists());
        assert!(project_path.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_shard_true_false() {
        let dir = unique_temp_root("has-shard");
        let root = dir.as_path();

        assert!(!has_shard(root, "src/none.rs"));

        let s1 = mk_sin_det("id-1", "hash1");
        upsert_scan_results(
            root,
            &[("src/real.rs".to_string(), "hash1".to_string(), vec![s1])],
        )
        .unwrap();

        assert!(has_shard(root, "src/real.rs"));
        assert!(!has_shard(root, "src/still-none.rs"));

        let _ = fs::remove_dir_all(&dir);
    }
}
