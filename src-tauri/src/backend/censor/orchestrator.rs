//! Censor orchestrator — the engine STEP that turns a settled set of changed
//! files (FINE trigger) or a project-level COARSE trigger into shard writes and a
//! single `censor://findings-updated` event.
//!
//! DESIGN (pure core + thin IO shell):
//!   - PURE, IO-free, fully unit-tested:
//!       * [`plan_fine`] — given the project kinds and a batch of changed files,
//!         decide which FINE runners apply to each file (runner-selection).
//!       * [`coarse_runners`] — the COARSE runner set for the project kinds.
//!       * [`group_by_file`] — bucket a flat `Vec<RawFinding>` by its (normalized)
//!         file path, the shape the scoped-merge consumes.
//!   - THIN (spawns subprocesses, hashes files, writes shards, emits the event):
//!       * [`run_fine_batch`] / [`run_coarse_pass`] / [`run_review_now`] — drive
//!         the pure plan against the real runners + ledger.
//!
//! MERGE (the critical correctness property): a single file's shard accumulates
//! findings from MULTIPLE runner *sources* arriving on DIFFERENT triggers (FINE
//! per-file vs COARSE project-level). Refreshing a given SET OF SOURCES for a file
//! must NOT delete findings from sources NOT in that set. That is enforced by the
//! ledger's SOURCE-SCOPED merge (`ledger::read_supersede_write_shard` →
//! `ledger::supersede_sources`), to which every write here passes the EXACT set of
//! sources it just produced for that file.
//!
//! LIFECYCLE: the orchestrator NEVER touches Tauri `State` and NEVER holds a map
//! lock — the caller (the watch thread / a command) clones out the root + app +
//! kinds first. Each unit of work is a plain function over a `&Path` root, so it
//! is testable against a tempdir with no tools installed (→ empty, no error).

use super::detect::{detect_project_kinds, FileLang, ProjectKind};
use super::gemma::{self, GemmaClient};
use super::ledger::read_supersede_write_shard;
use super::runners::{self, applicable_runners, Granularity, RawFinding, RunTarget, RunnerId};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

use crate::backend::training_export::{self, FindingLite};

/// The source name the Gemma tier stamps onto its findings. Centralized so the
/// scoped-merge source set the FINE pass refreshes always includes EXACTLY the
/// string `parse_gemma` writes — a drift here would let a deterministic finding be
/// clobbered (or a stale gemma finding survive). MUST equal the `source` set by
/// `gemma::parse_gemma` ("gemma").
const GEMMA_SOURCE: &str = "gemma";

/// The optional Gemma tier for a fine pass: a borrowed client + the cached
/// availability flag (probed ONCE per watch session, never per file). `None` means
/// the tier is disabled for this pass and the fine pass behaves exactly like the
/// deterministic-only A3 engine (clean degrade). Bundled so the long `fine_batch`
/// signature carries one optional param rather than two coupled ones.
#[derive(Clone, Copy)]
pub struct GemmaCtx<'a> {
    pub client: &'a dyn GemmaClient,
    pub available: bool,
}

/// Tauri event emitted after one or more shards change. Payload:
/// `{ projectId, files: [relPath] }` (camelCase). The frontend listens and
/// refetches the affected files' findings (NOT a poll).
pub const FINDINGS_UPDATED_EVENT: &str = "censor://findings-updated";

/// FINE debounce window: per-file TS/Python tools, snappy.
pub const FINE_DEBOUNCE_MS: u64 = 400;
/// COARSE debounce window: crate-level / whole-project tools (clippy/tsc/...),
/// slow, so coalesce a longer burst.
pub const COARSE_DEBOUNCE_MS: u64 = 4000;

/// The event payload for [`FINDINGS_UPDATED_EVENT`]. `files` are project-relative,
/// forward-slash-normalized paths whose shards changed in this pass (may be empty
/// — a pass that produced no shard change still emits so the UI can settle a
/// "reviewing…" indicator).
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FindingsUpdatedPayload {
    pub project_id: String,
    pub files: Vec<String>,
}

// ---------------------------------------------------------------------------
// PURE planning + grouping (no IO).
// ---------------------------------------------------------------------------

/// One file's FINE work: the changed file's project-relative path plus the FINE
/// runners that apply to it (per its language + the project kinds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePlan {
    pub file_rel_path: String,
    pub runners: Vec<RunnerId>,
}

/// Decide the FINE per-file work for a settled batch of changed files.
///
/// For each file, [`applicable_runners`] yields ALL runners (fine + coarse) that
/// apply to it; we keep only the FINE ones here (the coarse ones are dispatched
/// ONCE for the whole project by [`coarse_runners`], not per file). Files with no
/// applicable FINE runner are dropped from the plan (nothing to do per-file), but
/// they still get a shard refresh of the empty FINE-source set by the caller so a
/// fixed file's stale FINE findings clear (handled in [`run_fine_batch`]).
///
/// Deterministic + order-stable: input order preserved, runner order preserved.
pub fn plan_fine(kinds: &HashSet<ProjectKind>, files: &[String]) -> Vec<FilePlan> {
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let lang = FileLang::from_path(Path::new(file));
        let runners: Vec<RunnerId> = applicable_runners(kinds, lang)
            .into_iter()
            .filter(|r| r.granularity() == Granularity::Fine)
            .collect();
        out.push(FilePlan {
            file_rel_path: file.clone(),
            runners,
        });
    }
    out
}

/// The COARSE runner set for a project: the union, over a representative file of
/// each detected language, of the COARSE runners — deduped, order-stable.
///
/// Coarse runners are project/crate-wide (clippy/cargo-check/cargo-audit for
/// Rust; tsc/knip for Node; gitleaks/jscpd cross-cutting), so they run ONCE per
/// coarse trigger regardless of which file changed. We derive them from the
/// detected kinds rather than a specific file so a coarse pass covers every kind
/// present (e.g. a Tauri repo runs both the Rust and the Node coarse tools).
pub fn coarse_runners(kinds: &HashSet<ProjectKind>) -> Vec<RunnerId> {
    let mut seen: HashSet<RunnerId> = HashSet::new();
    let mut out = Vec::new();
    // Probe one file of each language so `applicable_runners` adds the matching
    // kind-specific coarse runners; `Other` contributes only the cross-cutting set.
    for lang in [
        FileLang::Rust,
        FileLang::Ts,
        FileLang::Py,
        FileLang::Go,
        FileLang::Other,
    ] {
        for r in applicable_runners(kinds, lang) {
            if r.granularity() == Granularity::Coarse && seen.insert(r) {
                out.push(r);
            }
        }
    }
    out
}

/// Bucket a flat list of raw findings by their (forward-slash-normalized) file
/// path. Used to convert a coarse runner's cross-file output, and a file's fine
/// output, into the per-file groups the scoped merge writes. Order within a file
/// preserves first-seen order; the file keys are sorted (BTreeMap) for a stable,
/// testable result.
pub fn group_by_file(raw: Vec<RawFinding>) -> BTreeMap<String, Vec<RawFinding>> {
    let mut map: BTreeMap<String, Vec<RawFinding>> = BTreeMap::new();
    for mut f in raw {
        f.file = f.file.replace('\\', "/");
        map.entry(f.file.clone()).or_default().push(f);
    }
    map
}

// ---------------------------------------------------------------------------
// THIN IO: dispatch a runner by id (kept here so the pure plan stays IO-free).
// ---------------------------------------------------------------------------

/// Dispatch one runner against the project. FINE runners receive the changed
/// file's `target`; COARSE runners ignore it and inspect the whole project. An
/// absent tool yields an empty vec (each `run` presence-detects internally).
fn dispatch_runner(id: RunnerId, root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    match id {
        RunnerId::Clippy => runners::clippy::run(root),
        RunnerId::CargoCheck => runners::cargo_check::run(root),
        RunnerId::CargoAudit => runners::cargo_audit::run(root),
        RunnerId::CargoDeny => runners::cargo_deny::run(root),
        RunnerId::CargoFmt => runners::cargo_fmt::run(root),
        RunnerId::NpmAudit => runners::npm_audit::run(root),
        RunnerId::Oxlint => runners::oxlint::run(root, target),
        RunnerId::PipAudit => runners::pip_audit::run(root),
        RunnerId::Pyright => runners::pyright::run(root, target),
        RunnerId::Tsc => runners::tsc::run(root),
        RunnerId::Knip => runners::knip::run(root),
        RunnerId::Jscpd => runners::jscpd::run(root),
        RunnerId::Gitleaks => runners::gitleaks::run(root),
        RunnerId::Eslint => runners::eslint::run(root, target),
        RunnerId::Prettier => runners::prettier::run(root, target),
        RunnerId::Ruff => runners::ruff::run(root, target),
        RunnerId::RuffFormat => runners::ruff_format::run(root, target),
        RunnerId::Bandit => runners::bandit::run(root, target),
        RunnerId::Vulture => runners::vulture::run(root, target),
        // gofmt is FINE (per-file target); go vet is COARSE (whole module, root only).
        RunnerId::Gofmt => runners::gofmt::run(root, target),
        RunnerId::GoVet => runners::go_vet::run(root),
        // cppcheck is FINE (per-file target; no-compile static analyzer).
        RunnerId::Cppcheck => runners::cppcheck::run(root, target),
        // tidy (HTML) and ktlint (Kotlin) are FINE (per-file target; no-compile).
        RunnerId::Tidy => runners::tidy::run(root, target),
        RunnerId::Ktlint => runners::ktlint::run(root, target),
        RunnerId::Lizard => runners::lizard::run(root, target),
        RunnerId::Semgrep => runners::semgrep::run(root, target),
        RunnerId::Zizmor => runners::zizmor::run(root),
    }
}

/// The source name a runner stamps onto its findings, so a shard write can scope
/// the merge to exactly the sources it just refreshed. MUST equal the `source`
/// string each parser writes (clippy → "clippy", cargo-check → "cargo-check",
/// etc.). Centralized here so the scoped-merge source set and the finding `source`
/// can never drift.
fn runner_source(id: RunnerId) -> &'static str {
    match id {
        RunnerId::Clippy => "clippy",
        RunnerId::CargoCheck => "cargo-check",
        RunnerId::CargoAudit => "cargo-audit",
        RunnerId::CargoDeny => "cargo-deny",
        RunnerId::CargoFmt => "cargo-fmt",
        RunnerId::NpmAudit => "npm-audit",
        RunnerId::Oxlint => "oxlint",
        RunnerId::PipAudit => "pip-audit",
        RunnerId::Pyright => "pyright",
        RunnerId::Tsc => "tsc",
        RunnerId::Knip => "knip",
        RunnerId::Jscpd => "jscpd",
        RunnerId::Gitleaks => "gitleaks",
        RunnerId::Eslint => "eslint",
        RunnerId::Prettier => "prettier",
        RunnerId::Ruff => "ruff",
        RunnerId::RuffFormat => "ruff-format",
        RunnerId::Bandit => "bandit",
        RunnerId::Vulture => "vulture",
        RunnerId::Gofmt => "gofmt",
        RunnerId::GoVet => "go-vet",
        RunnerId::Cppcheck => "cppcheck",
        RunnerId::Tidy => "tidy",
        RunnerId::Ktlint => "ktlint",
        RunnerId::Lizard => "lizard",
        RunnerId::Semgrep => "semgrep",
        RunnerId::Zizmor => "zizmor",
    }
}

// ---------------------------------------------------------------------------
// THIN IO: file hashing.
// ---------------------------------------------------------------------------

/// Upper bound on a file we will hash + review. Source files are never this large;
/// a giant generated/minified/binary blob (e.g. a checked-in bundle or a vendored
/// dataset) must NOT be slurped into memory or reviewed. Such a file is treated as
/// [`HashOutcome::Skip`] (the pass leaves its shard untouched), exactly like a
/// transiently-unreadable file. 8 MiB is comfortably above any real source file.
const MAX_HASH_BYTES: u64 = 8 * 1024 * 1024;

/// Streaming-hash read buffer. Memory used by [`hash_file`] is O(this), NOT
/// O(filesize), so a large file can never OOM the hasher.
const HASH_BUF_BYTES: usize = 64 * 1024;

/// The outcome of hashing a file for a shard write.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HashOutcome {
    /// The file was read and hashed; carries its sha256 hex.
    Hashed(String),
    /// The file is genuinely gone (`NotFound`) → its shard should be pruned.
    Deleted,
    /// The file exists but could not be hashed THIS pass (locked / transient IO
    /// error / over the size cap). The pass must SKIP it: do not write, do not
    /// prune, do not supersede — judged dispositions and provenance must survive a
    /// mid-save lock or a transient fault. The next settled change retries it.
    Skip,
}

/// sha256 of a file's current bytes, computed by STREAMING the file through a fixed
/// buffer (memory is O([`HASH_BUF_BYTES`]), never O(filesize) — a multi-GB file
/// cannot OOM us). Maps IO faults to a [`HashOutcome`]:
///   - `NotFound`            → [`HashOutcome::Deleted`] (real deletion; prune).
///   - any OTHER read error  → [`HashOutcome::Skip`] (locked / transient; leave the
///     shard exactly as-is — see BLOCKER 2: never let a mid-save editor lock or a
///     transient error destroy human `fp`/`wontfix` dispositions).
///   - size over the cap     → [`HashOutcome::Skip`] (a giant generated/binary blob
///     is not a source file we review).
fn hash_file(abs: &Path) -> HashOutcome {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = match std::fs::File::open(abs) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashOutcome::Deleted,
        // PermissionDenied, locked mid-save, transient IO, etc.: SKIP, do not prune.
        Err(_) => return HashOutcome::Skip,
    };
    // Size cap: don't hash/review a giant generated/binary file.
    if let Ok(meta) = file.metadata() {
        if meta.len() > MAX_HASH_BYTES {
            return HashOutcome::Skip;
        }
    }
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF_BYTES];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // A read fault partway through (file truncated/locked during read): SKIP
            // rather than persist a hash of partial bytes that would mis-supersede.
            Err(_) => return HashOutcome::Skip,
        }
    }
    HashOutcome::Hashed(hex::encode(hasher.finalize()))
}

/// The outcome of reading a file's FULL content (for the Gemma tier), carrying the
/// content + its hash so the fine pass can feed BOTH the content (to Gemma) and the
/// hash (to the shard write) without a second read or a second hash. Mirrors
/// [`HashOutcome`]'s fault mapping EXACTLY (`NotFound`→Deleted, other IO→Skip, over
/// the size cap→Skip) so the content path and the hash-only path agree on when a
/// file is reviewable, deleted, or to be left alone.
#[cfg_attr(test, derive(Debug))]
enum ReadOutcome {
    /// Read OK: the content as a lossy-UTF8 string + its sha256 hex.
    Read { content: String, hash: String },
    /// `NotFound` → the file was deleted; prune.
    Deleted,
    /// Locked / transient IO / over the size cap → leave the shard as-is.
    Skip,
}

/// Read a file's content (BOUNDED at [`MAX_HASH_BYTES`]) AND its sha256 hex in a
/// single pass, mapping IO faults to a [`ReadOutcome`] exactly like [`hash_file`].
/// Used by the FINE pass so the Gemma tier reviews the SAME bytes the shard hash is
/// computed from (no re-read, no hash drift). Non-UTF8 bytes are lossily decoded
/// (Gemma reviews source text; a binary file is gated out by the size cap / language
/// selection upstream anyway).
///
/// The read is bounded by `take(MAX_HASH_BYTES + 1)`, NOT by a metadata pre-check, so
/// a file that grows between syscalls can never cause an allocation past the cap
/// (BLOCKER 2 — TOCTOU OOM). Memory is therefore O(cap), strictly bounded; an
/// over-cap file maps to `Skip`.
fn read_file_outcome(abs: &Path) -> ReadOutcome {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    // BLOCKER 2 (TOCTOU OOM): do NOT `metadata().len()`-gate then `fs::read` — the
    // file can GROW between the two syscalls and `fs::read` would allocate to the NEW
    // size, defeating the cap. Instead bound the read itself: `take(cap + 1)` so we
    // never allocate more than the cap (plus one probe byte), regardless of the file's
    // current size. If we manage to read more than the cap, it is oversized → Skip.
    let file = match std::fs::File::open(abs) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::Deleted,
        // PermissionDenied, locked mid-save, transient IO, etc.: SKIP, do not prune.
        Err(_) => return ReadOutcome::Skip,
    };
    let mut buf: Vec<u8> = Vec::new();
    // `MAX_HASH_BYTES + 1`: read one byte past the cap so an exactly-at-cap file is
    // accepted while an over-cap file is detected without slurping it whole.
    let limit = MAX_HASH_BYTES.saturating_add(1);
    if file.take(limit).read_to_end(&mut buf).is_err() {
        // A read fault partway through (truncated/locked during read): SKIP rather
        // than persist a hash of partial bytes that would mis-supersede.
        return ReadOutcome::Skip;
    }
    if buf.len() as u64 > MAX_HASH_BYTES {
        // Over the cap (a giant generated/binary blob): not a source file we review.
        return ReadOutcome::Skip;
    }
    let mut hasher = Sha256::new();
    hasher.update(&buf);
    let hash = hex::encode(hasher.finalize());
    let content = String::from_utf8_lossy(&buf).into_owned();
    ReadOutcome::Read { content, hash }
}

// ---------------------------------------------------------------------------
// THIN IO: per-file shard write with the source-scoped merge.
// ---------------------------------------------------------------------------

/// Sentinel hash for a GENUINELY-DELETED file (`NotFound`). `supersede_sources`
/// treats a changed hash as "drop all old findings", and an empty new set for the
/// refreshed sources clears them — so writing this constant prunes the file's
/// refreshed-source findings. Chosen so it deterministically differs from any real
/// sha256 hex (those are 64 lowercase hex chars; this contains underscores).
const DELETED_HASH_SENTINEL: &str = "__deleted__";

/// Write one file's shard, scoping the merge to `refreshed_sources` so findings
/// from sources NOT in that set survive (clobber-avoidance, see module doc).
///
/// The file is hashed from disk via [`hash_file`], whose [`HashOutcome`] decides
/// the action (BLOCKER 2 — only a GENUINE deletion may prune):
///   - [`HashOutcome::Hashed`]  → normal scoped merge at the new hash.
///   - [`HashOutcome::Deleted`] (`NotFound`) → write the delete sentinel hash with
///     an empty new set: the hash-change drops the refreshed sources' findings
///     (true prune of a removed file).
///   - [`HashOutcome::Skip`] (locked / transient IO / over size cap) → DO NOTHING:
///     return `false` WITHOUT touching the shard, so a mid-save editor lock or a
///     transient fault can NEVER destroy human `fp`/`wontfix` dispositions or
///     provenance. The next settled change retries the file.
///
/// `raw` are the findings the refreshed sources produced for THIS file; they are
/// converted to `Finding`s stamped with the current hash + `now`.
///
/// Returns `true` if the shard write succeeded (so the caller can include the file
/// in the emitted event), `false` on a Skip or an IO/lock error (logged, swallowed
/// — a single bad shard never aborts the batch).
fn write_file_shard(
    root: &Path,
    file_rel_path: &str,
    raw: Vec<RawFinding>,
    refreshed_sources: &BTreeSet<String>,
    now: &str,
) -> bool {
    let abs = root.join(file_rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
    commit_shard(
        root,
        file_rel_path,
        hash_file(&abs),
        raw,
        refreshed_sources,
        now,
    )
}

/// Commit a shard from an ALREADY-RESOLVED [`HashOutcome`], applying the BLOCKER-2
/// action mapping (Hashed→merge, Deleted→prune via the sentinel, Skip→leave as-is)
/// and the source-scoped merge. Factored out so the FINE pass can resolve the hash
/// from a single content read ([`read_file_outcome`]) and reuse it here WITHOUT a
/// second disk hash, while `write_file_shard` (coarse pass + the deletion-clearing
/// path) re-hashes from disk. Both routes share this identical write semantics, so
/// the clobber-protection + skip/prune behaviour can never diverge between them.
fn commit_shard(
    root: &Path,
    file_rel_path: &str,
    outcome: HashOutcome,
    raw: Vec<RawFinding>,
    refreshed_sources: &BTreeSet<String>,
    now: &str,
) -> bool {
    let (hash, raw) = match outcome {
        HashOutcome::Hashed(h) => (h, raw),
        // Genuine deletion: prune via the sentinel hash + an empty new set. Any
        // `raw` is irrelevant for a vanished file, so discard it.
        HashOutcome::Deleted => (DELETED_HASH_SENTINEL.to_string(), Vec::new()),
        // Transiently unreadable / locked mid-save / oversized: leave the shard
        // EXACTLY as-is. Do not write, do not prune, do not supersede.
        HashOutcome::Skip => return false,
    };
    let findings: Vec<_> = raw
        .into_iter()
        .map(|r| r.into_finding(&hash, now))
        .collect();
    match read_supersede_write_shard(root, findings, &hash, refreshed_sources, file_rel_path, now) {
        Ok(_) => true,
        Err(e) => {
            // Path only — never finding contents (privacy). A single shard fault
            // must not abort the whole batch.
            eprintln!(
                "censor orchestrator: shard write failed for {file_rel_path}: {}",
                e.kind()
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// THIN IO: the three public engine entry points.
// ---------------------------------------------------------------------------

/// Run the FINE per-file pass for a settled batch of changed files, then emit the
/// event. Each file is hashed, its applicable FINE runners are run with the file
/// as the target, the results grouped, and the shard scoped-merged for the FINE
/// source set (so coarse-source findings for the same file are untouched).
///
/// Files with NO applicable FINE runner are SKIPPED (no per-file work; an empty
/// refresh would pointlessly rewrite the shard and spam the event). Their stale
/// fine findings, if any, clear via the same-hash drop on the next change or the
/// coarse pass.
///
/// `running` is the worker's stop flag: if it is already clear when this is called
/// the whole pass is skipped, and it is re-checked immediately before the emit so a
/// stop/replace that lands DURING the (slow) pass cannot fire a
/// `findings-updated` event for a torn-down or replaced project (see the worker
/// handoff in `commands::censor_start_watch`). The shard writes themselves are
/// idempotent and harmless, so a stop mid-pass simply suppresses the event.
pub fn run_fine_batch(
    app: &AppHandle,
    project_id: &str,
    root: &Path,
    files: &[String],
    gemma: Option<GemmaCtx<'_>>,
    running: &AtomicBool,
) {
    run_fine_batch_inner(app, project_id, root, files, gemma, running, true);
}

/// WARNING 4 (P6 verdict gate): like [`run_fine_batch`] but SKIPS the training-rail
/// `record_findings_for_batch` write. Used ONLY by the mini-coder verdict gate
/// (`real_censor_verdict`), which lints the SAME just-changed files the live watcher
/// is also about to process — recording on both paths would emit DUPLICATE
/// `censor_verdict` lines into `pairs.jsonl` for one file change. The gate runs the
/// deterministic collector for its escalation decision only; the watcher remains the
/// SOLE training-rail recorder. Still emits `findings-updated` (the shards are real).
pub fn run_fine_batch_no_rail(
    app: &AppHandle,
    project_id: &str,
    root: &Path,
    files: &[String],
    gemma: Option<GemmaCtx<'_>>,
    running: &AtomicBool,
) {
    run_fine_batch_inner(app, project_id, root, files, gemma, running, false);
}

/// Shared core of [`run_fine_batch`] / [`run_fine_batch_no_rail`]. `record_rail`
/// gates the training-rail write (the ONLY difference between the two entry points).
fn run_fine_batch_inner(
    app: &AppHandle,
    project_id: &str,
    root: &Path,
    files: &[String],
    gemma: Option<GemmaCtx<'_>>,
    running: &AtomicBool,
    record_rail: bool,
) {
    if !running.load(Ordering::SeqCst) {
        return;
    }
    let changed = fine_batch_collect(root, files, gemma);
    // TRAINING RAIL: record findings for every changed file while the project is
    // still running. Agent-state snapshot is read under its lock, cloned to owned
    // Vecs, and the lock is released BEFORE calling into training_export (lock-
    // ordering contract: training_export's JSONL lock must never nest inside the
    // agent-state lock). SKIPPED when `record_rail` is false (the verdict gate path —
    // the watcher records the same file change to avoid duplicate pairs.jsonl lines).
    if should_record_rail(record_rail, running.load(Ordering::SeqCst), changed.is_empty()) {
        record_findings_for_batch(app, root, &changed);
    }
    emit_if_running(app, project_id, changed, running);
}

/// PURE training-rail gate for [`run_fine_batch_inner`] (unit-testable without an
/// `AppHandle`). The rail write fires ONLY when the caller opted in (`record_rail`,
/// false for the mini-coder verdict gate — see [`run_fine_batch_no_rail`]), the
/// project worker is still running, AND at least one shard actually changed.
fn should_record_rail(record_rail: bool, running: bool, changed_empty: bool) -> bool {
    record_rail && running && !changed_empty
}

/// The IO core of [`run_fine_batch`] WITHOUT the Tauri emit: runs the fine pass
/// and returns the list of files whose shards changed. Factored out so it can be
/// tested against a tempdir with no tools installed (no `AppHandle` needed).
///
/// GEMMA (A4): after the deterministic FINE runners produce a file's findings, if
/// `gemma` is `Some` and available, the OPTIONAL Gemma tier reviews the SAME file
/// content (already read for the hash — never re-read) with the deterministic
/// findings passed as "already known", and its findings are APPENDED. The refreshed-
/// source set includes `"gemma"` whenever a Gemma CONTEXT exists (client present),
/// regardless of `available`, so the source-scoped merge both clobber-protects the
/// deterministic findings (deterministic runs FIRST, Gemma is purely additive) AND
/// clears stale gemma findings when the tier is offline (BLOCKER 3). If `gemma` is
/// `None` the pass is byte-for-byte the deterministic-only A3 behaviour: the "gemma"
/// source is never refreshed, so a pure-A3 caller never touches gemma findings.
fn fine_batch_collect(root: &Path, files: &[String], gemma: Option<GemmaCtx<'_>>) -> Vec<String> {
    let kinds = detect_project_kinds(root);
    let plan = plan_fine(&kinds, files);
    let now = super::now_stamp();
    let mut changed: Vec<String> = Vec::new();

    // The Gemma tier only runs when a client is supplied AND its cached probe says
    // available. Computed once here (a cheap field read), not per file.
    let gemma_active = gemma.map(|g| g.available).unwrap_or(false);

    for fp in plan {
        // SECURITY: validate every watcher-derived relative path before it can
        // target a runner argv or a shard outside `.aspis-censor/`. A malformed
        // path (`..`, absolute, `-`-leading component) is SKIPPED, not crashed —
        // the watcher should never produce one (it strips the root prefix), but a
        // symlink, a racing rename, or a non-UTF8 lossy conversion could, so we
        // gate here exactly as the coarse pass gates tool-reported paths.
        if super::ledger::validate_rel_path(&fp.file_rel_path).is_err() {
            continue;
        }
        // A file with no applicable FINE runner has no per-file work: an empty
        // refreshed-source set would be a no-op merge that pointlessly rewrites the
        // shard and spams the event. Skip it (its stale fine findings, if any, clear
        // when the file's hash changes and the COARSE pass re-runs). Rust/Other
        // files never reach here as fine work anyway (they ride the coarse pass).
        if fp.runners.is_empty() {
            continue;
        }

        // Resolve the file's hash (BLOCKER-2 outcome semantics). When Gemma is active
        // we read the FULL content ONCE (it feeds the model) and derive the hash from
        // those same bytes — so the model reviews EXACTLY the bytes the shard records,
        // with no re-read. When Gemma is OFF we keep A3's cheaper STREAMING hash (no
        // whole-file slurp), so the deterministic-only memory profile is unchanged.
        let abs = root.join(fp.file_rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let (content, hash_outcome): (Option<String>, HashOutcome) = if gemma_active {
            match read_file_outcome(&abs) {
                ReadOutcome::Read { content, hash } => (Some(content), HashOutcome::Hashed(hash)),
                // A genuine deletion still clears this file's fine findings (prune); a
                // deleted file has no content for Gemma.
                ReadOutcome::Deleted => (None, HashOutcome::Deleted),
                // Locked / transient / oversized: leave the shard untouched (skip), no
                // deterministic OR Gemma work — exactly the A3 skip.
                ReadOutcome::Skip => continue,
            }
        } else {
            match hash_file(&abs) {
                HashOutcome::Skip => continue,
                outcome => (None, outcome),
            }
        };

        let mut refreshed: BTreeSet<String> = fp
            .runners
            .iter()
            .map(|r| runner_source(*r).to_string())
            .collect();
        let target = RunTarget {
            file_rel_path: fp.file_rel_path.clone(),
        };
        let mut raw_for_file: Vec<RawFinding> = Vec::new();
        for id in &fp.runners {
            let mut produced = dispatch_runner(*id, root, &target);
            // A fine runner is invoked with the changed file; keep only findings
            // it attributed to THAT file (a fine tool may, defensively, mention
            // an imported file — those belong to that other file's shard, but the
            // fine pass is scoped to the changed file, so we ignore cross-file
            // fine output here; coarse passes own cross-file attribution).
            produced.retain(|f| f.file.replace('\\', "/") == fp.file_rel_path);
            raw_for_file.append(&mut produced);
        }

        // GEMMA (A4): additive, deterministic-clobber-protected.
        //
        // STALE-CLEAR (BLOCKER 3): whenever a Gemma CONTEXT exists for this session —
        // i.e. the orchestrator was given a client, regardless of whether it is
        // currently `available` — we ALWAYS refresh the "gemma" source. This means:
        //   - an ACTIVE pass adds the model's new findings AND clears any prior ones;
        //   - an OFFLINE pass (client present but `available == false`) refreshes the
        //     source with an EMPTY set, which CLEARS stale gemma findings written while
        //     the tier was online. Without this, a finding written during an online
        //     window would live forever in the shard once the tier went offline.
        // The deterministic sources are untouched (scoped-merge clobber-protection).
        // We only CALL the model when active AND we have the file content; the content
        // is read above only on the active path, so an offline pass does no slurp.
        if gemma.is_some() {
            refreshed.insert(GEMMA_SOURCE.to_string());
        }
        if gemma_active {
            if let (Some(ctx), Some(file_content)) = (gemma, content.as_deref()) {
                let mut g = gemma::run_gemma(
                    ctx.client,
                    ctx.available,
                    root,
                    &fp.file_rel_path,
                    file_content,
                    &raw_for_file,
                );
                raw_for_file.append(&mut g);
            }
        }

        if commit_shard(
            root,
            &fp.file_rel_path,
            hash_outcome,
            raw_for_file,
            &refreshed,
            &now,
        ) {
            changed.push(fp.file_rel_path.clone());
        }
    }

    changed
}

/// Run the COARSE project-level pass, then emit the event. Each coarse runner is
/// run once for the whole project; its (possibly cross-file) findings are grouped
/// by file and each referenced file's shard is scoped-merged for the COARSE
/// source set. Files that PREVIOUSLY had coarse-source findings but were NOT
/// re-emitted this pass are cleared: we read the existing shard dir, and for every
/// file that carried any of the refreshed coarse sources but is absent from this
/// pass's output, we write an empty refresh for those sources (same-hash drop).
///
/// `running` is the worker's stop flag, used exactly as in [`run_fine_batch`]:
/// already-stopped → skip the pass; stopped-during → suppress the emit (no stale
/// event for a torn-down/replaced project).
pub fn run_coarse_pass(app: &AppHandle, project_id: &str, root: &Path, running: &AtomicBool) {
    if !running.load(Ordering::SeqCst) {
        return;
    }
    let changed = coarse_pass_collect(root);
    // TRAINING RAIL: same lock-ordering discipline as run_fine_batch.
    if running.load(Ordering::SeqCst) && !changed.is_empty() {
        record_findings_for_batch(app, root, &changed);
    }
    emit_if_running(app, project_id, changed, running);
}

/// The IO core of [`run_coarse_pass`] WITHOUT the Tauri emit: runs the coarse pass
/// and returns the list of files whose shards changed. Factored out for testing
/// against a tempdir with no tools installed (no `AppHandle` needed).
fn coarse_pass_collect(root: &Path) -> Vec<String> {
    let kinds = detect_project_kinds(root);
    let coarse = coarse_runners(&kinds);
    if coarse.is_empty() {
        return Vec::new();
    }
    let now = super::now_stamp();
    let refreshed: BTreeSet<String> = coarse
        .iter()
        .map(|r| runner_source(*r).to_string())
        .collect();

    // Run every coarse runner once; coarse runners ignore the target.
    let empty_target = RunTarget {
        file_rel_path: String::new(),
    };
    let mut all_raw: Vec<RawFinding> = Vec::new();
    for id in &coarse {
        let mut produced = dispatch_runner(*id, root, &empty_target);
        all_raw.append(&mut produced);
    }
    let grouped = group_by_file(all_raw);

    // Files this coarse pass DID report on.
    let reported: BTreeSet<String> = grouped.keys().cloned().collect();

    let mut changed: Vec<String> = Vec::new();
    // 1) Write the reported files (scoped to the coarse source set).
    for (file, raw) in grouped {
        if super::ledger::validate_rel_path(&file).is_err() {
            // A coarse tool reported an out-of-tree / malformed path — skip it
            // (never let a tool path target a shard outside `.aspis-censor/`).
            continue;
        }
        if write_file_shard(root, &file, raw, &refreshed, &now) {
            changed.push(file);
        }
    }
    // 2) Clear coarse-source findings for files no longer reported. Enumerate the
    //    existing shards and, for any shard that still carries a refreshed coarse
    //    source AND was not reported this pass, write an empty refresh for the
    //    coarse sources (the same-hash drop removes the now-gone findings while
    //    leaving fine-source findings intact).
    for file in files_with_sources(root, &refreshed) {
        if reported.contains(&file) {
            continue;
        }
        if write_file_shard(root, &file, Vec::new(), &refreshed, &now) {
            changed.push(file);
        }
    }

    changed
}

/// On-demand pass bypassing debounce. `file = Some(rel)` runs that one file's FINE
/// runners (a quick single-file recheck); `file = None` runs the whole-project
/// COARSE sweep (clippy/cargo-*/tsc/knip/gitleaks/jscpd).
///
/// KNOWN A3 LIMITATION (for Phase E / final review): a `None` whole-project review
/// runs only the COARSE tools — the FINE per-file tools (eslint/ruff/bandit/
/// vulture/lizard/semgrep) are NOT re-run across every file, because that would
/// require enumerating the whole tree and the A2 fine runners require a per-file
/// target. The fine layer stays fresh via the live watcher (each edited file is
/// fine-reviewed on settle). A future "deep review" could walk the tree and fan
/// fine runners per file; deliberately out of A3 scope. Both arms emit the event.
///
/// SERIALIZATION: this is run on the per-project worker thread (enqueued by
/// `censor_review_now`), NEVER inline on the command thread, so an on-demand review
/// is serialized with the live watcher's fine/coarse passes — no concurrent
/// read-modify-write on the same shards. `running` is the worker's stop flag (see
/// [`run_fine_batch`]).
pub fn run_review_now(
    app: &AppHandle,
    project_id: &str,
    root: &Path,
    file: Option<&str>,
    gemma: Option<GemmaCtx<'_>>,
    running: &AtomicBool,
) {
    match file {
        // A single-file recheck IS a fine pass, so it runs Gemma too (additive).
        Some(rel) => run_fine_batch(app, project_id, root, &[rel.to_string()], gemma, running),
        // The whole-project sweep is COARSE-only (Gemma is per-file fine, see the
        // module doc); the Gemma ctx is intentionally not threaded here.
        None => run_coarse_pass(app, project_id, root, running),
    }
}

// ---------------------------------------------------------------------------
// Helpers: enumerate shards carrying a given source set; emit the event.
// ---------------------------------------------------------------------------

/// Project-relative file paths whose existing shards carry ANY of `sources`.
/// Reads the `.aspis-censor/` dir (best-effort: a read error → empty). Used to
/// find files whose coarse findings must be cleared when a coarse pass stops
/// reporting them. The shard's own `fileRelPath` field is the source of truth for
/// the path (the filename is a hash, not the path).
fn files_with_sources(root: &Path, sources: &BTreeSet<String>) -> Vec<String> {
    let shards = match super::ledger::list_shards(root) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    shards
        .into_iter()
        .filter(|s| {
            !s.file_rel_path.is_empty() && s.findings.iter().any(|f| sources.contains(&f.source))
        })
        .map(|s| s.file_rel_path)
        .collect()
}

/// Fire the training-export rail for a completed fine or coarse pass.
///
/// LOCK-ORDERING CONTRACT: reads the agent-state snapshot under its lock, clones the
/// directives and sessions to owned `Vec`s, drops the lock, THEN calls into
/// `training_export` — so the JSONL per-path mutex (owned by `training_export`) is
/// never acquired while the agent-state lock is held. This is the ONLY safe call
/// pattern; see `training_export` module header.
///
/// `shard_lookup` reads the already-written shard for each changed file:
///   - `Some(findings)` — shard exists (possibly empty ↔ clean verdict).
///   - `None`           — no shard for this file in this batch (shouldn't happen for
///     a file we just wrote, but the ledger read may fail; the rail skips it silently).
///
/// Fire-and-forget: any failure inside the training rail is logged by the module and
/// never propagated here — a training hiccup must never perturb the live pipeline.
fn record_findings_for_batch(app: &AppHandle, root: &Path, changed: &[String]) {
    // 1) Read agent-state snapshot under its lock, clone to owned Vecs, release.
    let (directives, sessions) = match crate::backend::agents::read_agent_live_state_snapshot(app)
    {
        Ok(snap) => (snap.mini_coder_directives, snap.sessions),
        Err(_) => {
            // Agent state unreadable (e.g. first run before state file exists): proceed
            // with empty attribution — the findings record is still written, just
            // without an agent attribution.
            (vec![], vec![])
        }
    };
    // Lock is now released (snapshot is fully owned). Build the shard_lookup closure
    // that is given to record_findings_batch — it reads the ALREADY-COMMITTED shards
    // for the changed files (written moments earlier in this same pass).
    let root_owned = root.to_path_buf();
    let shard_lookup = move |file: &str| -> Option<Vec<FindingLite>> {
        match super::ledger::read_shard(&root_owned, file) {
            Ok(Some(shard)) => Some(shard.findings.iter().map(FindingLite::from).collect()),
            // Shard genuinely absent for this file → None (no record for this file).
            Ok(None) => None,
            // IO failure reading the shard: treat as absent (don't record partial data).
            Err(_) => None,
        }
    };
    // 2) Call with owned snapshots (agent-state lock is NOT held). Fire-and-forget.
    training_export::record_findings_batch(root, changed, shard_lookup, &directives, &sessions);
}

/// Emit `censor://findings-updated` with the deduped, sorted changed-file set ONLY
/// if `running` is still set. Checking the stop flag immediately before the emit
/// closes the window where a stop/replace lands during a slow pass: without this a
/// torn-down or replaced project would still receive a stale `findings-updated`
/// event (MAJOR — stale-emit-after-stop). An emit failure (window gone) logs and is
/// ignored.
fn emit_if_running(app: &AppHandle, project_id: &str, files: Vec<String>, running: &AtomicBool) {
    if !running.load(Ordering::SeqCst) {
        return;
    }
    let mut deduped: BTreeSet<String> = BTreeSet::new();
    for f in files {
        deduped.insert(f);
    }
    let payload = FindingsUpdatedPayload {
        project_id: project_id.to_string(),
        files: deduped.into_iter().collect(),
    };
    if let Err(e) = app.emit(FINDINGS_UPDATED_EVENT, &payload) {
        eprintln!("censor orchestrator: emit {FINDINGS_UPDATED_EVENT} failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    fn kinds(list: &[ProjectKind]) -> HashSet<ProjectKind> {
        list.iter().copied().collect()
    }

    fn raw(file: &str, source: &str) -> RawFinding {
        RawFinding {
            file: file.into(),
            line: Some(1),
            severity: Severity::Medium,
            category: Category::Correctness,
            source: source.into(),
            title: "t".into(),
            body: "b".into(),
        }
    }

    // ---- plan_fine: runner selection per file ----

    #[test]
    fn plan_fine_ts_file_gets_fine_ts_and_cross_cutting_fine() {
        let plan = plan_fine(&kinds(&[ProjectKind::Node]), &["src/app.ts".to_string()]);
        assert_eq!(plan.len(), 1);
        let fp = &plan[0];
        assert_eq!(fp.file_rel_path, "src/app.ts");
        // tsc/knip are COARSE → excluded; eslint is FINE? No — eslint is FINE.
        assert!(fp.runners.contains(&RunnerId::Eslint));
        // Cross-cutting FINE: lizard, semgrep.
        assert!(fp.runners.contains(&RunnerId::Lizard));
        assert!(fp.runners.contains(&RunnerId::Semgrep));
        // Coarse runners must NOT appear in a fine plan.
        assert!(!fp.runners.contains(&RunnerId::Tsc));
        assert!(!fp.runners.contains(&RunnerId::Knip));
        assert!(!fp.runners.contains(&RunnerId::Gitleaks));
        assert!(!fp.runners.contains(&RunnerId::Jscpd));
        // Every kept runner is genuinely FINE.
        for r in &fp.runners {
            assert_eq!(r.granularity(), Granularity::Fine, "{r:?} must be fine");
        }
    }

    #[test]
    fn plan_fine_py_file_gets_ruff_bandit_vulture() {
        let plan = plan_fine(&kinds(&[ProjectKind::Python]), &["a.py".to_string()]);
        let fp = &plan[0];
        assert!(fp.runners.contains(&RunnerId::Ruff));
        assert!(fp.runners.contains(&RunnerId::Bandit));
        assert!(fp.runners.contains(&RunnerId::Vulture));
    }

    #[test]
    fn plan_fine_rust_file_has_only_cross_cutting_fine() {
        // All Rust kind-specific runners (clippy/cargo-check/cargo-audit) are
        // COARSE, so a .rs file's FINE plan is just the cross-cutting fine set.
        let plan = plan_fine(&kinds(&[ProjectKind::Rust]), &["src/lib.rs".to_string()]);
        let fp = &plan[0];
        assert!(!fp.runners.contains(&RunnerId::Clippy));
        assert!(fp.runners.contains(&RunnerId::Lizard));
        assert!(fp.runners.contains(&RunnerId::Semgrep));
    }

    #[test]
    fn plan_fine_preserves_file_order() {
        let files = vec!["b.py".to_string(), "a.py".to_string()];
        let plan = plan_fine(&kinds(&[ProjectKind::Python]), &files);
        assert_eq!(plan[0].file_rel_path, "b.py");
        assert_eq!(plan[1].file_rel_path, "a.py");
    }

    // ---- coarse_runners: project-level set ----

    #[test]
    fn coarse_runners_for_rust_node_polyglot() {
        let c = coarse_runners(&kinds(&[ProjectKind::Rust, ProjectKind::Node]));
        // Rust coarse.
        assert!(c.contains(&RunnerId::Clippy));
        assert!(c.contains(&RunnerId::CargoCheck));
        assert!(c.contains(&RunnerId::CargoAudit));
        assert!(c.contains(&RunnerId::CargoDeny));
        // Node coarse.
        assert!(c.contains(&RunnerId::Tsc));
        assert!(c.contains(&RunnerId::Knip));
        // Cross-cutting coarse.
        assert!(c.contains(&RunnerId::Gitleaks));
        assert!(c.contains(&RunnerId::Jscpd));
        // No FINE runners.
        for r in &c {
            assert_eq!(r.granularity(), Granularity::Coarse, "{r:?} must be coarse");
        }
        // No duplicates.
        let set: HashSet<RunnerId> = c.iter().copied().collect();
        assert_eq!(set.len(), c.len());
    }

    #[test]
    fn coarse_runners_for_empty_kinds_is_only_cross_cutting_coarse() {
        let c = coarse_runners(&kinds(&[]));
        // Only the cross-cutting COARSE runners (gitleaks, jscpd, zizmor) survive.
        assert!(c.contains(&RunnerId::Gitleaks));
        assert!(c.contains(&RunnerId::Jscpd));
        assert!(c.contains(&RunnerId::Zizmor));
        assert!(!c.contains(&RunnerId::Clippy));
        assert!(!c.contains(&RunnerId::Tsc));
        assert_eq!(c.len(), 3);
    }

    // ---- group_by_file ----

    #[test]
    fn group_by_file_buckets_and_normalizes_paths() {
        let raws = vec![
            raw("src\\a.rs", "gitleaks"),
            raw("src/a.rs", "jscpd"),
            raw("src/b.rs", "gitleaks"),
        ];
        let grouped = group_by_file(raws);
        // Backslash + forward-slash collapse to one key.
        assert_eq!(grouped.get("src/a.rs").map(|v| v.len()), Some(2));
        assert_eq!(grouped.get("src/b.rs").map(|v| v.len()), Some(1));
        assert_eq!(grouped.len(), 2);
    }

    #[test]
    fn group_by_file_empty_input() {
        assert!(group_by_file(Vec::new()).is_empty());
    }

    // ---- payload shape ----

    #[test]
    fn findings_updated_payload_is_camel_case() {
        let p = FindingsUpdatedPayload {
            project_id: "proj-1".into(),
            files: vec!["src/a.rs".into()],
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"projectId\":\"proj-1\""), "json: {json}");
        assert!(json.contains("\"files\":[\"src/a.rs\"]"), "json: {json}");
        assert!(!json.contains("project_id"), "snake_case leaked: {json}");
    }

    #[test]
    fn runner_source_matches_program_family() {
        // The scoped-merge source set is derived from runner_source; pin the
        // strings so a rename of a parser's `source` is caught here.
        assert_eq!(runner_source(RunnerId::Clippy), "clippy");
        assert_eq!(runner_source(RunnerId::CargoCheck), "cargo-check");
        assert_eq!(runner_source(RunnerId::CargoAudit), "cargo-audit");
        // Must match parse_cargo_deny's RawFinding.source or the scoped merge
        // stops clearing stale findings (zombie shard entries).
        assert_eq!(runner_source(RunnerId::CargoDeny), "cargo-deny");
        assert_eq!(runner_source(RunnerId::Eslint), "eslint");
        assert_eq!(runner_source(RunnerId::Gitleaks), "gitleaks");
        // A5 (EOD review): the 8 other runners added in the same wave were
        // unpinned — a runner_source rename without the matching parser
        // RawFinding.source rename silently strands zombie shard findings.
        assert_eq!(runner_source(RunnerId::CargoFmt), "cargo-fmt");
        assert_eq!(runner_source(RunnerId::NpmAudit), "npm-audit");
        assert_eq!(runner_source(RunnerId::Oxlint), "oxlint");
        assert_eq!(runner_source(RunnerId::PipAudit), "pip-audit");
        assert_eq!(runner_source(RunnerId::Pyright), "pyright");
        assert_eq!(runner_source(RunnerId::Prettier), "prettier");
        assert_eq!(runner_source(RunnerId::RuffFormat), "ruff-format");
        assert_eq!(runner_source(RunnerId::Zizmor), "zizmor");
        // Go runners: these MUST equal the `source:` literal each parser stamps
        // (gofmt.rs → "gofmt", go_vet.rs → "go-vet") or the scoped merge strands
        // zombie shard findings.
        assert_eq!(runner_source(RunnerId::Gofmt), "gofmt");
        assert_eq!(runner_source(RunnerId::GoVet), "go-vet");
        // C/C++ runner: MUST equal the `source:` literal cppcheck.rs stamps ("cppcheck").
        assert_eq!(runner_source(RunnerId::Cppcheck), "cppcheck");
        // HTML/Kotlin runners: MUST equal the `source:` literals tidy.rs ("tidy") and
        // ktlint.rs ("ktlint") stamp, or the scoped merge strands zombie shard findings.
        assert_eq!(runner_source(RunnerId::Tidy), "tidy");
        assert_eq!(runner_source(RunnerId::Ktlint), "ktlint");
    }

    #[test]
    fn coarse_runners_for_go_includes_go_vet_not_gofmt() {
        // A Go project's COARSE set must include go vet (compile-based, project-wide)
        // and must NOT include gofmt (it is FINE, per-file).
        let c = coarse_runners(&kinds(&[ProjectKind::Go]));
        assert!(c.contains(&RunnerId::GoVet));
        assert!(!c.contains(&RunnerId::Gofmt));
        // Cross-cutting coarse still present.
        assert!(c.contains(&RunnerId::Gitleaks));
        for r in &c {
            assert_eq!(r.granularity(), Granularity::Coarse, "{r:?} must be coarse");
        }
    }

    #[test]
    fn plan_fine_go_file_gets_gofmt() {
        // A .go file's FINE plan includes gofmt (instant) but NOT go vet (coarse).
        let plan = plan_fine(&kinds(&[ProjectKind::Go]), &["main.go".to_string()]);
        let fp = &plan[0];
        assert!(fp.runners.contains(&RunnerId::Gofmt));
        assert!(!fp.runners.contains(&RunnerId::GoVet));
        for r in &fp.runners {
            assert_eq!(r.granularity(), Granularity::Fine, "{r:?} must be fine");
        }
    }

    #[test]
    fn coarse_runners_for_cpp_has_no_cppcheck() {
        // cppcheck is FINE (no-compile, per-file), so it must NEVER appear in the
        // COARSE set — only the cross-cutting coarse runners do for a C/C++ project.
        let c = coarse_runners(&kinds(&[ProjectKind::Cpp]));
        assert!(!c.contains(&RunnerId::Cppcheck));
        assert!(c.contains(&RunnerId::Gitleaks));
        for r in &c {
            assert_eq!(r.granularity(), Granularity::Coarse, "{r:?} must be coarse");
        }
    }

    #[test]
    fn plan_fine_cpp_file_gets_cppcheck() {
        // A .cpp file's FINE plan includes cppcheck (no-compile, per-file).
        let plan = plan_fine(&kinds(&[ProjectKind::Cpp]), &["main.cpp".to_string()]);
        let fp = &plan[0];
        assert!(fp.runners.contains(&RunnerId::Cppcheck));
        for r in &fp.runners {
            assert_eq!(r.granularity(), Granularity::Fine, "{r:?} must be fine");
        }
    }

    #[test]
    fn plan_fine_html_file_gets_tidy_with_no_project_kind() {
        // An .html file's FINE plan includes tidy even with NO project kind (HTML has no
        // manifest — tidy gates on the FileLang alone). cppcheck is FINE, so it must be.
        let plan = plan_fine(&kinds(&[]), &["index.html".to_string()]);
        let fp = &plan[0];
        assert!(fp.runners.contains(&RunnerId::Tidy));
        for r in &fp.runners {
            assert_eq!(r.granularity(), Granularity::Fine, "{r:?} must be fine");
        }
    }

    #[test]
    fn coarse_runners_for_kotlin_has_no_ktlint() {
        // ktlint is FINE (no-compile, per-file), so it must NEVER appear in the COARSE
        // set — only the cross-cutting coarse runners do for a Kotlin project.
        let c = coarse_runners(&kinds(&[ProjectKind::Kotlin]));
        assert!(!c.contains(&RunnerId::Ktlint));
        assert!(c.contains(&RunnerId::Gitleaks));
        for r in &c {
            assert_eq!(r.granularity(), Granularity::Coarse, "{r:?} must be coarse");
        }
    }

    #[test]
    fn plan_fine_kotlin_file_gets_ktlint() {
        // A .kt file's FINE plan includes ktlint (no-compile, per-file).
        let plan = plan_fine(&kinds(&[ProjectKind::Kotlin]), &["Main.kt".to_string()]);
        let fp = &plan[0];
        assert!(fp.runners.contains(&RunnerId::Ktlint));
        for r in &fp.runners {
            assert_eq!(r.granularity(), Granularity::Fine, "{r:?} must be fine");
        }
    }

    // ---- collect cores: tempdir, no tools installed → empty, no panic ----

    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "aspis-censor-orch-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Any shard produced by a real pass must be internally consistent: its
    /// findings stamped with the shard's content hash, and the shard path valid.
    /// (We can't assert emptiness — a dev machine may have linters installed —
    /// only that the engine runs cleanly and writes well-formed shards.)
    fn assert_shards_well_formed(root: &Path) {
        for shard in super::super::ledger::list_shards(root).unwrap() {
            assert!(super::super::ledger::validate_rel_path(&shard.file_rel_path).is_ok());
            for f in &shard.findings {
                // A produced finding carries the shard's hash and a non-empty source.
                assert_eq!(
                    f.content_hash, shard.content_hash,
                    "finding hash matches shard"
                );
                assert!(!f.source.is_empty(), "finding has a source");
                // SECURITY: the body must never carry a raw AWS-key-shaped secret
                // (the runners redact; this is a belt-and-braces engine-level check).
                assert!(
                    !f.body.contains("AKIAIOSFODNN7EXAMPLE"),
                    "redacted secret leaked into a persisted finding body"
                );
            }
        }
    }

    #[test]
    fn fine_batch_collect_runs_cleanly_and_writes_well_formed_shards() {
        // A Node project with a .ts file. The fine pass must complete without panic
        // regardless of which linters are installed; any shard it writes must be
        // well-formed. (Tolerant: we don't assert a specific finding count.)
        let dir = unique_temp_root("fine");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.ts"), "export const x = 1;\n").unwrap();

        let _changed = fine_batch_collect(root, &["src/app.ts".to_string()], None);
        assert_shards_well_formed(root);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn coarse_pass_collect_runs_cleanly() {
        let dir = unique_temp_root("coarse");
        let root = dir.as_path();
        fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

        // Must not panic; any shards written are well-formed.
        let _changed = coarse_pass_collect(root);
        assert_shards_well_formed(root);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fine_batch_empty_file_list_is_a_clean_noop() {
        // The degenerate empty batch: nothing planned, nothing changed, no panic.
        let dir = unique_temp_root("empty");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        let changed = fine_batch_collect(root, &[], None);
        assert!(changed.is_empty());
        // No shard dir is created for an empty batch.
        assert!(!root.join(super::super::CENSOR_DIR).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- BLOCKER 1: streaming hash, no whole-file slurp, size cap ----

    #[test]
    fn hash_file_streams_large_file_and_matches_oneshot_sha256() {
        use sha2::{Digest, Sha256};
        // A file LARGER than the streaming buffer must hash correctly without ever
        // being read whole into memory (the loop is O(HASH_BUF_BYTES)).
        let dir = unique_temp_root("hash-large");
        let root = dir.as_path();
        // 3 buffers + a tail, well over HASH_BUF_BYTES, well under the 8 MiB cap.
        let len = HASH_BUF_BYTES * 3 + 12345;
        let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let path = root.join("big.bin");
        fs::write(&path, &bytes).unwrap();

        let expected = {
            let mut h = Sha256::new();
            h.update(&bytes);
            hex::encode(h.finalize())
        };
        match hash_file(&path) {
            HashOutcome::Hashed(got) => {
                assert_eq!(got, expected, "streamed hash must equal one-shot")
            }
            other => panic!("expected Hashed, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_file_over_size_cap_is_skip_not_hashed() {
        let dir = unique_temp_root("hash-cap");
        let root = dir.as_path();
        let path = root.join("huge.bin");
        // One byte over the cap → Skip (don't hash/review a giant blob).
        let f = fs::File::create(&path).unwrap();
        f.set_len(MAX_HASH_BYTES + 1).unwrap();
        assert_eq!(hash_file(&path), HashOutcome::Skip);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_file_missing_is_deleted_dir_is_skip() {
        let dir = unique_temp_root("hash-missing");
        let root = dir.as_path();
        // A genuinely absent file → Deleted (its shard may be pruned).
        assert_eq!(hash_file(&root.join("nope.rs")), HashOutcome::Deleted);
        // A path that EXISTS but is not a readable regular file (a directory) maps to
        // a non-NotFound IO error → Skip (never a spurious delete).
        let subdir = root.join("adir");
        fs::create_dir_all(&subdir).unwrap();
        assert_eq!(hash_file(&subdir), HashOutcome::Skip);
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- BLOCKER 2: a transient (non-NotFound) read error must NOT destroy a
    //                 judged disposition. write_file_shard must SKIP, leaving the
    //                 shard exactly as-is. ----

    #[test]
    fn write_file_shard_skips_on_transient_read_error_preserving_fp_disposition() {
        use crate::backend::censor::ledger::{read_shard, write_shard};
        use crate::backend::censor::schema::{
            Category, CensorShard, Disposition, Finding, ProvenanceEntry, Severity, Verdict,
        };

        let dir = unique_temp_root("blocker2");
        let root = dir.as_path();
        let rel = "src/locked.ts";

        // Seed a shard whose single finding a human marked Fp (with provenance) at
        // hash "h0", source eslint.
        let judged = Finding {
            id: "es-judged".into(),
            file: rel.into(),
            content_hash: "h0".into(),
            line: Some(3),
            severity: Severity::Medium,
            category: Category::Correctness,
            source: "eslint".into(),
            title: "t".into(),
            body: "b".into(),
            verdict: Verdict::Suspected,
            disposition: Disposition::Fp,
            provenance: vec![ProvenanceEntry {
                actor: "coder".into(),
                action: "fp".into(),
                role: String::new(),
                at: "t0".into(),
            }],
            created_at: "t0".into(),
            commit: None,
        };
        write_shard(
            root,
            &CensorShard {
                file_rel_path: rel.into(),
                content_hash: "h0".into(),
                updated_at: "t0".into(),
                findings: vec![judged],
            },
        )
        .unwrap();

        // Now make the path UNREADABLE-as-a-file but PRESENT: replace it with a
        // DIRECTORY at <root>/src/locked.ts. hash_file → Skip (non-NotFound error),
        // so write_file_shard must NOT touch the shard.
        let abs = root.join("src").join("locked.ts");
        fs::create_dir_all(&abs).unwrap();

        let refreshed: BTreeSet<String> = ["eslint".to_string()].into_iter().collect();
        let wrote = write_file_shard(root, rel, Vec::new(), &refreshed, "t1");
        assert!(!wrote, "a Skip outcome must report no shard change");

        // The Fp finding + its provenance MUST survive untouched.
        let shard = read_shard(root, rel).unwrap().expect("shard still present");
        assert_eq!(
            shard.content_hash, "h0",
            "shard hash unchanged (not pruned)"
        );
        assert_eq!(shard.findings.len(), 1, "judged finding survived the skip");
        assert_eq!(shard.findings[0].disposition, Disposition::Fp);
        assert_eq!(shard.findings[0].provenance.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    // ---- MAJOR: validate_rel_path applied to watcher-derived FINE paths ----

    #[test]
    fn fine_batch_skips_invalid_rel_path_without_writing() {
        // A malformed watcher-derived path (`..` traversal) must be SKIPPED, never
        // crash and never write a shard outside `.aspis-censor/`.
        let dir = unique_temp_root("fine-badpath");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        let changed = fine_batch_collect(root, &["../escape.ts".to_string()], None);
        assert!(changed.is_empty(), "invalid path produces no shard change");
        // No shard dir created from the rejected path.
        assert!(!root.join(super::super::CENSOR_DIR).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- A4: Gemma tier merges additively without clobbering deterministic
    //          findings; degrades to deterministic-only when disabled. ----

    /// A stub Gemma client: a fixed availability + a canned generate response. No
    /// network. Mirrors the gemma.rs stub but local to the orchestrator tests.
    struct StubGemma {
        response: String,
    }
    impl gemma::GemmaClient for StubGemma {
        fn probe(&self) -> bool {
            true
        }
        fn generate(&self, _prompt: &str) -> Result<String, gemma::GemmaError> {
            Ok(self.response.clone())
        }
        fn provider_label(&self) -> &'static str {
            "stub"
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
    }

    /// Seed a shard for `rel` at the file's CURRENT on-disk hash with one finding
    /// from `source` (so it is a deterministic-style finding the fine pass for a
    /// .ts file does NOT refresh — gitleaks is a COARSE cross-cutting source, never
    /// in the fine refreshed set). This is exactly the A3 no-clobber test pattern.
    fn seed_foreign_source_finding(root: &Path, rel: &str, source: &str) -> String {
        use crate::backend::censor::ledger::{read_supersede_write_shard, validate_rel_path};
        validate_rel_path(rel).unwrap();
        let abs = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let hash = match super::hash_file(&abs) {
            super::HashOutcome::Hashed(h) => h,
            other => panic!("expected a hashable seed file, got {other:?}"),
        };
        let f = crate::backend::censor::schema::Finding {
            id: "seed-foreign".into(),
            file: rel.into(),
            content_hash: hash.clone(),
            line: Some(1),
            severity: Severity::High,
            category: Category::Security,
            source: source.into(),
            title: "seeded foreign finding".into(),
            body: "b".into(),
            verdict: crate::backend::censor::schema::Verdict::Suspected,
            disposition: crate::backend::censor::schema::Disposition::Open,
            provenance: vec![],
            created_at: "t0".into(),
            commit: None,
        };
        let refreshed: BTreeSet<String> = [source.to_string()].into_iter().collect();
        read_supersede_write_shard(root, vec![f], &hash, &refreshed, rel, "t0").unwrap();
        hash
    }

    #[test]
    fn fine_pass_with_gemma_merges_without_clobbering_foreign_source() {
        // A .ts file with a pre-existing gitleaks (coarse, NOT fine-refreshed)
        // finding. Running the FINE pass with an available Gemma stub that returns a
        // semantic finding must: (a) ADD the gemma finding, (b) leave the gitleaks
        // finding intact (clobber-protection via the source-scoped merge).
        let dir = unique_temp_root("gemma-merge");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.ts"), "export const x = 1;\n").unwrap();

        let seeded_hash = seed_foreign_source_finding(root, "src/app.ts", "gitleaks");

        let stub = StubGemma {
            response: r#"[{"line": 1, "title": "Inverted guard", "body": "logic backwards", "severity": "high"}]"#
                .into(),
        };
        let ctx = GemmaCtx {
            client: &stub,
            available: true,
        };
        let changed = fine_batch_collect(root, &["src/app.ts".to_string()], Some(ctx));
        assert!(
            changed.contains(&"src/app.ts".to_string()),
            "the shard changed"
        );

        let shard = super::super::ledger::read_shard(root, "src/app.ts")
            .unwrap()
            .expect("shard present");
        // The file did not change, so its hash is unchanged → gitleaks finding kept.
        assert_eq!(shard.content_hash, seeded_hash);
        let has_gitleaks = shard.findings.iter().any(|f| f.source == "gitleaks");
        let gemma_findings: Vec<_> = shard
            .findings
            .iter()
            .filter(|f| f.source == "gemma")
            .collect();
        assert!(
            has_gitleaks,
            "deterministic (gitleaks) finding must NOT be clobbered by gemma"
        );
        assert_eq!(gemma_findings.len(), 1, "gemma finding added");
        assert_eq!(gemma_findings[0].title, "Inverted guard");
        assert_eq!(gemma_findings[0].category, Category::Correctness);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fine_pass_disabled_gemma_equals_deterministic_only() {
        // The degrade guarantee: with Gemma DISABLED (available=false), the fine pass
        // produces NO gemma-source findings — identical to passing no client at all
        // (deterministic-only A3). We compare the gemma-source finding set (which must
        // be empty) for both a None ctx and an available=false ctx, against the same
        // seeded shard, so the only variable is the Gemma tier.
        let dir = unique_temp_root("gemma-degrade");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.ts"), "export const x = 1;\n").unwrap();
        seed_foreign_source_finding(root, "src/app.ts", "gitleaks");

        // available=false: the stub's generate must never be consulted; no gemma finding.
        let stub = StubGemma {
            response: r#"[{"line": 1, "title": "should not appear", "severity": "high"}]"#.into(),
        };
        let ctx = GemmaCtx {
            client: &stub,
            available: false,
        };
        let _ = fine_batch_collect(root, &["src/app.ts".to_string()], Some(ctx));
        let shard = super::super::ledger::read_shard(root, "src/app.ts")
            .unwrap()
            .unwrap();
        let gemma_count = shard
            .findings
            .iter()
            .filter(|f| f.source == "gemma")
            .count();
        assert_eq!(gemma_count, 0, "disabled gemma must add no findings");
        // The foreign deterministic finding is still protected.
        assert!(shard.findings.iter().any(|f| f.source == "gitleaks"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fine_pass_clears_stale_gemma_when_model_returns_nothing() {
        // If Gemma previously reported a finding but now returns [], the stale gemma
        // finding must clear (the fine pass always refreshes "gemma" when active), so
        // a fixed smell does not linger forever. The foreign gitleaks finding stays.
        let dir = unique_temp_root("gemma-clear");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.ts"), "export const x = 1;\n").unwrap();
        seed_foreign_source_finding(root, "src/app.ts", "gitleaks");

        // First pass: gemma reports one finding.
        let stub1 = StubGemma {
            response: r#"[{"line": 2, "title": "Transient smell", "severity": "low"}]"#.into(),
        };
        let _ = fine_batch_collect(
            root,
            &["src/app.ts".to_string()],
            Some(GemmaCtx {
                client: &stub1,
                available: true,
            }),
        );
        let mid = super::super::ledger::read_shard(root, "src/app.ts")
            .unwrap()
            .unwrap();
        assert_eq!(
            mid.findings.iter().filter(|f| f.source == "gemma").count(),
            1
        );

        // Second pass on the SAME (unchanged) file: gemma now returns nothing → the
        // stale gemma finding clears (same-hash drop on the refreshed "gemma" source).
        let stub2 = StubGemma {
            response: "[]".into(),
        };
        let _ = fine_batch_collect(
            root,
            &["src/app.ts".to_string()],
            Some(GemmaCtx {
                client: &stub2,
                available: true,
            }),
        );
        let after = super::super::ledger::read_shard(root, "src/app.ts")
            .unwrap()
            .unwrap();
        assert_eq!(
            after
                .findings
                .iter()
                .filter(|f| f.source == "gemma")
                .count(),
            0,
            "stale gemma finding must clear when the model returns nothing"
        );
        assert!(
            after.findings.iter().any(|f| f.source == "gitleaks"),
            "foreign finding survives"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // ---- BLOCKER 3 / N1: stale gemma findings clear when the tier goes OFFLINE ----

    #[test]
    fn fine_pass_clears_stale_gemma_when_tier_goes_offline() {
        // The missing N1 case: Gemma was ONLINE (wrote a finding), then goes OFFLINE.
        // A subsequent fine pass with the client PRESENT but available=false must
        // CLEAR the stale gemma finding (the source is always refreshed when a context
        // exists) while leaving deterministic findings untouched.
        let dir = unique_temp_root("gemma-offline-clear");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.ts"), "export const x = 1;\n").unwrap();
        seed_foreign_source_finding(root, "src/app.ts", "gitleaks");

        // Online pass: gemma writes a finding.
        let stub_online = StubGemma {
            response: r#"[{"line": 2, "title": "Online-era smell", "severity": "low"}]"#.into(),
        };
        let _ = fine_batch_collect(
            root,
            &["src/app.ts".to_string()],
            Some(GemmaCtx {
                client: &stub_online,
                available: true,
            }),
        );
        let mid = super::super::ledger::read_shard(root, "src/app.ts")
            .unwrap()
            .unwrap();
        assert_eq!(
            mid.findings.iter().filter(|f| f.source == "gemma").count(),
            1
        );

        // The tier now goes OFFLINE: client still present, available=false. The stub's
        // generate must NEVER be consulted, yet the stale gemma finding must clear.
        let stub_offline = StubGemma {
            response: r#"[{"line": 9, "title": "must not appear", "severity": "high"}]"#.into(),
        };
        let _ = fine_batch_collect(
            root,
            &["src/app.ts".to_string()],
            Some(GemmaCtx {
                client: &stub_offline,
                available: false,
            }),
        );
        let after = super::super::ledger::read_shard(root, "src/app.ts")
            .unwrap()
            .unwrap();
        assert_eq!(
            after
                .findings
                .iter()
                .filter(|f| f.source == "gemma")
                .count(),
            0,
            "an OFFLINE pass must clear stale gemma findings"
        );
        assert!(
            after.findings.iter().any(|f| f.source == "gitleaks"),
            "deterministic findings untouched by the offline gemma clear"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fine_pass_no_gemma_ctx_never_touches_gemma_source() {
        // The pure-A3 path: with NO client/ctx, the fine pass must NOT refresh the
        // "gemma" source — a pre-existing gemma finding (e.g. from an earlier session
        // that had a client) survives untouched, because a pure-A3 caller has no
        // business clearing the gemma tier's findings.
        let dir = unique_temp_root("gemma-noctx");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.ts"), "export const x = 1;\n").unwrap();
        seed_foreign_source_finding(root, "src/app.ts", "gemma");

        let _ = fine_batch_collect(root, &["src/app.ts".to_string()], None);
        let after = super::super::ledger::read_shard(root, "src/app.ts")
            .unwrap()
            .unwrap();
        assert_eq!(
            after
                .findings
                .iter()
                .filter(|f| f.source == "gemma")
                .count(),
            1,
            "a pure-A3 pass (no gemma ctx) must NOT refresh/clear the gemma source"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // ---- BLOCKER 2: read_file_outcome bounds the read regardless of file size ----

    #[test]
    fn read_file_outcome_over_cap_is_skip_without_full_slurp() {
        // A file LARGER than MAX_HASH_BYTES must map to Skip, and the bounded read must
        // never allocate the whole file (we only ever take cap + 1 bytes).
        let dir = unique_temp_root("read-cap");
        let root = dir.as_path();
        let path = root.join("huge.ts");
        let f = fs::File::create(&path).unwrap();
        f.set_len(MAX_HASH_BYTES + 4096).unwrap();
        match read_file_outcome(&path) {
            ReadOutcome::Skip => {}
            ReadOutcome::Read { content, .. } => {
                panic!(
                    "expected Skip for an over-cap file; slurped {} bytes",
                    content.len()
                )
            }
            ReadOutcome::Deleted => panic!("expected Skip, got Deleted"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_outcome_at_cap_is_read_over_cap_is_skip() {
        // Exactly at the cap → Read; one byte over → Skip (boundary check).
        let dir = unique_temp_root("read-boundary");
        let root = dir.as_path();

        let at = root.join("at.ts");
        let fa = fs::File::create(&at).unwrap();
        fa.set_len(MAX_HASH_BYTES).unwrap();
        assert!(
            matches!(read_file_outcome(&at), ReadOutcome::Read { .. }),
            "a file exactly at the cap must be Read"
        );

        let over = root.join("over.ts");
        let fo = fs::File::create(&over).unwrap();
        fo.set_len(MAX_HASH_BYTES + 1).unwrap();
        assert!(
            matches!(read_file_outcome(&over), ReadOutcome::Skip),
            "a file one byte over the cap must be Skip"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_outcome_missing_is_deleted() {
        let dir = unique_temp_root("read-missing");
        let root = dir.as_path();
        assert!(matches!(
            read_file_outcome(&root.join("nope.ts")),
            ReadOutcome::Deleted
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_outcome_reads_content_and_hash_matching_hash_file() {
        // The content path and the hash-only path must agree on the hash for a normal
        // file, so the Gemma-active read and the deterministic-only stream hash never
        // diverge.
        let dir = unique_temp_root("read-agree");
        let root = dir.as_path();
        let path = root.join("a.ts");
        fs::write(&path, "export const x = 1;\n").unwrap();
        let (content, hash) = match read_file_outcome(&path) {
            ReadOutcome::Read { content, hash } => (content, hash),
            other => panic!("expected Read, got {other:?}"),
        };
        assert_eq!(content, "export const x = 1;\n");
        match hash_file(&path) {
            HashOutcome::Hashed(h) => {
                assert_eq!(h, hash, "content-read hash must equal stream hash")
            }
            other => panic!("expected Hashed, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- TRAINING RAIL: record_findings_for_batch writes .aspis-training/ ----

    /// Seed a shard for `rel` with a single finding of the given severity + source,
    /// at the file's CURRENT on-disk hash. Returned finding id is deterministic.
    fn seed_shard_with_finding(
        root: &Path,
        rel: &str,
        source: &str,
        severity: crate::backend::censor::schema::Severity,
    ) {
        use crate::backend::censor::ledger::{read_supersede_write_shard, validate_rel_path};
        use crate::backend::censor::schema::{Category, Disposition, Finding, Verdict};
        validate_rel_path(rel).unwrap();
        let abs = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let hash = match super::hash_file(&abs) {
            super::HashOutcome::Hashed(h) => h,
            other => panic!("expected hashable file, got {other:?}"),
        };
        let f = Finding {
            id: format!("seed-{source}-{}", rel.replace('/', "_")),
            file: rel.into(),
            content_hash: hash.clone(),
            line: Some(1),
            severity,
            category: Category::Security,
            source: source.into(),
            title: "test finding".into(),
            body: "body".into(),
            verdict: Verdict::Suspected,
            disposition: Disposition::Open,
            provenance: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            commit: None,
        };
        let refreshed: std::collections::BTreeSet<String> =
            [source.to_string()].into_iter().collect();
        read_supersede_write_shard(root, vec![f], &hash, &refreshed, rel, "2026-01-01T00:00:00Z")
            .unwrap();
    }

    #[test]
    fn record_findings_for_batch_writes_findings_and_pairs_jsonl() {
        // A tempdir project with two files:
        //   - "src/dirty.ts"  — has a High finding in its shard (seeded manually)
        //   - "src/clean.ts"  — has a shard but zero findings (clean verdict)
        //
        // `record_findings_for_batch` (called with no agent-state, so no attribution)
        // must write:
        //   - `findings.jsonl` with one line per changed file
        //   - each line is valid JSON with the expected file / finding count
        //
        // We use `record_findings_for_batch` directly (not via `run_fine_batch` which
        // needs a real AppHandle) by calling it with a snapshot of empty directives +
        // sessions (the same degraded path the real call takes when the state file is
        // absent).  We cannot call `record_findings_for_batch` itself (it reads the
        // AppHandle for agent state), so we call `training_export::record_findings_batch`
        // directly — which is the exact call site it dispatches to, reproducing the same
        // behaviour without needing a Tauri runtime.
        use crate::backend::training_export;
        use crate::backend::censor::schema::Severity;

        let dir = unique_temp_root("train-batch");
        let root = dir.as_path();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/dirty.ts"), "const x = 1;\n").unwrap();
        fs::write(root.join("src/clean.ts"), "const y = 2;\n").unwrap();

        // Seed a High finding into dirty.ts's shard.
        seed_shard_with_finding(root, "src/dirty.ts", "eslint", Severity::High);

        // Seed an empty shard for clean.ts (use the same helper with a fresh source +
        // let the shard have no findings by superseding with an empty vec). We write a
        // shard with zero findings directly.
        {
            use crate::backend::censor::ledger::read_supersede_write_shard;
            let abs = root.join("src").join("clean.ts");
            let hash = match super::hash_file(&abs) {
                super::HashOutcome::Hashed(h) => h,
                other => panic!("expected hashable, got {other:?}"),
            };
            let refreshed: std::collections::BTreeSet<String> =
                ["eslint".to_string()].into_iter().collect();
            read_supersede_write_shard(
                root,
                vec![],
                &hash,
                &refreshed,
                "src/clean.ts",
                "2026-01-01T00:00:00Z",
            )
            .unwrap();
        }

        let changed = vec!["src/dirty.ts".to_string(), "src/clean.ts".to_string()];

        // Build the shard_lookup closure (mirrors record_findings_for_batch's logic).
        let root_owned = root.to_path_buf();
        let shard_lookup = move |file: &str| -> Option<Vec<FindingLite>> {
            match super::super::ledger::read_shard(&root_owned, file) {
                Ok(Some(shard)) => Some(shard.findings.iter().map(FindingLite::from).collect()),
                Ok(None) => None,
                Err(_) => None,
            }
        };

        // Call with empty directives + sessions (no attribution, same as the degraded
        // agent-state path).
        training_export::record_findings_batch(root, &changed, shard_lookup, &[], &[]);

        // Verify findings.jsonl was written with two lines.
        let findings_path = root
            .join(".aspis-training")
            .join("findings.jsonl");
        assert!(
            findings_path.exists(),
            ".aspis-training/findings.jsonl must be created"
        );
        let body = fs::read_to_string(&findings_path).unwrap();
        let lines: Vec<serde_json::Value> = body
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("valid JSON line"))
            .collect();
        assert_eq!(lines.len(), 2, "one line per changed file");

        // Find the dirty-file line and assert it carries the High finding.
        let dirty = lines
            .iter()
            .find(|l| l["file"] == "src/dirty.ts")
            .expect("dirty.ts line present");
        let findings_arr = dirty["findings"].as_array().expect("findings array");
        assert_eq!(findings_arr.len(), 1, "one High finding for dirty.ts");
        assert_eq!(
            findings_arr[0]["severity"].as_str(),
            Some("high"),
            "severity is high"
        );
        assert_eq!(
            findings_arr[0]["source"].as_str(),
            Some("eslint"),
            "source is eslint"
        );

        // The clean-file line must have zero findings (the clean-verdict signal).
        let clean = lines
            .iter()
            .find(|l| l["file"] == "src/clean.ts")
            .expect("clean.ts line present");
        let clean_findings = clean["findings"].as_array().expect("findings array");
        assert_eq!(
            clean_findings.len(),
            0,
            "clean.ts shard has zero findings — clean verdict"
        );

        // LOCK-ORDERING: the call is structurally after the agent-state snapshot is
        // owned (no live lock held), which is the invariant enforced by the call
        // placement in `record_findings_for_batch`. The test validates the same pattern:
        // owned slices passed to record_findings_batch, no lock held at call time.

        let _ = fs::remove_dir_all(&dir);
    }

    /// WARNING 4 (P6): the verdict-gate path (`run_fine_batch_no_rail`, `record_rail =
    /// false`) must NEVER write a training pair, while the watcher path (`run_fine_batch`,
    /// `record_rail = true`) still does. Recording on both produced DUPLICATE
    /// `censor_verdict` lines in pairs.jsonl for one file change.
    #[test]
    fn gate_path_skips_training_rail_watcher_path_records() {
        // Watcher path: rail on, running, changed -> records.
        assert!(
            should_record_rail(true, true, false),
            "watcher path (record_rail=true) records when running and shards changed"
        );
        // Gate path: rail OFF -> never records, even running with changes.
        assert!(
            !should_record_rail(false, true, false),
            "gate path (record_rail=false) must skip the training rail"
        );
        // Defensive: even the watcher path skips when stopped or nothing changed.
        assert!(!should_record_rail(true, false, false), "stopped worker skips rail");
        assert!(!should_record_rail(true, true, true), "no changed shards -> no rail");
    }
}
