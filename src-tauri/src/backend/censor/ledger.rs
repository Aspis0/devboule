//! Censor ledger file IO + pure supersede/staleness logic.
//!
//! Storage shape (decided in the plan): a `.aspis-censor/` directory of per-file
//! shards under the WATCHED PROJECT ROOT, NOT a single ledger file. Each shard is
//! `<root>/.aspis-censor/<sha256(fileRelPath)>.json`; each settled file rewrites
//! only its own shard under its own `<shard>.lock` sidecar, so there is no
//! whole-ledger lock contention.
//!
//! The lock + atomic-write mechanism is COPIED from the agent/project ledgers
//! (`agents.rs` `agent_state_file_lock`, `projects.rs` `project_file_lock_spin`,
//! `fs_replace::replace_file_with_backup`) so it interoperates with the Python
//! MCP writer (msvcrt/fcntl on the same `<file>.lock` sidecar) exactly as the
//! agent state file does. This module does NOT invent a new mechanism.
//!
//! This whole public IO surface is exercised by the in-module tests but has no
//! non-test caller yet — the A2 runners (write findings) and A3 orchestrator
//! (read/supersede on file settle) are its first production callers. The transient
//! dead-code is annotated per-item (NOT a module-wide `allow`) so genuinely dead
//! code added later is still flagged; the logic is fully tested now.

use super::schema::{CensorShard, Finding};
use super::CENSOR_DIR;
use crate::backend::fs_replace::{replace_file_with_backup, replace_file_with_backup_with_fallback};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

/// Spin-lock parameters mirror the agent state lock (`agents.rs`): up to 100
/// attempts, 50ms apart (~5s ceiling) before giving up.
const LOCK_ATTEMPTS: u32 = 100;
const LOCK_SPIN_INTERVAL: Duration = Duration::from_millis(50);

/// Maximum provenance entries kept on a finding (BLOCKER 1 — shard bloat / DoS).
/// A repeated/idempotent dispose must not grow a shard unbounded; past this cap
/// the OLDEST entries are dropped. MIRROR: `CENSOR_PROVENANCE_MAX` in
/// oracle/server/aspis_mcp.py.
const CENSOR_PROVENANCE_MAX: usize = 50;

/// Append a provenance entry with the two BLOCKER-1 guards (mirror of the Python
/// `_append_provenance`):
///   - DEDUP: if the last entry has the same `(actor, action)`, skip the append
///     (an idempotent re-dispose must not grow the trail);
///   - CAP: keep at most `CENSOR_PROVENANCE_MAX`, dropping the OLDEST first.
fn push_provenance(
    provenance: &mut Vec<super::schema::ProvenanceEntry>,
    entry: super::schema::ProvenanceEntry,
) {
    if let Some(last) = provenance.last() {
        if last.actor == entry.actor && last.action == entry.action {
            return;
        }
    }
    provenance.push(entry);
    if provenance.len() > CENSOR_PROVENANCE_MAX {
        let overflow = provenance.len() - CENSOR_PROVENANCE_MAX;
        provenance.drain(0..overflow);
    }
}

/// Canonical byte-form of a rel path used for the shard HASH: backslashes to `/`,
/// then consecutive slashes collapsed (`//` → `/`). So `src//a.rs`, `src/a.rs`
/// and a Windows `src\a.rs` all hash to the SAME shard (NITPICK 1).
/// BYTE-IDENTICAL to `normalize_censor_rel_path` in oracle/server/aspis_mcp.py —
/// the two writers MUST produce the same sha256 for one file.
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

/// Per-process monotonic counter for unique temp/backup file names. Combined with
/// `process::id()` this is collision-free even when two writes land in the same
/// nanosecond — and, unlike `SystemTime`, it cannot go backwards on a clock
/// adjustment. Mirrors the test helper's `AtomicU64` convention.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Directory holding the per-file shards for a project root.
#[allow(dead_code)] // first non-test caller is the A3 watcher ignore-set.
pub fn censor_dir(root: &Path) -> PathBuf {
    root.join(CENSOR_DIR)
}

/// Reject a relative path that could escape the censor dir or break the
/// hash-as-filename contract: absolute paths (including Windows drive-letter /
/// UNC roots) and any `..` parent component are refused. A normal relative path
/// with `.` segments is fine. Called at the top of every shard-path / IO entry so
/// neither a malicious nor a malformed `file_rel_path` (from the MCP boundary)
/// can target a shard outside `.aspis-censor/`.
///
/// MINOR H: validate on the SLASH-NORMALIZED form. `Path::components()` only
/// recognizes `\` as a separator on Windows, so on Linux/macOS a hostile
/// `a\..\secret` would be a single `Normal` component and its `..` would slip past
/// the `ParentDir` check. Replacing `\` with `/` before parsing makes the
/// traversal/absolute checks host-OS-independent (this mirrors the Python
/// `validate_censor_rel_path`, which splits on `[\\/]+`). The original `rel` is
/// still used in error messages for diagnostics.
pub fn validate_rel_path(rel: &str) -> io::Result<()> {
    let normalized = rel.replace('\\', "/");
    // Windows drive paths (`C:\...`, `C:/...`, drive-relative `C:foo`) must be
    // rejected on EVERY host OS, not just Windows: ledger rel paths are
    // cross-platform, and on POSIX `C:` parses as a plain `Normal` component so
    // the `Prefix` arm below never fires for them.
    let mut head = normalized.bytes();
    if let (Some(first), Some(b':')) = (head.next(), head.next()) {
        if first.is_ascii_alphabetic() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("censor rel path must be relative, got absolute: {rel}"),
            ));
        }
    }
    let path = Path::new(&normalized);
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("censor rel path must not contain '..': {rel}"),
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("censor rel path must be relative, got absolute: {rel}"),
                ));
            }
            Component::Normal(name) => {
                // ARGV-INJECTION GUARD: a path component starting with '-' would be
                // read as a CLI flag by a linter we hand the path to (eslint/ruff/
                // bandit/semgrep/lizard/vulture). The runners also pass `--` before
                // the positional path, but we reject `-`-leading components here too
                // (defense in depth): such a name cannot describe a real source file
                // we want to track. A backslash-separated Windows path (`a\-b.rs`)
                // is a single `Normal` component here, so check each `/`-split piece.
                let name = name.to_string_lossy();
                for piece in name.split(['/', '\\']) {
                    if piece.starts_with('-') {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("censor rel path component must not start with '-': {rel}"),
                        ));
                    }
                }
            }
            // CurDir is fine.
            _ => {}
        }
    }
    Ok(())
}

/// Shard path for one file: `<root>/.aspis-censor/<sha256(normalizedRelPath)>.json`.
///
/// The relative path is hashed (not used verbatim) so the shard filename is flat,
/// fixed-length, and free of path separators / illegal filename chars on every
/// platform. Backslashes are normalized to `/` BEFORE hashing so `a\b.rs`
/// (Windows) and `a/b.rs` (the Python MCP writer) map to the SAME shard — without
/// this the two writers would maintain two divergent shards for one file. The
/// caller's path is validated first (`validate_rel_path`); on violation the path
/// hashes to a deterministic but isolated value AND the IO entry points reject it.
pub fn shard_path(root: &Path, file_rel_path: &str) -> io::Result<PathBuf> {
    validate_rel_path(file_rel_path)?;
    let normalized = normalize_rel_path(file_rel_path);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let name = hex::encode(hasher.finalize());
    Ok(censor_dir(root).join(format!("{name}.json")))
}

/// RAII lock on a shard's `<shard>.lock` sidecar.
struct ShardLock {
    _file: File,
}

/// Acquire the exclusive lock for a shard via the fs2 spin pattern (same shape
/// as `agent_state_file_lock`). The lock lives on a separate `.lock` sidecar so
/// it never collides with the atomic replace of the shard itself. Creates the
/// `.aspis-censor/` directory if needed.
fn lock_shard(shard: &Path) -> io::Result<ShardLock> {
    if let Some(parent) = shard.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = shard.with_extension("json.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)?;
    for attempt in 0..LOCK_ATTEMPTS {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(ShardLock { _file: file }),
            Err(_) if attempt + 1 < LOCK_ATTEMPTS => thread::sleep(LOCK_SPIN_INTERVAL),
            // Final attempt failed: surface the real OS error, but wrap it with
            // the shard path for context (the raw fs2 error carries none). There
            // is no unreachable post-loop branch — the last iteration returns here.
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!(
                        "could not acquire censor shard lock: {}: {e}",
                        lock_path.display()
                    ),
                ));
            }
        }
    }
    // `LOCK_ATTEMPTS` is a non-zero const, so the loop always returns; this is
    // unreachable but keeps the fn total without a dead error literal.
    unreachable!("lock_shard loop runs at least once for LOCK_ATTEMPTS > 0")
}

/// Read a shard by path. `Ok(None)` ONLY for a genuinely-absent file (NotFound).
///
/// A file that EXISTS but is unparseable is a CORRUPT shard, returned as `Err`
/// (InvalidData) — NOT `Ok(None)`. Collapsing corruption to "absent" was a
/// data-loss bug: the caller would then write only the new findings, destroying
/// the prior dispositions/provenance it could not read. Surfacing the error lets
/// the write path abort and leave the unreadable file untouched. Tolerance for a
/// *missing* shard is preserved (`Ok(None)`); a non-NotFound IO fault is `Err`.
///
/// The error message carries the shard PATH only — never the file contents, which
/// could embed a leaked secret value (privacy: shard bodies are English summaries
/// but a hand-corrupted file could contain anything).
fn read_shard_at(path: &Path) -> io::Result<Option<CensorShard>> {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<CensorShard>(&content) {
            Ok(shard) => Ok(Some(shard)),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "corrupt censor shard (unparseable JSON): {}",
                    path.display()
                ),
            )),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read a shard for read-only queries (no lock). Returns `Ok(None)` only when the
/// shard is absent; a corrupt shard is `Err` (see `read_shard_at`). The write path
/// does NOT use this — it reads under the lock via `read_supersede_write_shard`.
#[allow(dead_code)] // first non-test caller is the A3 read/query path.
pub fn read_shard(root: &Path, file_rel_path: &str) -> io::Result<Option<CensorShard>> {
    let path = shard_path(root, file_rel_path)?;
    read_shard_at(&path)
}

/// Enumerate ALL shards in the project's `.aspis-censor/` dir (no lock; read-only
/// query for the board/panel). A MISSING dir → empty (the project was never
/// reviewed). A corrupt/unreadable individual shard is SKIPPED (never aborts the
/// whole listing) — a single hand-broken file must not blank the panel. Only
/// `.json` files are considered (the `.lock`/`.tmp`/`.bak` sidecars are ignored).
pub fn list_shards(root: &Path) -> io::Result<Vec<CensorShard>> {
    let dir = censor_dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue; // skip .lock/.tmp/.bak sidecars and non-shard files
        }
        // A corrupt single shard is skipped (best-effort listing), not fatal.
        if let Ok(Some(shard)) = read_shard_at(&path) {
            out.push(shard);
        }
    }
    Ok(out)
}

/// Atomic shard write assuming the shard lock is ALREADY held by the caller.
///
/// Serializes pretty JSON to a unique temp file (`process::id()` + a per-process
/// atomic counter — collision-free and clock-independent, unlike `SystemTime`),
/// then atomically replaces the target via `replace_file_with_backup` (MoveFileEx
/// on Windows / rename on unix). On a write failure before the rename the temp
/// file is removed so a failed write never leaves an orphan `.tmp` behind.
///
/// Split out from the locked region so `read_supersede_write_shard` can read +
/// supersede + write under ONE lock acquisition (TOCTOU-free), mirroring how
/// `agents.rs::record_agent_entry` reads under the lock it holds.
fn write_shard_locked(path: &Path, shard: &CensorShard) -> io::Result<()> {
    let content = serde_json::to_string_pretty(shard)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let stamp = format!(
        "{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let temp_path = path.with_extension(format!("json.{stamp}.tmp"));
    let backup_path = path.with_extension(format!("json.{stamp}.bak"));

    if let Err(e) = fs::write(&temp_path, content) {
        // Clean up a partially-written temp before bubbling the error.
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }
    replace_file_with_backup_with_fallback(&temp_path, path, &backup_path, "censor shard")
        .map_err(io::Error::other)
}

/// Write a shard atomically under its own lock. Creates `.aspis-censor/` if
/// needed, acquires the `<shard>.lock`, then writes via `write_shard_locked`.
///
/// This is the LOCK-then-WRITE path for callers that already hold the merged
/// shard. Callers that must read-modify-write MUST use
/// `read_supersede_write_shard` instead so the read happens under the same lock.
#[allow(dead_code)] // first non-test caller is the A2/A3 direct-write path.
pub fn write_shard(root: &Path, shard: &CensorShard) -> io::Result<()> {
    let path = shard_path(root, &shard.file_rel_path)?;
    let _guard = lock_shard(&path)?;
    write_shard_locked(&path, shard)
}

/// TOCTOU-free read-modify-write of one shard under a SINGLE lock acquisition.
///
/// Acquires the shard lock ONCE, reads the existing shard while holding it, calls
/// `supersede` to merge `new_findings` (preserving prior human dispositions at the
/// same hash), writes the merged shard, and returns it — all before releasing the
/// lock. This is the ONLY correct write path for a review pass: the 3-call
/// read→supersede→write_shard sequence let the Python MCP writer or a concurrent
/// Rust pass clobber disposition changes between the read and the write.
///
/// If the existing shard is CORRUPT (unparseable), the write is ABORTED (the error
/// is returned) rather than overwriting it — never destroy unreadable prior data.
/// Only the shard path is logged on corruption, never the contents.
#[allow(dead_code)] // first non-test caller is the A3 orchestrator (file settle).
pub fn read_supersede_write_shard(
    root: &Path,
    new_findings: Vec<Finding>,
    new_hash: &str,
    refreshed_sources: &std::collections::BTreeSet<String>,
    file_rel_path: &str,
    now: &str,
) -> io::Result<CensorShard> {
    let path = shard_path(root, file_rel_path)?;
    let _guard = lock_shard(&path)?;
    // Read under the held lock. A corrupt existing shard aborts the write so we
    // never clobber prior dispositions/provenance we could not parse.
    let existing = match read_shard_at(&path) {
        Ok(existing) => existing,
        Err(e) => {
            eprintln!(
                "censor: refusing to overwrite unreadable shard: {}",
                path.display()
            );
            return Err(e);
        }
    };
    // SOURCE-SCOPED merge: only the sources this pass actually ran are refreshed;
    // findings from OTHER sources (e.g. coarse clippy findings when this is a fine
    // eslint pass) survive untouched (same-hash). This is what prevents a per-pass
    // write from clobbering the other trigger's findings for the same file.
    let merged = supersede_sources(
        existing,
        new_findings,
        new_hash,
        refreshed_sources,
        file_rel_path,
        now,
    );
    write_shard_locked(&path, &merged)?;
    Ok(merged)
}

/// Set a finding's disposition + append a provenance entry, under the shard's lock
/// (TOCTOU-free: read→modify→write in one lock acquisition, mirroring
/// `read_supersede_write_shard`). The finding is located by `id` within the file's
/// shard; an absent shard or an absent id is an error (the caller passes the file
/// the finding belongs to). A corrupt shard aborts (never overwritten).
///
/// `actor` is the disposing principal (a project/agent id); `now` the audit stamp.
/// The disposition becomes `new_disposition` and a `{actor, action, at}` entry is
/// APPENDED to provenance (history is never rewritten). This is the write path the
/// `censor_dispose_finding` command and (later) the Python MCP `censor_dispose`
/// tool both go through, so a disposition cannot be clobbered by a concurrent
/// review pass between read and write.
pub fn dispose_finding(
    root: &Path,
    file_rel_path: &str,
    id: &str,
    new_disposition: super::schema::Disposition,
    actor: &str,
    now: &str,
) -> io::Result<()> {
    use super::schema::ProvenanceEntry;
    let path = shard_path(root, file_rel_path)?;
    let _guard = lock_shard(&path)?;
    let mut shard = match read_shard_at(&path) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no censor shard for file: {file_rel_path}"),
            ));
        }
        Err(e) => {
            eprintln!(
                "censor: refusing to dispose against unreadable shard: {}",
                path.display()
            );
            return Err(e);
        }
    };
    let finding = shard.findings.iter_mut().find(|f| f.id == id);
    let finding = match finding {
        Some(f) => f,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no censor finding with id {id} in {file_rel_path}"),
            ));
        }
    };
    finding.disposition = new_disposition;
    // BLOCKER 1 — bounded, dedup'd append. This UI/local path has no agent role
    // (the actor is the trusted project id), so `role` is "" — the WARNING 2
    // coder-vs-verifier precedence is enforced only on the Python MCP agent path.
    push_provenance(
        &mut finding.provenance,
        ProvenanceEntry {
            actor: actor.to_string(),
            action: disposition_action(new_disposition).to_string(),
            role: String::new(),
            at: now.to_string(),
        },
    );
    shard.updated_at = now.to_string();
    write_shard_locked(&path, &shard)
}

/// The provenance `action` string for a disposition (the audit verb).
fn disposition_action(d: super::schema::Disposition) -> &'static str {
    use super::schema::Disposition;
    match d {
        Disposition::Open => "reopen",
        Disposition::Fixed => "fixed",
        Disposition::Fp => "fp",
        Disposition::Wontfix => "wontfix",
    }
}

/// Is this finding stale relative to the file's current content hash? A finding
/// whose `content_hash` no longer matches the current file content describes code
/// that has since changed and should be dropped on the next supersede.
#[allow(dead_code)] // first non-test caller is the A3 staleness sweep.
pub fn is_stale(finding: &Finding, current_hash: &str) -> bool {
    finding.content_hash != current_hash
}

/// Pure merge of a fresh review pass into an existing shard.
///
/// Policy:
///   1. Drop every OLD finding whose `content_hash != new_hash` (it described a
///      version of the file that no longer exists) — REGARDLESS of disposition.
///   2. Dedup `new_findings` by `id` (LAST-WINS: if a pass emits the same id
///      twice, the later entry replaces the earlier; the merged shard never
///      carries duplicate ids).
///   3. Merge in the deduped new findings.
///   4. For any new finding whose `id` matches a SURVIVING old finding (same id at
///      the same content hash), PRESERVE the old finding's `disposition` and
///      `provenance`. A coder's "fp" mark (or any disposition + its audit trail)
///      is NOT blown away just because a re-review re-detected the same id.
///   5. A surviving old finding (same hash) that is NOT re-emitted this pass and
///      carries a HUMAN disposition (`disposition != Open`, i.e. Fixed/Fp/Wontfix)
///      is KEPT — it is a judged finding / audit trail, not noise. Only old
///      findings still `Open` (machine-default) and not re-emitted are dropped as
///      resolved-by-absence. Stale (hash-changed) findings are always dropped
///      (step 1), even if judged.
///
/// The result's `content_hash`/`updated_at`/`file_rel_path` are set to the new
/// values; every retained finding's `content_hash` is stamped to `new_hash`.
///
/// This is the WHOLE-FILE variant (every source is considered refreshed): it is
/// the right merge when a single pass produced the complete finding set for a
/// file. The SOURCE-SCOPED variant ([`supersede_sources`]) is used by the live
/// orchestrator, where a pass refreshes only SOME sources for a file and the rest
/// must survive.
///
/// Retained as the public whole-file merge primitive (exercised by the A1 tests
/// and a natural fit for any caller that DOES produce a file's complete finding
/// set in one pass). The live orchestrator uses the source-scoped variant via
/// `read_supersede_write_shard`.
#[allow(dead_code)]
pub fn supersede(
    old: Option<CensorShard>,
    new_findings: Vec<Finding>,
    new_hash: &str,
    file_rel_path: &str,
    now: &str,
) -> CensorShard {
    // Whole-file merge = scoped merge with NO source restriction (`None`).
    supersede_inner(old, new_findings, new_hash, None, file_rel_path, now)
}

/// SOURCE-SCOPED merge of a fresh review pass into an existing shard.
///
/// A file's shard accumulates findings from MULTIPLE runner sources that arrive on
/// DIFFERENT triggers (FINE per-file eslint/ruff/... vs COARSE project-level
/// clippy/tsc/gitleaks/...). A pass refreshes only the sources in
/// `refreshed_sources`; the merge must therefore:
///
///   - if the file content HASH CHANGED (old shard hash != `new_hash`): all old
///     findings describe a version of the file that no longer exists → DROP them
///     all (full re-review), keep only the new findings;
///   - if the hash is the SAME:
///       * KEEP every old finding whose `source` is NOT in `refreshed_sources`
///         (those sources were not re-run; their findings still stand) —
///         REGARDLESS of disposition;
///       * for old findings whose `source` IS in `refreshed_sources`, apply the
///         same per-id reconciliation as [`supersede`]: a re-emitted id keeps its
///         prior disposition+provenance; a judged (non-Open) survivor not
///         re-emitted is KEPT; an Open survivor not re-emitted is DROPPED
///         (resolved-by-absence).
///
/// New findings are deduped by id (last-wins) and stamped to `new_hash`. This is
/// the merge `read_supersede_write_shard` uses; the orchestrator passes the exact
/// source set it just produced for the file.
pub fn supersede_sources(
    old: Option<CensorShard>,
    new_findings: Vec<Finding>,
    new_hash: &str,
    refreshed_sources: &std::collections::BTreeSet<String>,
    file_rel_path: &str,
    now: &str,
) -> CensorShard {
    supersede_inner(
        old,
        new_findings,
        new_hash,
        Some(refreshed_sources),
        file_rel_path,
        now,
    )
}

/// Shared implementation behind [`supersede`] (`refreshed = None` → all sources
/// refreshed) and [`supersede_sources`] (`refreshed = Some(set)` → only those
/// sources refreshed; findings from other sources survive at the same hash).
fn supersede_inner(
    old: Option<CensorShard>,
    new_findings: Vec<Finding>,
    new_hash: &str,
    refreshed: Option<&std::collections::BTreeSet<String>>,
    file_rel_path: &str,
    now: &str,
) -> CensorShard {
    use super::schema::Disposition;

    // `true` if `source` is being refreshed by this pass (so its old findings are
    // subject to per-id reconciliation). When `refreshed` is `None` EVERY source
    // is refreshed (whole-file merge), reproducing the original `supersede`.
    let is_refreshed = |source: &str| refreshed.is_none_or(|set| set.contains(source));

    // Partition old findings AT THE SAME HASH into:
    //   - `kept_other`: source NOT being refreshed → survives verbatim;
    //   - `survivors`:  source IS being refreshed → eligible for per-id reconcile.
    // Hash-changed findings are dropped entirely (full re-review of stale code).
    let mut kept_other: Vec<Finding> = Vec::new();
    let mut survivors: HashMap<String, Finding> = HashMap::new();
    if let Some(old_shard) = old {
        for f in old_shard.findings.into_iter() {
            if f.content_hash != new_hash {
                continue; // stale: code changed, drop regardless of source/disposition
            }
            if is_refreshed(&f.source) {
                survivors.insert(f.id.clone(), f);
            } else {
                kept_other.push(f);
            }
        }
    }

    // Track which surviving ids the new pass re-emitted, so leftover judged
    // survivors (not re-emitted) can be appended afterwards.
    let mut emitted_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Dedup new findings by id (last-wins) while preserving first-seen order.
    let mut order: Vec<String> = Vec::with_capacity(new_findings.len());
    let mut deduped: HashMap<String, Finding> = HashMap::new();
    for mut f in new_findings.into_iter() {
        f.content_hash = new_hash.to_string();
        if let Some(prev) = survivors.get(&f.id) {
            // Preserve the coder/reviewer-set lifecycle from the prior pass.
            f.disposition = prev.disposition;
            f.provenance = prev.provenance.clone();
        }
        if !deduped.contains_key(&f.id) {
            order.push(f.id.clone());
        }
        deduped.insert(f.id.clone(), f);
    }

    let mut merged: Vec<Finding> =
        Vec::with_capacity(order.len() + survivors.len() + kept_other.len());
    // 1) Findings from sources NOT refreshed this pass survive verbatim.
    merged.append(&mut kept_other);
    // 2) The deduped new findings, in first-seen order.
    for id in order {
        if let Some(f) = deduped.remove(&id) {
            emitted_ids.insert(id);
            merged.push(f);
        }
    }
    // 3) Judged survivors (refreshed source, same hash) the pass did not re-emit.
    for (id, f) in survivors.into_iter() {
        if !emitted_ids.contains(&id) && f.disposition != Disposition::Open {
            merged.push(f);
        }
    }

    // DEFENSIVE FINAL DEDUP-BY-ID (belt-and-braces). The three buckets above are
    // disjoint by id BY CONSTRUCTION: `compute_id` mixes the `source` (schema.rs),
    // so `kept_other` (sources NOT refreshed) and the new findings (sources IN
    // refreshed) can never share an id, and the survivors loop skips re-emitted
    // ids. A real cross-bucket id collision is therefore IMPOSSIBLE today. We
    // nonetheless drop any duplicate id from the merged set, FIRST-OCCURRENCE WINS
    // (kept_other → new → judged-survivors precedence), so any FUTURE drift (e.g. a
    // change to `compute_id` inputs) cannot persist duplicate ids — which would
    // break `dispose_finding`'s first-match lookup and collide React list keys.
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    merged.retain(|f| seen_ids.insert(f.id.clone()));

    CensorShard {
        file_rel_path: file_rel_path.to_string(),
        content_hash: new_hash.to_string(),
        updated_at: now.to_string(),
        findings: merged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{
        Category, Disposition, ProvenanceEntry, Severity, Verdict,
    };

    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique temp dir per test (process id + a monotonic counter), matching the
    /// codebase's `std::env::temp_dir()` convention (no `tempfile` crate dep).
    /// Caller is responsible for cleanup.
    fn unique_temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aspis-censor-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn finding(id: &str, hash: &str, disposition: Disposition) -> Finding {
        finding_src(id, hash, disposition, "clippy")
    }

    /// Build a finding with an explicit `source` (for the source-scoped tests).
    fn finding_src(id: &str, hash: &str, disposition: Disposition, source: &str) -> Finding {
        Finding {
            id: id.into(),
            file: "src/a.rs".into(),
            content_hash: hash.into(),
            line: Some(1),
            severity: Severity::Medium,
            category: Category::Correctness,
            source: source.into(),
            title: "t".into(),
            body: "b".into(),
            verdict: Verdict::Suspected,
            disposition,
            provenance: Vec::new(),
            created_at: "t0".into(),
            commit: None,
        }
    }

    /// A source set from a slice of names (for `supersede_sources` / rsw tests).
    fn srcs(list: &[&str]) -> std::collections::BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn shard_path_is_flat_hashed_json_in_censor_dir() {
        let root = Path::new("/proj");
        let p = shard_path(root, "src/main.rs").unwrap();
        assert_eq!(p.parent().unwrap(), censor_dir(root));
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.ends_with(".json"));
        // 64 hex chars + ".json".
        assert_eq!(name.len(), 64 + 5);
        // Different rel paths → different shard files.
        assert_ne!(
            shard_path(root, "a").unwrap(),
            shard_path(root, "b").unwrap()
        );
    }

    #[test]
    fn shard_path_normalizes_backslash_to_slash() {
        // Windows `a\b.rs` and POSIX/Python `a/b.rs` MUST map to one shard, else
        // the two writers maintain divergent shards for the same file.
        let root = Path::new("/proj");
        assert_eq!(
            shard_path(root, "a\\b.rs").unwrap(),
            shard_path(root, "a/b.rs").unwrap()
        );
    }

    #[test]
    fn validate_rel_path_rejects_traversal_and_absolute() {
        assert!(validate_rel_path("../x").is_err());
        assert!(validate_rel_path("a/../b").is_err());
        assert!(validate_rel_path("/abs").is_err());
        assert!(validate_rel_path("C:\\abs").is_err());
        // Normal relative paths (incl. a `.` segment) are accepted.
        assert!(validate_rel_path("src/a.rs").is_ok());
        assert!(validate_rel_path("./src/a.rs").is_ok());
        assert!(validate_rel_path("a\\b.rs").is_ok());
    }

    #[test]
    fn validate_rel_path_rejects_backslash_traversal_on_any_os() {
        // MINOR H: a backslash-separated `..` must be rejected REGARDLESS of host OS.
        // On non-Windows `Path::components()` would treat `a\..\secret` as one Normal
        // component and miss the `..`; slash-normalizing first catches it everywhere.
        assert!(validate_rel_path("a\\..\\secret").is_err());
        assert!(validate_rel_path("..\\secret").is_err());
        assert!(validate_rel_path("src\\..\\..\\etc\\passwd").is_err());
    }

    #[test]
    fn validate_rel_path_rejects_dash_leading_component() {
        // ARGV-injection guard: a `-`-leading component would be read as a CLI flag
        // by a linter, so it is refused (in any position, with `/` or `\` seps).
        assert_eq!(
            validate_rel_path("--config=evil").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            validate_rel_path("src/--rm-rf").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            validate_rel_path("src/-leading.py").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            validate_rel_path("a\\-b.rs").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        // A hyphen INSIDE a component is fine (kebab-case files).
        assert!(validate_rel_path("src/my-file.rs").is_ok());
    }

    #[test]
    fn shard_path_rejects_bad_rel_path() {
        let root = Path::new("/proj");
        assert!(shard_path(root, "../escape").is_err());
        assert!(shard_path(root, "/abs").is_err());
    }

    #[test]
    fn is_stale_true_false() {
        let f = finding("id1", "h1", Disposition::Open);
        assert!(!is_stale(&f, "h1"));
        assert!(is_stale(&f, "h2"));
    }

    #[test]
    fn supersede_drops_stale_hash_mismatch() {
        let old = CensorShard {
            file_rel_path: "src/a.rs".into(),
            content_hash: "old".into(),
            updated_at: "t0".into(),
            findings: vec![finding("stale", "old", Disposition::Open)],
        };
        let new = vec![finding("fresh", "ignored", Disposition::Open)];
        let result = supersede(Some(old), new, "new", "src/a.rs", "t1");
        // Only the fresh finding remains, stamped with the new hash.
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].id, "fresh");
        assert_eq!(result.findings[0].content_hash, "new");
        assert_eq!(result.content_hash, "new");
        assert_eq!(result.updated_at, "t1");
        assert_eq!(result.file_rel_path, "src/a.rs");
    }

    #[test]
    fn supersede_preserves_disposition_and_provenance_for_surviving_id() {
        // A coder marked finding "x" as fp last pass at hash "h", with an audit
        // entry. The re-review re-flags the same id "x" at the same hash "h".
        let mut prev = finding("x", "h", Disposition::Fp);
        prev.provenance = vec![ProvenanceEntry {
            actor: "coder-7f".into(),
            action: "fp".into(),
            role: String::new(),
            at: "t0".into(),
        }];
        let old = CensorShard {
            file_rel_path: "src/a.rs".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            findings: vec![prev],
        };
        // Re-flag arrives as a fresh Open finding with no provenance.
        let new = vec![finding("x", "ignored", Disposition::Open)];
        let result = supersede(Some(old), new, "h", "src/a.rs", "t1");

        assert_eq!(result.findings.len(), 1);
        let f = &result.findings[0];
        assert_eq!(f.id, "x");
        // Disposition + provenance carried over from the prior pass.
        assert_eq!(f.disposition, Disposition::Fp);
        assert_eq!(f.provenance.len(), 1);
        assert_eq!(f.provenance[0].action, "fp");
        assert_eq!(f.content_hash, "h");
    }

    #[test]
    fn supersede_does_not_preserve_across_hash_change() {
        // Same id, but the prior disposition was at a DIFFERENT hash → it does
        // not survive, the new finding stays Open.
        let prev = finding("x", "old", Disposition::Fp);
        let old = CensorShard {
            file_rel_path: "src/a.rs".into(),
            content_hash: "old".into(),
            updated_at: "t0".into(),
            findings: vec![prev],
        };
        let new = vec![finding("x", "ignored", Disposition::Open)];
        let result = supersede(Some(old), new, "new", "src/a.rs", "t1");
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].disposition, Disposition::Open);
    }

    #[test]
    fn supersede_with_no_old_shard() {
        let new = vec![finding("a", "x", Disposition::Open)];
        let result = supersede(None, new, "h", "src/a.rs", "t1");
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].content_hash, "h");
    }

    #[test]
    fn supersede_old_open_finding_not_reflagged_is_dropped() {
        // Survivor at same hash, still Open, not in the new pass → resolved, dropped.
        let old = CensorShard {
            file_rel_path: "src/a.rs".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            findings: vec![finding("gone", "h", Disposition::Open)],
        };
        let result = supersede(Some(old), Vec::new(), "h", "src/a.rs", "t1");
        assert!(result.findings.is_empty());
    }

    #[test]
    fn supersede_keeps_judged_survivor_drops_open_drops_stale() {
        // Three old findings at the current pass:
        //   - "wontfix" : judged (Wontfix), same hash, NOT re-emitted → KEPT.
        //   - "open"    : Open, same hash, NOT re-emitted            → DROPPED.
        //   - "stale-fp": judged (Fp) but at a DIFFERENT hash        → DROPPED.
        let old = CensorShard {
            file_rel_path: "src/a.rs".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            findings: vec![
                finding("wontfix", "h", Disposition::Wontfix),
                finding("open", "h", Disposition::Open),
                finding("stale-fp", "old-hash", Disposition::Fp),
            ],
        };
        let result = supersede(Some(old), Vec::new(), "h", "src/a.rs", "t1");
        assert_eq!(result.findings.len(), 1);
        let f = &result.findings[0];
        assert_eq!(f.id, "wontfix");
        assert_eq!(f.disposition, Disposition::Wontfix);
        // The kept survivor is stamped to (already at) the current hash.
        assert_eq!(f.content_hash, "h");
    }

    #[test]
    fn supersede_dedups_new_findings_by_id_last_wins() {
        // Two new findings share an id → merged shard has ONE, the last wins.
        let mut first = finding("dup", "ignored", Disposition::Open);
        first.title = "first".into();
        let mut second = finding("dup", "ignored", Disposition::Open);
        second.title = "second".into();
        let result = supersede(None, vec![first, second], "h", "src/a.rs", "t1");
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].id, "dup");
        assert_eq!(result.findings[0].title, "second");
    }

    #[test]
    fn lock_write_read_round_trip_in_temp_dir() {
        let dir = unique_temp_root("roundtrip");
        let root = dir.as_path();
        let rel = "src/lib.rs";

        // Absent → None.
        assert!(read_shard(root, rel).unwrap().is_none());

        let shard = supersede(
            None,
            vec![finding("a", "h", Disposition::Open)],
            "h",
            rel,
            "t1",
        );
        write_shard(root, &shard).unwrap();

        // Shard file + censor dir exist.
        assert!(censor_dir(root).exists());
        assert!(shard_path(root, rel).unwrap().exists());

        let back = read_shard(root, rel).unwrap().unwrap();
        assert_eq!(back, shard);

        // Overwrite under the lock with a superseded shard.
        let updated = supersede(
            Some(back),
            vec![finding("a", "ignored", Disposition::Open)],
            "h2",
            rel,
            "t2",
        );
        write_shard(root, &updated).unwrap();
        let back2 = read_shard(root, rel).unwrap().unwrap();
        assert_eq!(back2.content_hash, "h2");
        assert_eq!(back2.updated_at, "t2");
        assert_eq!(back2.findings[0].content_hash, "h2");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_shard_absent_is_none() {
        let dir = unique_temp_root("absent");
        let root = dir.as_path();
        assert!(read_shard(root, "src/none.rs").unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_shard_corrupt_is_err_not_none() {
        // A corrupt (present-but-unparseable) shard MUST surface as Err so the
        // write path can abort instead of silently overwriting prior data.
        let dir = unique_temp_root("corrupt");
        let root = dir.as_path();
        let rel = "src/bad.rs";
        let path = shard_path(root, rel).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ this is not valid json ").unwrap();
        let err = read_shard(root, rel).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_supersede_write_round_trip_under_single_lock() {
        let dir = unique_temp_root("rsw");
        let root = dir.as_path();
        let rel = "src/lib.rs";

        // First pass: no existing shard.
        let s1 = read_supersede_write_shard(
            root,
            vec![finding("a", "ignored", Disposition::Open)],
            "h1",
            &srcs(&["clippy"]),
            rel,
            "t1",
        )
        .unwrap();
        assert_eq!(s1.content_hash, "h1");
        assert_eq!(s1.findings.len(), 1);
        assert_eq!(s1.findings[0].content_hash, "h1");

        // The returned shard equals what is on disk.
        let on_disk = read_shard(root, rel).unwrap().unwrap();
        assert_eq!(on_disk, s1);

        // Second pass at a new hash: stale dropped, fresh stamped.
        let s2 = read_supersede_write_shard(
            root,
            vec![finding("b", "ignored", Disposition::Open)],
            "h2",
            &srcs(&["clippy"]),
            rel,
            "t2",
        )
        .unwrap();
        assert_eq!(s2.content_hash, "h2");
        assert_eq!(s2.findings.len(), 1);
        assert_eq!(s2.findings[0].id, "b");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_supersede_write_preserves_disposition_across_passes() {
        let dir = unique_temp_root("rsw-dispo");
        let root = dir.as_path();
        let rel = "src/lib.rs";

        // Seed a shard with a finding the user marked Wontfix at hash "h".
        let mut judged = finding("keep", "h", Disposition::Wontfix);
        judged.provenance = vec![ProvenanceEntry {
            actor: "coder".into(),
            action: "wontfix".into(),
            role: String::new(),
            at: "t0".into(),
        }];
        write_shard(
            root,
            &CensorShard {
                file_rel_path: rel.into(),
                content_hash: "h".into(),
                updated_at: "t0".into(),
                findings: vec![judged],
            },
        )
        .unwrap();

        // Re-review at the SAME hash re-emits "keep" as a fresh Open finding.
        let merged = read_supersede_write_shard(
            root,
            vec![finding("keep", "ignored", Disposition::Open)],
            "h",
            &srcs(&["clippy"]),
            rel,
            "t1",
        )
        .unwrap();
        assert_eq!(merged.findings.len(), 1);
        assert_eq!(merged.findings[0].disposition, Disposition::Wontfix);
        assert_eq!(merged.findings[0].provenance.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_supersede_write_aborts_on_corrupt_existing_shard() {
        // A corrupt existing shard must NOT be overwritten — prior (unreadable)
        // dispositions/provenance are never destroyed.
        let dir = unique_temp_root("rsw-corrupt");
        let root = dir.as_path();
        let rel = "src/bad.rs";
        let path = shard_path(root, rel).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let corrupt = "{ not json ";
        fs::write(&path, corrupt).unwrap();

        let err = read_supersede_write_shard(
            root,
            vec![finding("new", "ignored", Disposition::Open)],
            "h",
            &srcs(&["clippy"]),
            rel,
            "t1",
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // The corrupt file is left exactly as-is (not overwritten).
        assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_supersede_write_rejects_bad_rel_path() {
        let dir = unique_temp_root("rsw-badpath");
        let root = dir.as_path();
        let err = read_supersede_write_shard(root, Vec::new(), "h", &srcs(&[]), "../escape", "t1")
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- dispose_finding: sets disposition + appends provenance ----

    #[test]
    fn dispose_finding_sets_disposition_and_appends_provenance() {
        let dir = unique_temp_root("dispose");
        let root = dir.as_path();
        let rel = "src/a.rs";
        write_shard(
            root,
            &CensorShard {
                file_rel_path: rel.into(),
                content_hash: "h".into(),
                updated_at: "t0".into(),
                findings: vec![finding("f1", "h", Disposition::Open)],
            },
        )
        .unwrap();

        dispose_finding(root, rel, "f1", Disposition::Fp, "proj-7", "t1").unwrap();

        let shard = read_shard(root, rel).unwrap().unwrap();
        assert_eq!(shard.findings.len(), 1);
        let f = &shard.findings[0];
        assert_eq!(f.disposition, Disposition::Fp);
        assert_eq!(f.provenance.len(), 1);
        assert_eq!(f.provenance[0].actor, "proj-7");
        assert_eq!(f.provenance[0].action, "fp");
        assert_eq!(f.provenance[0].at, "t1");
        assert_eq!(shard.updated_at, "t1");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dispose_finding_identical_redispose_does_not_grow_provenance() {
        // BLOCKER 1 — an idempotent re-dispose (same actor+action as the last entry)
        // must NOT append; repeated re-dispose cannot bloat the shard.
        let dir = unique_temp_root("dispose-dedup");
        let root = dir.as_path();
        let rel = "src/a.rs";
        write_shard(
            root,
            &CensorShard {
                file_rel_path: rel.into(),
                content_hash: "h".into(),
                updated_at: "t0".into(),
                findings: vec![finding("f1", "h", Disposition::Open)],
            },
        )
        .unwrap();
        for i in 0..200 {
            dispose_finding(root, rel, "f1", Disposition::Fp, "proj-7", &format!("t{i}")).unwrap();
        }
        let shard = read_shard(root, rel).unwrap().unwrap();
        // Exactly ONE fp entry (the test finding started with empty provenance).
        assert_eq!(shard.findings[0].provenance.len(), 1);
        assert_eq!(shard.findings[0].provenance[0].action, "fp");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dispose_finding_alternating_disposes_are_capped_oldest_dropped() {
        // BLOCKER 1 — alternating disposes each append (different action), but the
        // trail is capped at CENSOR_PROVENANCE_MAX with the OLDEST dropped.
        let dir = unique_temp_root("dispose-cap");
        let root = dir.as_path();
        let rel = "src/a.rs";
        write_shard(
            root,
            &CensorShard {
                file_rel_path: rel.into(),
                content_hash: "h".into(),
                updated_at: "t0".into(),
                findings: vec![finding("f1", "h", Disposition::Open)],
            },
        )
        .unwrap();
        let toggles = [
            Disposition::Fp,
            Disposition::Open,
            Disposition::Wontfix,
            Disposition::Fixed,
        ];
        for i in 0..500usize {
            dispose_finding(
                root,
                rel,
                "f1",
                toggles[i % toggles.len()],
                "p",
                &format!("t{i}"),
            )
            .unwrap();
        }
        let shard = read_shard(root, rel).unwrap().unwrap();
        assert!(shard.findings[0].provenance.len() <= CENSOR_PROVENANCE_MAX);
        // The trail keeps the MOST RECENT entries: the last `at` stamp survives.
        assert_eq!(shard.findings[0].provenance.last().unwrap().at, "t499");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shard_path_collapses_consecutive_slashes() {
        // NITPICK 1 — `src//a.rs`, `src/a.rs` and a Windows `src\\a.rs` map to ONE
        // shard, and the hash is BYTE-IDENTICAL to the Python side (sha256 of the
        // collapsed "src/a.rs").
        let root = Path::new("/proj");
        let single = shard_path(root, "src/a.rs").unwrap();
        assert_eq!(single, shard_path(root, "src//a.rs").unwrap());
        assert_eq!(single, shard_path(root, "src\\\\a.rs").unwrap());
        let expected = {
            let mut h = Sha256::new();
            h.update(b"src/a.rs");
            hex::encode(h.finalize())
        };
        assert_eq!(
            single.file_name().unwrap().to_string_lossy(),
            format!("{expected}.json")
        );
    }

    #[test]
    fn dispose_finding_unknown_id_or_missing_shard_errs() {
        let dir = unique_temp_root("dispose-missing");
        let root = dir.as_path();
        let rel = "src/a.rs";
        // Missing shard → NotFound.
        let err = dispose_finding(root, rel, "f1", Disposition::Fp, "p", "t").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        // Present shard, unknown id → NotFound.
        write_shard(
            root,
            &CensorShard {
                file_rel_path: rel.into(),
                content_hash: "h".into(),
                updated_at: "t0".into(),
                findings: vec![finding("f1", "h", Disposition::Open)],
            },
        )
        .unwrap();
        let err = dispose_finding(root, rel, "nope", Disposition::Fp, "p", "t").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        let _ = fs::remove_dir_all(&dir);
    }

    // ---- list_shards: enumerate, skip sidecars/corrupt, missing dir → empty ----

    #[test]
    fn list_shards_enumerates_and_tolerates() {
        let dir = unique_temp_root("list");
        let root = dir.as_path();
        // Missing dir → empty.
        assert!(list_shards(root).unwrap().is_empty());

        write_shard(
            root,
            &CensorShard {
                file_rel_path: "src/a.rs".into(),
                content_hash: "h".into(),
                updated_at: "t0".into(),
                findings: vec![finding("f1", "h", Disposition::Open)],
            },
        )
        .unwrap();
        write_shard(
            root,
            &CensorShard {
                file_rel_path: "src/b.rs".into(),
                content_hash: "h".into(),
                updated_at: "t0".into(),
                findings: vec![finding("f2", "h", Disposition::Fp)],
            },
        )
        .unwrap();
        // Plant a corrupt .json shard (must be skipped, not abort the listing).
        fs::write(censor_dir(root).join("corrupt.json"), "{ not json ").unwrap();

        let shards = list_shards(root).unwrap();
        assert_eq!(shards.len(), 2, "two valid shards, corrupt skipped");
        let paths: std::collections::HashSet<String> =
            shards.iter().map(|s| s.file_rel_path.clone()).collect();
        assert!(paths.contains("src/a.rs"));
        assert!(paths.contains("src/b.rs"));

        let _ = fs::remove_dir_all(&dir);
    }

    // ---- SOURCE-SCOPED merge (clobber-avoidance) ----

    #[test]
    fn supersede_sources_does_not_clobber_unrefreshed_sources() {
        // A file's shard holds an eslint finding AND a clippy finding at hash "h".
        // A FINE eslint pass (refreshed = {eslint}) re-emits its eslint finding.
        // The clippy finding (source NOT refreshed) MUST survive untouched.
        let old = CensorShard {
            file_rel_path: "src/a.ts".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            findings: vec![
                finding_src("es1", "h", Disposition::Open, "eslint"),
                finding_src("cl1", "h", Disposition::Open, "clippy"),
            ],
        };
        let new = vec![finding_src("es1", "ignored", Disposition::Open, "eslint")];
        let result = supersede_sources(Some(old), new, "h", &srcs(&["eslint"]), "src/a.ts", "t1");
        let ids: std::collections::HashSet<&str> =
            result.findings.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains("es1"), "refreshed eslint finding present");
        assert!(
            ids.contains("cl1"),
            "unrefreshed clippy finding must SURVIVE"
        );
        assert_eq!(result.findings.len(), 2);
    }

    #[test]
    fn supersede_sources_coarse_pass_adds_without_dropping_fine() {
        // Shard already has an eslint (fine) finding at hash "h". A COARSE clippy
        // pass (refreshed = {clippy}) adds a clippy finding. The eslint finding is
        // NOT in the refreshed set → it survives; the clippy finding is added.
        let old = CensorShard {
            file_rel_path: "src/a.ts".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            findings: vec![finding_src("es1", "h", Disposition::Open, "eslint")],
        };
        let new = vec![finding_src("cl1", "ignored", Disposition::Open, "clippy")];
        let result = supersede_sources(Some(old), new, "h", &srcs(&["clippy"]), "src/a.ts", "t1");
        let ids: std::collections::HashSet<&str> =
            result.findings.iter().map(|f| f.id.as_str()).collect();
        assert!(
            ids.contains("es1"),
            "fine eslint finding survives coarse pass"
        );
        assert!(ids.contains("cl1"), "coarse clippy finding added");
        assert_eq!(result.findings.len(), 2);
    }

    #[test]
    fn supersede_sources_second_fine_pass_replaces_only_that_source() {
        // eslint + clippy present. A second eslint pass emits a DIFFERENT eslint
        // finding (es2, es1 gone). Only eslint findings are reconciled: es1 (Open,
        // refreshed, not re-emitted) is dropped, es2 added, clippy untouched.
        let old = CensorShard {
            file_rel_path: "src/a.ts".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            findings: vec![
                finding_src("es1", "h", Disposition::Open, "eslint"),
                finding_src("cl1", "h", Disposition::Open, "clippy"),
            ],
        };
        let new = vec![finding_src("es2", "ignored", Disposition::Open, "eslint")];
        let result = supersede_sources(Some(old), new, "h", &srcs(&["eslint"]), "src/a.ts", "t1");
        let ids: std::collections::HashSet<&str> =
            result.findings.iter().map(|f| f.id.as_str()).collect();
        assert!(!ids.contains("es1"), "old eslint finding replaced");
        assert!(ids.contains("es2"), "new eslint finding present");
        assert!(ids.contains("cl1"), "clippy finding untouched");
        assert_eq!(result.findings.len(), 2);
    }

    #[test]
    fn supersede_sources_preserves_fp_on_refreshed_re_emit() {
        // An eslint finding marked Fp survives a re-run that re-emits the same id.
        let mut judged = finding_src("es1", "h", Disposition::Fp, "eslint");
        judged.provenance = vec![ProvenanceEntry {
            actor: "coder".into(),
            action: "fp".into(),
            role: String::new(),
            at: "t0".into(),
        }];
        let old = CensorShard {
            file_rel_path: "src/a.ts".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            findings: vec![judged],
        };
        let new = vec![finding_src("es1", "ignored", Disposition::Open, "eslint")];
        let result = supersede_sources(Some(old), new, "h", &srcs(&["eslint"]), "src/a.ts", "t1");
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].disposition, Disposition::Fp);
        assert_eq!(result.findings[0].provenance.len(), 1);
    }

    #[test]
    fn supersede_sources_hash_change_drops_everything_including_unrefreshed() {
        // A content change (hash) drops ALL old findings for the file, even ones
        // whose source is not in the refreshed set — the code they described is gone.
        let old = CensorShard {
            file_rel_path: "src/a.ts".into(),
            content_hash: "old".into(),
            updated_at: "t0".into(),
            findings: vec![
                finding_src("es1", "old", Disposition::Open, "eslint"),
                finding_src("cl1", "old", Disposition::Fp, "clippy"),
            ],
        };
        let new = vec![finding_src("es2", "ignored", Disposition::Open, "eslint")];
        let result = supersede_sources(Some(old), new, "new", &srcs(&["eslint"]), "src/a.ts", "t1");
        // Everything old dropped; only the fresh eslint finding remains.
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].id, "es2");
        assert_eq!(result.findings[0].content_hash, "new");
    }

    #[test]
    fn supersede_merged_set_has_unique_ids_even_with_artificial_cross_bucket_collision() {
        // DEFENSE-IN-DEPTH (MAJOR-defensive-dedup). By construction a cross-bucket id
        // collision is IMPOSSIBLE: `compute_id` mixes `source`, so a kept_other
        // finding (source NOT refreshed) and a new finding (source IN refreshed) can
        // never share an id. We force the impossible anyway — an old finding and a
        // new finding sharing an id "dup" but DIFFERENT sources, refreshing only the
        // new source — to prove the final dedup keeps the merged set unique.
        let old = CensorShard {
            file_rel_path: "src/a.ts".into(),
            content_hash: "h".into(),
            updated_at: "t0".into(),
            // Same id "dup", source "clippy" (NOT refreshed → would land in
            // kept_other), Wontfix so it cannot be dropped as resolved-by-absence.
            findings: vec![finding_src("dup", "h", Disposition::Wontfix, "clippy")],
        };
        // New finding shares id "dup" but source "eslint" (refreshed).
        let new = vec![finding_src("dup", "ignored", Disposition::Open, "eslint")];
        let result = supersede_sources(Some(old), new, "h", &srcs(&["eslint"]), "src/a.ts", "t1");

        // Exactly ONE finding with id "dup" survives (no duplicate ids on disk,
        // which would break dispose_finding's first-match lookup + React keys).
        let dup_count = result.findings.iter().filter(|f| f.id == "dup").count();
        assert_eq!(dup_count, 1, "merged set must not carry duplicate ids");
        // FIRST-OCCURRENCE precedence: kept_other is appended before new findings,
        // so the kept_other (clippy/Wontfix) entry wins.
        let f = result.findings.iter().find(|f| f.id == "dup").unwrap();
        assert_eq!(
            f.source, "clippy",
            "kept_other precedence (first-occurrence)"
        );
        assert_eq!(f.disposition, Disposition::Wontfix);
    }

    #[test]
    fn read_supersede_write_eslint_then_clippy_clobber_free_on_disk() {
        // End-to-end on disk: a fine eslint pass writes es1; a coarse clippy pass
        // (refreshed={clippy}) adds cl1 WITHOUT dropping es1; a second eslint pass
        // (refreshed={eslint}) emitting es2 replaces es1 but leaves cl1; an Fp on
        // an eslint finding survives a re-emit.
        let dir = unique_temp_root("rsw-scoped");
        let root = dir.as_path();
        let rel = "src/a.ts";

        // 1) Fine eslint pass at hash "h".
        let s1 = read_supersede_write_shard(
            root,
            vec![finding_src("es1", "ignored", Disposition::Open, "eslint")],
            "h",
            &srcs(&["eslint"]),
            rel,
            "t1",
        )
        .unwrap();
        assert_eq!(s1.findings.len(), 1);

        // 2) Coarse clippy pass at the SAME hash → es1 survives, cl1 added.
        let s2 = read_supersede_write_shard(
            root,
            vec![finding_src("cl1", "ignored", Disposition::Open, "clippy")],
            "h",
            &srcs(&["clippy", "cargo-check", "cargo-audit", "gitleaks", "jscpd"]),
            rel,
            "t2",
        )
        .unwrap();
        let ids: std::collections::HashSet<String> =
            s2.findings.iter().map(|f| f.id.clone()).collect();
        assert!(ids.contains("es1"), "eslint survives the coarse pass");
        assert!(ids.contains("cl1"), "clippy finding added");
        assert_eq!(s2.findings.len(), 2);

        // 3) A coder marks es1 as Fp out-of-band (simulate via a direct write).
        let mut shard = read_shard(root, rel).unwrap().unwrap();
        for f in &mut shard.findings {
            if f.id == "es1" {
                f.disposition = Disposition::Fp;
                f.provenance.push(ProvenanceEntry {
                    actor: "coder".into(),
                    action: "fp".into(),
                    role: String::new(),
                    at: "t3".into(),
                });
            }
        }
        write_shard(root, &shard).unwrap();

        // 4) Second eslint pass re-emits es1 (and es2). The Fp on es1 survives,
        //    cl1 (unrefreshed) survives.
        let s4 = read_supersede_write_shard(
            root,
            vec![
                finding_src("es1", "ignored", Disposition::Open, "eslint"),
                finding_src("es2", "ignored", Disposition::Open, "eslint"),
            ],
            "h",
            &srcs(&["eslint"]),
            rel,
            "t4",
        )
        .unwrap();
        let es1 = s4
            .findings
            .iter()
            .find(|f| f.id == "es1")
            .expect("es1 present");
        assert_eq!(
            es1.disposition,
            Disposition::Fp,
            "Fp preserved across re-run"
        );
        assert!(s4.findings.iter().any(|f| f.id == "es2"), "es2 added");
        assert!(
            s4.findings.iter().any(|f| f.id == "cl1"),
            "clippy untouched"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
