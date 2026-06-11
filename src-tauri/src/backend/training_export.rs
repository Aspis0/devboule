//! Training-export rail: an append-only, per-project local JSONL capture of the
//! code-review cycle (Censor findings + mini-coder directive results + censor
//! verdicts) for FUTURE preference training. Nothing here feeds back into a live
//! decision; it is a write-only side-channel.
//!
//! On-disk layout, rooted at `<project_root>/.aspis-training/`:
//!   * `.gitignore`            — literal `*`, written ONCE on dir creation so the
//!                               whole rail self-excludes from version control.
//!   * `findings.jsonl`        — one record per Censor batch per file.
//!   * `pairs.jsonl`           — directive_result / censor_verdict events.
//!   * `blobs/<sha256>`        — content-addressed file snapshots, 256 KiB cap
//!                               each, deduped (an existing blob is never rewritten).
//!
//! ## Fire-and-forget contract
//! Every PUBLIC entry point that does IO (`record_findings_batch`,
//! `record_directive_result`) returns `()` and NEVER surfaces an error to the
//! caller — these run on hot agent/censor threads and a training-rail hiccup must
//! never perturb the real pipeline. Internal failures degrade to an `eprintln!`
//! that logs the PATH ONLY (never file contents, never finding bodies, never blob
//! bytes — privacy: this rail handles user source).
//!
//! ## Lock ordering (HARD INVARIANT — read before touching the registry)
//! This module owns ONE class of lock: a process-wide per-path JSONL mutex
//! (`append_jsonl`'s registry). That mutex must NEVER be held while the agent-state
//! lock (`AgentLiveState` in model.rs / agents.rs) is taken. The discipline that
//! guarantees this: CALLERS read their agent-state snapshot FIRST (under the
//! agent-state lock), release it, THEN call into this module with owned
//! `&[MiniCoderDirective]` / `&[AgentSession]` copies. This module itself NEVER
//! touches agent state — it only reads the snapshots it was handed. So the two
//! lock classes are acquired in strictly disjoint critical sections and can never
//! deadlock against each other.
//!
//! ## Concurrency
//! `pairs.jsonl` has TWO concurrent writers in production: the Censor worker thread
//! (`record_findings_batch` -> censor_verdict lines) and the mini-coder executor
//! thread (`record_directive_result`). `append_jsonl` serializes them per-path via
//! a `Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>` registry so a line is never
//! interleaved/torn.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::censor::schema::{Finding, Severity};
use super::mini_coder::{MiniCoderDirective, MiniCoderOutcome, MiniCoderStatus};
use super::model::AgentSession;

// ---------------------------------------------------------------------------
// Constants (test-injectable variants live alongside the public wrappers)
// ---------------------------------------------------------------------------

/// A file is attributed to a mini-coder directive only if that directive is active
/// OR reached a terminal state within this many seconds of "now". Past the window a
/// long-dead directive can no longer claim a freshly-changed file (the edit is far
/// more likely a later coder pass). Mirrors the mini wall-clock cap (600s).
pub const ATTRIBUTION_WINDOW_SECS: i64 = 600;

/// Per-blob content cap. A snapshot larger than this is skipped (the rail is for
/// review-sized files, not build artifacts / vendored blobs).
const BLOB_CAP_BYTES: u64 = 256 * 1024; // 256 KiB

/// `output` field cap (chars) inside a `directive_result` record.
const OUTPUT_CAP_CHARS: usize = 64 * 1024; // 64 KiB

/// Rotation threshold: when a JSONL file reaches this size it is renamed to `.1`
/// (overwriting any prior `.1`) before the new line is appended.
const ROTATE_AT_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

// ---------------------------------------------------------------------------
// Attribution (PURE — no IO)
// ---------------------------------------------------------------------------

/// A lean finding projection the wiring callers can build from a Censor shard
/// without coupling this module to the full `Finding`/ledger internals beyond the
/// schema types it already imports. `From<&Finding>` is provided for the common case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingLite {
    pub id: String,
    pub severity: Severity,
    pub category: String,
    pub source: String,
    pub title: String,
    pub line: Option<u32>,
}

impl From<&Finding> for FindingLite {
    fn from(f: &Finding) -> Self {
        FindingLite {
            id: f.id.clone(),
            severity: f.severity,
            // Serde token for the category, identical to the on-wire string.
            category: f.category.id_token().to_string(),
            source: f.source.clone(),
            title: f.title.clone(),
            line: f.line,
        }
    }
}

/// Who a changed file is attributed to. Absent from the attribution map == no
/// confident attribution (neither a mini directive nor a coder session claims it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// A mini-coder directive (its parent coder is the responsible agent).
    Mini {
        directive_id: String,
        agent_id: String,
    },
    /// A live coder-role session.
    Coder { agent_id: String },
}

/// Severity -> the lowercase wire token, matching `Severity`'s serde.
fn severity_token(s: Severity) -> &'static str {
    match s {
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
    }
}

/// Severity ordering for `maxSeverity` (high > medium > low).
fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::High => 2,
        Severity::Medium => 1,
        Severity::Low => 0,
    }
}

/// The directive's best "activity time": the most recent of started/claimed/created
/// timestamps that parses as RFC3339. None if the directive carries no parseable
/// stamp at all (a hand-written / not-yet-launched entry).
fn directive_activity_time(d: &MiniCoderDirective) -> Option<DateTime<Utc>> {
    let candidates = [
        d.started_at.as_deref(),
        d.claimed_at.as_deref(),
        if d.created_at.is_empty() {
            None
        } else {
            Some(d.created_at.as_str())
        },
    ];
    candidates
        .into_iter()
        .flatten()
        .filter_map(parse_rfc3339)
        .max()
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// WARNING 11: normalize a path for cross-writer comparison. Censor's MCP writer emits
/// forward-slash paths; a Windows mini-coder result can carry `\`. Normalize separators to
/// `/` so `src\a.rs` and `src/a.rs` compare equal. Case is folded ONLY on Windows (its
/// filesystem is case-insensitive) — matching the tolerance the blob `abs_of` split has.
fn normalize_path_key(p: &str) -> String {
    let slashed = p.replace('\\', "/");
    if cfg!(windows) {
        slashed.to_ascii_lowercase()
    } else {
        slashed
    }
}

/// Does this directive "touch" `file`? True if the file is in its requested
/// `files[]` OR in its result's `files_touched`. Paths are normalized (separator +
/// Windows case) before comparison so a `\`-vs-`/` mismatch doesn't silently miss.
fn directive_touches(d: &MiniCoderDirective, file: &str) -> bool {
    let target = normalize_path_key(file);
    if d.files.iter().any(|f| normalize_path_key(f) == target) {
        return true;
    }
    if let Some(result) = &d.result {
        if result
            .files_touched
            .iter()
            .any(|f| normalize_path_key(f) == target)
        {
            return true;
        }
    }
    false
}

/// Is the directive eligible to claim a freshly-changed file right now? Active
/// directives always are; terminal ones only within the attribution window of `now`.
fn directive_in_window(d: &MiniCoderDirective, now: DateTime<Utc>) -> bool {
    if d.status.is_active() {
        return true;
    }
    if d.status.is_terminal() {
        if let Some(t) = directive_activity_time(d) {
            // WARNING 10: NO `.abs()`. A future-dated terminal stamp (clock skew or a
            // crafted directive) means `now - t < 0` — that is OUT of window, not "recent".
            // Only a stamp in the past, within the window, attributes. (Active directives
            // are handled by the `is_active()` branch above, regardless of stamp.)
            let age = (now - t).num_seconds();
            return (0..=ATTRIBUTION_WINDOW_SECS).contains(&age);
        }
        // Terminal but no parseable stamp -> cannot prove it's recent -> not eligible.
        return false;
    }
    // Pending: not yet doing work, do not attribute.
    false
}

/// PURE attribution: for each changed file, decide whether a mini directive or a
/// coder session owns the edit. See module rules. Uses `Utc::now()` for the window
/// check (the only clock read in the module's pure surface; callers don't need to
/// thread a clock and the 600s window is coarse).
pub fn attribute_files(
    files: &[String],
    directives: &[MiniCoderDirective],
    sessions: &[AgentSession],
) -> HashMap<String, Attribution> {
    attribute_files_at(files, directives, sessions, Utc::now())
}

/// Clock-split core of `attribute_files` (testable without wall-clock).
pub fn attribute_files_at(
    files: &[String],
    directives: &[MiniCoderDirective],
    sessions: &[AgentSession],
    now: DateTime<Utc>,
) -> HashMap<String, Attribution> {
    // Pre-compute the single active coder session (the coder fallback) once.
    let coder_sessions: Vec<&AgentSession> = sessions
        .iter()
        .filter(|s| is_live_coder(s))
        .collect();
    let sole_active_coder: Option<&AgentSession> = if coder_sessions.len() == 1 {
        Some(coder_sessions[0])
    } else {
        None
    };

    let mut out = HashMap::new();
    for file in files {
        // 1) Mini wins: pick the most-recent in-window directive that touches the file.
        let mini = directives
            .iter()
            .filter(|d| directive_in_window(d, now) && directive_touches(d, file))
            .max_by_key(|d| directive_activity_time(*d).unwrap_or(DateTime::<Utc>::MIN_UTC));

        if let Some(d) = mini {
            out.insert(
                file.clone(),
                Attribution::Mini {
                    directive_id: d.id.clone(),
                    agent_id: d.parent_agent_id.clone(),
                },
            );
            continue;
        }

        // 2) Coder fallback: a live coder whose current file matches, else the sole
        //    active coder for the project.
        let by_file = coder_sessions
            .iter()
            .copied()
            .filter(|s| s.current_file_path.as_deref() == Some(file.as_str()))
            // most-recent-wins via last_seen_at
            .max_by_key(|s| {
                s.last_seen_at
                    .as_deref()
                    .and_then(parse_rfc3339)
                    .unwrap_or(DateTime::<Utc>::MIN_UTC)
            });

        let coder = by_file.or(sole_active_coder);
        if let Some(s) = coder {
            out.insert(
                file.clone(),
                Attribution::Coder {
                    agent_id: s.agent_id.clone(),
                },
            );
        }
        // else: no candidate -> absent from the map.
    }
    out
}

/// A session that can own a code edit: coder role and not a terminated/closed
/// status. We treat the role string leniently ("" normalizes to coder per the
/// Python MCP contract, see AgentSession::role doc).
fn is_live_coder(s: &AgentSession) -> bool {
    let role = if s.role.is_empty() { "coder" } else { s.role.as_str() };
    if role != "coder" {
        return false;
    }
    // A closed/exited session is not "live". The fleet uses free-form status
    // strings; treat the known-dead ones as not-live, everything else as live.
    !matches!(s.status.as_str(), "closed" | "exited" | "terminated" | "dead")
}

fn attribution_json(a: &Attribution) -> serde_json::Value {
    match a {
        Attribution::Mini {
            directive_id,
            agent_id,
        } => json!({ "kind": "mini", "directiveId": directive_id, "agentId": agent_id }),
        Attribution::Coder { agent_id } => json!({ "kind": "coder", "agentId": agent_id }),
    }
}

// ---------------------------------------------------------------------------
// Per-path append registry + rotation (process-wide)
// ---------------------------------------------------------------------------

/// Registry of per-path mutexes so two threads appending the SAME jsonl path are
/// serialized, while writes to DIFFERENT paths proceed in parallel.
fn path_locks() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The per-path append lock for `path`, created on first use. Holding the registry
/// mutex only long enough to clone out the path's `Arc<Mutex<()>>`.
fn lock_for(path: &Path) -> Arc<Mutex<()>> {
    let mut map = path_locks()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    map.entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Append one compact JSON object as a line to `path`, rotating at `rotate_at`.
/// Process-wide serialized per path. Returns `io::Result` so internal callers can
/// log+swallow; the PUBLIC API never propagates it.
fn append_jsonl_at(
    path: &Path,
    value: &serde_json::Value,
    rotate_at: u64,
) -> std::io::Result<()> {
    let guard = lock_for(path);
    let _held = guard.lock().unwrap_or_else(|p| p.into_inner());

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Rotation: if the current file is at/over the cap, rename to `.1` (clobbering
    // any prior `.1`) so the live file restarts empty.
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= rotate_at {
            let rotated = rotated_path(path);
            // Best-effort: a failed rotate must not lose the append, so we only
            // proceed to append regardless. Overwrite old `.1`.
            let _ = std::fs::rename(path, &rotated);
        }
    }

    let mut line = serde_json::to_string(value).map_err(std::io::Error::other)?;
    line.push('\n');

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

/// `findings.jsonl` -> `findings.jsonl.1`.
fn rotated_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".1");
    PathBuf::from(s)
}

/// Public-ish append used by the record functions with the production rotation cap.
fn append_jsonl(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    append_jsonl_at(path, value, ROTATE_AT_BYTES)
}

/// BLOCKER 3: the single, shared appender for `<root>/.aspis-training/findings.jsonl`.
/// Other modules (e.g. `api_fuzz`) MUST route their findings writes through THIS function
/// rather than maintaining a second per-path mutex registry — otherwise a concurrent
/// Censor batch and an api-fuzz run could interleave/torn-write the same file and race
/// rotation. This ensures both writers serialize on the ONE `path_locks()` registry.
///
/// Ensures the training dir + self-`.gitignore` exist, then appends `value` as a single
/// compact-JSON line with the production rotation cap. Returns `io::Result` so the caller
/// can log+swallow.
pub fn append_findings_line(root: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let dir = ensure_training_dir(root)?;
    let path = dir.join("findings.jsonl");
    append_jsonl(&path, value)
}

/// BLOCKER 3 test hook: expose the shared per-path append lock for a given path so a
/// cross-module test can assert that `api_fuzz` and `training_export` resolve to the
/// SAME `Arc<Mutex<()>>` (one registry, not two).
#[cfg(test)]
pub fn lock_for_path_test_hook(path: &Path) -> Arc<Mutex<()>> {
    lock_for(path)
}

// ---------------------------------------------------------------------------
// Training dir + self-gitignore
// ---------------------------------------------------------------------------

/// `<root>/.aspis-training`.
fn training_dir(root: &Path) -> PathBuf {
    root.join(".aspis-training")
}

/// Ensure the training dir exists with its self-`.gitignore` (`*`). The gitignore
/// is written ONCE: if it already exists we leave it untouched (idempotent init,
/// no mtime churn). Returns the dir path on success.
fn ensure_training_dir(root: &Path) -> std::io::Result<PathBuf> {
    let dir = training_dir(root);
    std::fs::create_dir_all(&dir)?;
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        std::fs::write(&gi, b"*\n")?;
    }
    Ok(dir)
}

// ---------------------------------------------------------------------------
// Blob snapshots (content-addressed, deduped)
// ---------------------------------------------------------------------------

/// Snapshot `file_abs` into `<root>/.aspis-training/blobs/<sha256>`, returning the
/// hex hash. Returns None (no write) if the file is missing, larger than the blob
/// cap, or binary (NUL-byte heuristic). Deduped: if the blob already exists the
/// hash is returned WITHOUT rewriting.
pub fn snapshot_blob(root: &Path, file_abs: &Path) -> Option<String> {
    snapshot_blob_capped(root, file_abs, BLOB_CAP_BYTES)
}

fn snapshot_blob_capped(root: &Path, file_abs: &Path, cap: u64) -> Option<String> {
    let meta = std::fs::metadata(file_abs).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    // WARNING 6: BOUNDED read to avoid a TOCTOU window where the file grows between the
    // `metadata` check and the read (which would let an over-cap blob through). Read at
    // most `cap + 1` bytes; if we actually got more than `cap`, the file grew past the
    // cap after the stat — skip it.
    let bytes = {
        use std::io::Read;
        let f = std::fs::File::open(file_abs).ok()?;
        let mut buf = Vec::new();
        f.take(cap + 1).read_to_end(&mut buf).ok()?;
        if buf.len() as u64 > cap {
            return None;
        }
        buf
    };
    // Binary heuristic: any NUL byte -> treat as binary, skip.
    if bytes.contains(&0u8) {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = hex::encode(hasher.finalize());

    let dir = ensure_training_dir(root).ok()?;
    let blob_dir = dir.join("blobs");
    if std::fs::create_dir_all(&blob_dir).is_err() {
        return None;
    }
    let blob_path = blob_dir.join(&hash);
    if blob_path.exists() {
        // Dedupe: identical content already snapshotted, do not rewrite.
        return Some(hash);
    }
    // Write atomically enough: a torn blob would still hash-mismatch on read, but
    // the dedupe key is the name, so a plain write is fine for a local rail.
    if std::fs::write(&blob_path, &bytes).is_err() {
        return None;
    }
    Some(hash)
}

// ---------------------------------------------------------------------------
// Public record API (fire-and-forget)
// ---------------------------------------------------------------------------

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Resolve a caller-supplied repo-relative (forward-slash) path against `root` for
/// blob snapshotting, without canonicalizing beyond the join.
fn abs_of(root: &Path, rel: &str) -> PathBuf {
    // Accept either '/' or native separators in the incoming key.
    let mut p = root.to_path_buf();
    for seg in rel.split(['/', '\\']) {
        if !seg.is_empty() {
            p.push(seg);
        }
    }
    p
}

/// Record one Censor batch: ALWAYS append a `findings.jsonl` record per changed
/// file (attribution included when known, absent when not), and additionally append
/// a `censor_verdict` line to `pairs.jsonl` for each ATTRIBUTED file — including the
/// zero-finding (clean) verdict, which is the "chosen" signal after a dirty one.
///
/// Fire-and-forget: never returns Err. `shard_lookup(file)` yields the file's
/// current findings (None == no shard / not reviewed -> empty findings record).
///
/// The caller MUST have already released the agent-state lock before calling (it
/// passes owned snapshots `directives`/`sessions`); see the module lock-ordering note.
pub fn record_findings_batch(
    root: &Path,
    changed_files: &[String],
    shard_lookup: impl Fn(&str) -> Option<Vec<FindingLite>>,
    directives: &[MiniCoderDirective],
    sessions: &[AgentSession],
) {
    let attributions = attribute_files(changed_files, directives, sessions);
    let dir = training_dir(root);
    let findings_path = dir.join("findings.jsonl");
    let pairs_path = dir.join("pairs.jsonl");

    for file in changed_files {
        let findings = shard_lookup(file).unwrap_or_default();
        let attribution = attributions.get(file);

        // Snapshot the file content (best-effort) for both records' contentHash/blob.
        let blob = snapshot_blob(root, &abs_of(root, file));
        let content_hash = blob.clone().unwrap_or_default();

        // findings.jsonl: ALWAYS, attribution optional.
        let findings_json: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                json!({
                    "id": f.id,
                    "severity": severity_token(f.severity),
                    "category": f.category,
                    "source": f.source,
                    "title": f.title,
                    "line": f.line,
                })
            })
            .collect();
        let mut rec = json!({
            "ts": now_rfc3339(),
            "file": file,
            "contentHash": content_hash,
            "findings": findings_json,
        });
        if let Some(a) = attribution {
            rec["attribution"] = attribution_json(a);
        }
        if let Err(e) = append_jsonl(&findings_path, &rec) {
            eprintln!(
                "training_export: findings append failed at {}: {}",
                findings_path.display(),
                e
            );
        }

        // pairs.jsonl censor_verdict: ONLY for attributed files (clean included).
        if let Some(a) = attribution {
            let max_sev = findings
                .iter()
                .map(|f| f.severity)
                .max_by_key(|s| severity_rank(*s))
                .map(severity_token);
            let open_findings = findings.len();
            let verdict = json!({
                "type": "censor_verdict",
                "ts": now_rfc3339(),
                "file": file,
                "contentHash": content_hash,
                "blob": blob,
                "openFindings": open_findings,
                "maxSeverity": max_sev,
                "attribution": attribution_json(a),
            });
            if let Err(e) = append_jsonl(&pairs_path, &verdict) {
                eprintln!(
                    "training_export: censor_verdict append failed at {}: {}",
                    pairs_path.display(),
                    e
                );
            }
        }
    }
}

/// Record one mini-coder directive's terminal result as a `directive_result` line
/// in `pairs.jsonl`, snapshotting every `files_touched` into blobs. `output` is
/// capped at 64 KiB chars. Fire-and-forget: never returns Err.
pub fn record_directive_result(
    root: &Path,
    directive: &MiniCoderDirective,
    outcome: &MiniCoderOutcome,
) {
    record_directive_result_capped(root, directive, outcome, OUTPUT_CAP_CHARS)
}

fn record_directive_result_capped(
    root: &Path,
    directive: &MiniCoderDirective,
    outcome: &MiniCoderOutcome,
    output_cap: usize,
) {
    let dir = training_dir(root);
    let pairs_path = dir.join("pairs.jsonl");

    // Snapshot every touched file (best-effort, deduped).
    //
    // WARNING 9: `outcome.files_touched` comes from the mini's parsed result JSON and is
    // UNTRUSTED. Without a guard, an entry like `../../etc/shadow` would have `abs_of`
    // resolve it (it splits on separators and pushes each segment, so a leading `..`
    // escapes `root`) and snapshot an arbitrary file into the blob store. Validate each
    // entry with the SAME rel-path guard the orchestrator/ledger uses (rejects absolute,
    // `..`, drive-letter, `-`-leading) and SKIP invalid entries — never snapshot them.
    let mut blobs = serde_json::Map::new();
    for f in &outcome.files_touched {
        if super::censor::ledger::validate_rel_path(f).is_err() {
            // Skip silently: a malicious/malformed path is not a recordable touch.
            continue;
        }
        if let Some(hash) = snapshot_blob(root, &abs_of(root, f)) {
            blobs.insert(f.clone(), serde_json::Value::String(hash));
        }
    }

    let output = outcome.output.as_deref().map(|o| cap_chars(o, output_cap));

    // Phase 6 will add real attempt / parentDirectiveId; emit stable defaults today.
    let attempt = directive_attempt(directive);
    let parent_directive_id = directive_parent_directive_id(directive);

    let mut rec = json!({
        "type": "directive_result",
        "ts": now_rfc3339(),
        "directiveId": directive.id,
        "parentAgentId": directive.parent_agent_id,
        "attempt": attempt,
        "parentDirectiveId": parent_directive_id,
        "task": directive.task,
        "files": directive.files,
        "status": status_token(outcome.status),
        "output": output,
        "filesTouched": outcome.files_touched,
        "blobs": serde_json::Value::Object(blobs),
    });

    // B-F7: for an escalated outcome, include the `escalation { attempts, findings }`
    // payload — the RICHEST training signal (why the mini chain gave up: how many tries
    // and the still-open High findings). It rides on the outcome only for the escalated
    // case; serialize it natively (camelCase, privacy-safe — no file body, no secrets).
    if let Some(escalation) = &outcome.escalation {
        if let (Ok(value), Some(map)) = (serde_json::to_value(escalation), rec.as_object_mut()) {
            map.insert("escalation".to_string(), value);
        }
    }

    if let Err(e) = append_jsonl(&pairs_path, &rec) {
        eprintln!(
            "training_export: directive_result append failed at {}: {}",
            pairs_path.display(),
            e
        );
    }
}

/// Truncate to at most `cap` CHARS (not bytes) on a char boundary.
///
/// NIT 12: O(cap), not O(len). `char_indices().nth(cap)` finds the byte offset of the
/// (cap+1)-th char without scanning the whole string; if it returns None the string has
/// `<= cap` chars and is returned as-is.
fn cap_chars(s: &str, cap: usize) -> String {
    match s.char_indices().nth(cap) {
        Some((byte_idx, _)) => s[..byte_idx].to_string(),
        None => s.to_string(),
    }
}

fn status_token(s: MiniCoderStatus) -> &'static str {
    match s {
        MiniCoderStatus::Pending => "pending",
        MiniCoderStatus::Launching => "launching",
        MiniCoderStatus::Running => "running",
        MiniCoderStatus::Done => "done",
        MiniCoderStatus::NeedsClarification => "needs_clarification",
        MiniCoderStatus::AbortedByHuman => "aborted_by_human",
        MiniCoderStatus::Failed => "failed",
        MiniCoderStatus::Timeout => "timeout",
        MiniCoderStatus::AwaitingRetry => "awaiting_retry",
        MiniCoderStatus::Escalated => "escalated",
    }
}

// P6: the directive now carries the real retry lineage; these read it directly
// (the Phase-2 forward-compat shims that returned 0/None are retired).
fn directive_attempt(d: &MiniCoderDirective) -> u32 {
    d.attempt
}
fn directive_parent_directive_id(d: &MiniCoderDirective) -> Option<String> {
    d.parent_directive_id.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Unique tempdir per test (process id + monotonic counter), mirroring the
    // mini_coder.rs test idiom but collision-free across tests in one process.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    fn tmp(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("tx_{tag}_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rfc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn directive(id: &str, parent: &str) -> MiniCoderDirective {
        MiniCoderDirective {
            id: id.to_string(),
            parent_agent_id: parent.to_string(),
            status: MiniCoderStatus::Running,
            task: String::new(),
            files: vec![],
            backend: None,
            allow_oracle: false,
            kill_requested: false,
            result_path: String::new(),
            agent_id: None,
            created_at: String::new(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: None,
            attempt: 0,
            parent_directive_id: None,
            retry_directive_id: None,
        }
    }

    fn session(id: &str, role: &str, status: &str) -> AgentSession {
        AgentSession {
            agent_id: id.to_string(),
            role: role.to_string(),
            model: None,
            status: status.to_string(),
            message: None,
            client: None,
            current_project_id: None,
            current_task_id: None,
            current_file_path: None,
            first_seen_at: None,
            last_seen_at: None,
            launch_token_hash: None,
            launch_token_issued_at: None,
            session_token_hash: None,
            session_token_issued_at: None,
            subagents: vec![],
            needs_user: None,
            host: None,
            parent_agent_id: None,
            pending_question: None,
            user_reply: None,
        }
    }

    fn read_lines(path: &Path) -> Vec<serde_json::Value> {
        let body = std::fs::read_to_string(path).unwrap_or_default();
        body.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("valid json line"))
            .collect()
    }

    // -- attribute_files matrix --------------------------------------------

    #[test]
    fn mini_beats_coder_same_file() {
        let now = rfc("2026-06-09T12:00:00Z");
        let mut d = directive("d1", "coderA");
        d.files = vec!["src/a.rs".into()];
        d.started_at = Some("2026-06-09T11:59:00Z".into());
        let mut s = session("coderB", "coder", "running");
        s.current_file_path = Some("src/a.rs".into());

        let attr =
            attribute_files_at(&["src/a.rs".into()], &[d], std::slice::from_ref(&s), now);
        assert_eq!(
            attr.get("src/a.rs"),
            Some(&Attribution::Mini {
                directive_id: "d1".into(),
                agent_id: "coderA".into()
            })
        );
    }

    #[test]
    fn window_expiry_terminal_old_not_attributed() {
        let now = rfc("2026-06-09T12:00:00Z");
        let mut d = directive("d1", "coderA");
        d.files = vec!["src/a.rs".into()];
        d.status = MiniCoderStatus::Done;
        // 700s before now -> outside the 600s window.
        d.started_at = Some("2026-06-09T11:48:20Z".into());

        let attr = attribute_files_at(&["src/a.rs".into()], &[d], &[], now);
        assert!(attr.get("src/a.rs").is_none());
    }

    #[test]
    fn terminal_within_window_attributed() {
        let now = rfc("2026-06-09T12:00:00Z");
        let mut d = directive("d1", "coderA");
        d.files = vec!["src/a.rs".into()];
        d.status = MiniCoderStatus::Done;
        d.started_at = Some("2026-06-09T11:55:00Z".into()); // 300s ago, inside window

        let attr = attribute_files_at(&["src/a.rs".into()], &[d], &[], now);
        assert!(matches!(
            attr.get("src/a.rs"),
            Some(Attribution::Mini { .. })
        ));
    }

    #[test]
    fn files_touched_matches_when_files_dont() {
        let now = rfc("2026-06-09T12:00:00Z");
        let mut d = directive("d1", "coderA");
        d.files = vec!["src/other.rs".into()];
        // result.files_touched carries the real edit
        let mut outcome = MiniCoderOutcome::default();
        outcome.status = MiniCoderStatus::Done;
        outcome.files_touched = vec!["src/a.rs".into()];
        d.status = MiniCoderStatus::Done;
        d.result = Some(outcome);
        d.started_at = Some("2026-06-09T11:59:00Z".into());

        let attr = attribute_files_at(&["src/a.rs".into()], &[d], &[], now);
        assert!(matches!(
            attr.get("src/a.rs"),
            Some(Attribution::Mini { directive_id, .. }) if directive_id == "d1"
        ));
    }

    #[test]
    fn two_directives_same_file_most_recent_wins() {
        let now = rfc("2026-06-09T12:00:00Z");
        let mut old = directive("dOld", "coderOld");
        old.files = vec!["src/a.rs".into()];
        old.started_at = Some("2026-06-09T11:50:00Z".into());
        let mut new = directive("dNew", "coderNew");
        new.files = vec!["src/a.rs".into()];
        new.started_at = Some("2026-06-09T11:59:00Z".into());

        let attr = attribute_files_at(&["src/a.rs".into()], &[old, new], &[], now);
        assert_eq!(
            attr.get("src/a.rs"),
            Some(&Attribution::Mini {
                directive_id: "dNew".into(),
                agent_id: "coderNew".into()
            })
        );
    }

    #[test]
    fn coder_fallback_via_session() {
        let now = rfc("2026-06-09T12:00:00Z");
        let mut s = session("coderB", "coder", "running");
        s.current_file_path = Some("src/a.rs".into());
        let attr = attribute_files_at(&["src/a.rs".into()], &[], &[s], now);
        assert_eq!(
            attr.get("src/a.rs"),
            Some(&Attribution::Coder {
                agent_id: "coderB".into()
            })
        );
    }

    #[test]
    fn coder_fallback_sole_active_coder() {
        let now = rfc("2026-06-09T12:00:00Z");
        // No current_file_path match, but a single active coder -> still attributed.
        let s = session("coderSolo", "coder", "running");
        let attr = attribute_files_at(&["src/a.rs".into()], &[], &[s], now);
        assert_eq!(
            attr.get("src/a.rs"),
            Some(&Attribution::Coder {
                agent_id: "coderSolo".into()
            })
        );
    }

    #[test]
    fn no_candidates_absent() {
        let now = rfc("2026-06-09T12:00:00Z");
        // Two coders, neither working src/a.rs -> ambiguous -> no fallback.
        let s1 = session("c1", "coder", "running");
        let s2 = session("c2", "coder", "running");
        let attr = attribute_files_at(&["src/a.rs".into()], &[], &[s1, s2], now);
        assert!(attr.get("src/a.rs").is_none());
    }

    #[test]
    fn closed_coder_not_live() {
        let now = rfc("2026-06-09T12:00:00Z");
        let s = session("cDead", "coder", "closed");
        let attr = attribute_files_at(&["src/a.rs".into()], &[], &[s], now);
        assert!(attr.get("src/a.rs").is_none());
    }

    // -- append concurrency -------------------------------------------------

    #[test]
    fn append_concurrency_no_torn_lines() {
        let dir = tmp("concurrency");
        let path = dir.join("pairs.jsonl");
        const THREADS: usize = 8;
        const PER: usize = 50;

        let mut handles = vec![];
        for t in 0..THREADS {
            let p = path.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..PER {
                    let v = json!({ "t": t, "i": i });
                    append_jsonl(&p, &v).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let lines = read_lines(&path);
        assert_eq!(lines.len(), THREADS * PER);
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- rotation -----------------------------------------------------------

    #[test]
    fn rotation_at_small_cap() {
        let dir = tmp("rotate");
        let path = dir.join("findings.jsonl");
        // cap small so the second append triggers rotation.
        append_jsonl_at(&path, &json!({ "a": 1 }), 5).unwrap();
        // First file now exists and is > 5 bytes -> next append rotates it.
        append_jsonl_at(&path, &json!({ "b": 2 }), 5).unwrap();

        let rotated = rotated_path(&path);
        assert!(rotated.exists(), "rotated .1 file must exist");
        // live file holds only the post-rotation line
        let live = read_lines(&path);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0]["b"], 2);
        let old = read_lines(&rotated);
        assert_eq!(old[0]["a"], 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- gitignore once -----------------------------------------------------

    #[test]
    fn gitignore_written_once() {
        let root = tmp("gitignore");
        let d1 = ensure_training_dir(&root).unwrap();
        let gi = d1.join(".gitignore");
        assert_eq!(std::fs::read_to_string(&gi).unwrap(), "*\n");
        // Overwrite with a sentinel; a second init must NOT rewrite it.
        std::fs::write(&gi, b"SENTINEL").unwrap();
        ensure_training_dir(&root).unwrap();
        assert_eq!(std::fs::read_to_string(&gi).unwrap(), "SENTINEL");
        std::fs::remove_dir_all(&root).ok();
    }

    // -- snapshot_blob ------------------------------------------------------

    #[test]
    fn snapshot_blob_dedupe() {
        let root = tmp("blobdedupe");
        let f = root.join("a.txt");
        std::fs::write(&f, b"hello world").unwrap();
        let h1 = snapshot_blob(&root, &f).unwrap();
        let blob_path = training_dir(&root).join("blobs").join(&h1);
        let mtime1 = std::fs::metadata(&blob_path).unwrap().modified().unwrap();
        // Re-snapshot identical content -> same hash, file not rewritten.
        let h2 = snapshot_blob(&root, &f).unwrap();
        assert_eq!(h1, h2);
        let mtime2 = std::fs::metadata(&blob_path).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "deduped blob must not be rewritten");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn snapshot_blob_over_cap_skipped() {
        let root = tmp("blobcap");
        let f = root.join("big.txt");
        std::fs::write(&f, vec![b'a'; 1000]).unwrap();
        assert!(snapshot_blob_capped(&root, &f, 100).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn snapshot_blob_binary_skipped() {
        let root = tmp("blobbin");
        let f = root.join("bin.dat");
        std::fs::write(&f, b"abc\0def").unwrap();
        assert!(snapshot_blob(&root, &f).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    // -- record_directive_result -------------------------------------------

    #[test]
    fn record_directive_result_emits_line_and_caps_output() {
        let root = tmp("dirresult");
        // a touched file to snapshot
        let touched = "src/x.rs";
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("x.rs"), b"fn x() {}").unwrap();

        let mut d = directive("d9", "coderZ");
        d.files = vec!["src/x.rs".into()];
        d.task = "do the thing".into();
        let mut outcome = MiniCoderOutcome::default();
        outcome.status = MiniCoderStatus::Done;
        outcome.output = Some("X".repeat(500));
        outcome.files_touched = vec![touched.to_string()];

        record_directive_result_capped(&root, &d, &outcome, 100);

        let pairs = training_dir(&root).join("pairs.jsonl");
        let lines = read_lines(&pairs);
        assert_eq!(lines.len(), 1);
        let rec = &lines[0];
        assert_eq!(rec["type"], "directive_result");
        assert_eq!(rec["directiveId"], "d9");
        assert_eq!(rec["parentAgentId"], "coderZ");
        assert_eq!(rec["attempt"], 0);
        assert!(rec["parentDirectiveId"].is_null());
        assert_eq!(rec["status"], "done");
        assert_eq!(rec["output"].as_str().unwrap().chars().count(), 100);
        // blob recorded for the touched file
        assert!(rec["blobs"][touched].is_string());
        // B-F7: a NON-escalated outcome carries no escalation sub-object.
        assert!(rec.get("escalation").is_none(), "no escalation on a done outcome");
        std::fs::remove_dir_all(&root).ok();
    }

    /// B-F7: an ESCALATED outcome's record includes the `escalation { attempts, findings }`
    /// payload — the richest training signal (why the mini chain gave up). Before the fix
    /// it was silently dropped.
    #[test]
    fn record_directive_result_includes_escalation_payload() {
        let root = tmp("dirresult_escal");
        let d = directive("dE", "coderE");
        let escalation = super::super::mini_coder::EscalationInfo {
            attempts: 3,
            findings: vec![super::super::mini_coder::EscalationFinding {
                file: "src/x.rs".into(),
                severity: "high".into(),
                source: "clippy".into(),
                title: "unhandled unwrap".into(),
                line: Some(42),
            }],
        };
        let outcome =
            MiniCoderOutcome::escalated(vec!["src/x.rs".into()], escalation);
        assert_eq!(outcome.status, MiniCoderStatus::Escalated, "fixture is escalated");

        record_directive_result(&root, &d, &outcome);

        let lines = read_lines(&training_dir(&root).join("pairs.jsonl"));
        assert_eq!(lines.len(), 1);
        let rec = &lines[0];
        assert_eq!(rec["status"], "escalated");
        let esc = &rec["escalation"];
        assert!(!esc.is_null(), "escalation sub-object present on an escalated outcome");
        assert_eq!(esc["attempts"], 3, "attempts carried");
        let findings = esc["findings"].as_array().expect("findings array");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0]["file"], "src/x.rs");
        assert_eq!(findings[0]["source"], "clippy");
        assert_eq!(findings[0]["title"], "unhandled unwrap");
        assert_eq!(findings[0]["line"], 42);
        std::fs::remove_dir_all(&root).ok();
    }

    // -- record_findings_batch ---------------------------------------------

    #[test]
    fn record_findings_batch_clean_verdict_and_attribution() {
        let root = tmp("findbatch");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("a.rs"), b"fn a() {}").unwrap();

        let mut d = directive("dA", "coderQ");
        d.files = vec!["src/a.rs".into()];
        d.started_at = Some(Utc::now().to_rfc3339());

        // clean: shard_lookup returns Some(empty) -> zero findings, still attributed.
        let lookup = |_f: &str| -> Option<Vec<FindingLite>> { Some(vec![]) };
        record_findings_batch(&root, &["src/a.rs".into()], lookup, &[d], &[]);

        let findings = read_lines(&training_dir(&root).join("findings.jsonl"));
        assert_eq!(findings.len(), 1, "findings record always written");
        assert_eq!(findings[0]["file"], "src/a.rs");
        assert_eq!(findings[0]["findings"].as_array().unwrap().len(), 0);
        assert_eq!(findings[0]["attribution"]["kind"], "mini");

        let pairs = read_lines(&training_dir(&root).join("pairs.jsonl"));
        assert_eq!(pairs.len(), 1, "clean verdict emitted for attributed file");
        assert_eq!(pairs[0]["type"], "censor_verdict");
        assert_eq!(pairs[0]["openFindings"], 0);
        assert!(pairs[0]["maxSeverity"].is_null());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn record_findings_batch_unattributed_writes_finding_no_verdict() {
        let root = tmp("findbatch2");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("b.rs"), b"fn b() {}").unwrap();

        // dirty findings, but no directive and no coder -> attribution absent.
        let lookup = |_f: &str| -> Option<Vec<FindingLite>> {
            Some(vec![FindingLite {
                id: "f1".into(),
                severity: Severity::High,
                category: "security".into(),
                source: "gitleaks".into(),
                title: "leak".into(),
                line: Some(3),
            }])
        };
        record_findings_batch(&root, &["src/b.rs".into()], lookup, &[], &[]);

        let findings = read_lines(&training_dir(&root).join("findings.jsonl"));
        assert_eq!(findings.len(), 1);
        assert!(findings[0].get("attribution").is_none());
        assert_eq!(findings[0]["findings"][0]["severity"], "high");

        // No attribution -> no censor_verdict line. pairs.jsonl should be empty/absent.
        let pairs_path = training_dir(&root).join("pairs.jsonl");
        assert!(read_lines(&pairs_path).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    // -- failure isolation --------------------------------------------------

    #[test]
    fn failure_isolation_unwritable_dir_no_panic() {
        let base = tmp("failiso");
        // Put a FILE where the .aspis-training dir would go -> create_dir_all fails.
        let blocker = base.join(".aspis-training");
        std::fs::write(&blocker, b"i am a file").unwrap();

        let d = directive("dF", "coderF");
        let mut outcome = MiniCoderOutcome::default();
        outcome.status = MiniCoderStatus::Failed;
        // Must not panic, returns unit.
        record_directive_result(&base, &d, &outcome);
        record_findings_batch(
            &base,
            &["src/z.rs".into()],
            |_f| Some(vec![]),
            &[],
            &[],
        );
        std::fs::remove_dir_all(&base).ok();
    }

    // -- WARNING 6: bounded blob read (TOCTOU / cap) -----------------------

    #[test]
    fn snapshot_blob_exact_cap_captured_over_cap_skipped() {
        // A file EXACTLY at cap is captured; cap+1 is skipped (bounded read).
        let root = tmp("blobcapboundary");
        let at_cap = root.join("at_cap.txt");
        let over_cap = root.join("over_cap.txt");
        std::fs::write(&at_cap, vec![b'a'; 100]).unwrap();
        std::fs::write(&over_cap, vec![b'a'; 101]).unwrap();

        assert!(
            snapshot_blob_capped(&root, &at_cap, 100).is_some(),
            "a file exactly at cap must be captured"
        );
        assert!(
            snapshot_blob_capped(&root, &over_cap, 100).is_none(),
            "a file at cap+1 must be skipped"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // -- WARNING 9: files_touched path traversal guard ---------------------

    #[test]
    fn record_directive_result_skips_traversal_files_touched() {
        // WARNING 9: a result whose files_touched contains `../../secret` must write NO
        // blob for it (the rel-path guard rejects it before snapshot).
        let root = tmp("touchtraversal");
        // Create a "secret" OUTSIDE the project root that the traversal would target.
        let outside = root.parent().unwrap().join(format!(
            "tx_secret_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&outside, b"TOP SECRET").unwrap();
        let secret_name = outside.file_name().unwrap().to_string_lossy().into_owned();
        let traversal = format!("../{secret_name}");

        let mut d = directive("dT", "coderT");
        d.task = "t".into();
        let mut outcome = MiniCoderOutcome::default();
        outcome.status = MiniCoderStatus::Done;
        outcome.files_touched = vec![traversal.clone()];

        record_directive_result(&root, &d, &outcome);

        let pairs = training_dir(&root).join("pairs.jsonl");
        let lines = read_lines(&pairs);
        assert_eq!(lines.len(), 1);
        let blobs = lines[0]["blobs"].as_object().unwrap();
        assert!(
            blobs.is_empty(),
            "traversal files_touched entry must produce NO blob, got: {blobs:?}"
        );
        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    // -- WARNING 10: future-dated terminal directive not attributed --------

    #[test]
    fn future_dated_terminal_directive_not_attributed() {
        // WARNING 10: a terminal directive stamped +300s in the FUTURE (clock skew /
        // crafted) must NOT be attributed (now - t < 0 is out of window).
        let now = rfc("2026-06-09T12:00:00Z");
        let mut d = directive("dFuture", "coderF");
        d.files = vec!["src/a.rs".into()];
        d.status = MiniCoderStatus::Done;
        d.started_at = Some("2026-06-09T12:05:00Z".into()); // +300s future

        let attr = attribute_files_at(&["src/a.rs".into()], &[d], &[], now);
        assert!(
            attr.get("src/a.rs").is_none(),
            "future-dated terminal directive must not be attributed"
        );
    }

    #[test]
    fn active_directive_attributed_regardless_of_future_stamp() {
        // The active-status branch is checked independently: an ACTIVE directive is still
        // attributed even with a future stamp (only terminal ones gate on the window).
        let now = rfc("2026-06-09T12:00:00Z");
        let mut d = directive("dActive", "coderA");
        d.files = vec!["src/a.rs".into()];
        d.status = MiniCoderStatus::Running; // active
        d.started_at = Some("2026-06-09T12:05:00Z".into()); // future stamp

        let attr = attribute_files_at(&["src/a.rs".into()], &[d], &[], now);
        assert!(
            matches!(attr.get("src/a.rs"), Some(Attribution::Mini { .. })),
            "active directive must still be attributed regardless of stamp"
        );
    }

    // -- WARNING 11: path-separator normalization in directive_touches -----

    #[test]
    fn directive_touches_normalizes_separators() {
        // WARNING 11: directive.files uses `\` (Windows) but changed_files uses `/`.
        // Normalization must let them match.
        let now = rfc("2026-06-09T12:00:00Z");
        let mut d = directive("dSep", "coderS");
        d.files = vec!["src\\a.rs".into()];
        d.started_at = Some("2026-06-09T11:59:00Z".into());

        let attr = attribute_files_at(&["src/a.rs".into()], &[d], &[], now);
        assert!(
            matches!(attr.get("src/a.rs"), Some(Attribution::Mini { .. })),
            "backslash vs forward-slash must still attribute"
        );
    }

    // -- NIT 12: cap_chars correctness -------------------------------------

    #[test]
    fn cap_chars_truncates_on_char_boundary() {
        // Multi-byte chars: cap counts CHARS not bytes, and must not split a char.
        assert_eq!(cap_chars("abcdef", 3), "abc");
        assert_eq!(cap_chars("abc", 5), "abc"); // shorter than cap -> unchanged
        assert_eq!(cap_chars("aé😀z", 2), "aé"); // 4 chars; take 2
        assert_eq!(cap_chars("😀😀😀", 1), "😀");
    }

    // -- BLOCKER 3: shared findings.jsonl lock (single registry) -----------

    #[test]
    fn append_findings_line_shares_lock_with_internal_registry() {
        // BLOCKER 3: the public `append_findings_line` path and api_fuzz both must resolve
        // to the SAME per-path Arc<Mutex<()>> as the internal registry. Assert the test
        // hook returns the same Arc pointer the internal `lock_for` would.
        let dir = tmp("sharedlock");
        let path = dir.join(".aspis-training").join("findings.jsonl");
        let a = lock_for_path_test_hook(&path);
        let b = lock_for_path_test_hook(&path);
        assert!(
            Arc::ptr_eq(&a, &b),
            "same path must resolve to the SAME lock Arc (one registry)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
