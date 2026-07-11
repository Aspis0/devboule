//! Censor — Tauri command surface + the managed `CensorState`.
//!
//! All commands are gated by `BackendState::ensure_unlocked()` (they read/write
//! arbitrary local files + spawn linters).
//!
//! ON-DEMAND MODEL: Censor runs deterministically on mini-coder task completion
//! (FINE per-file linters, async via the mini-coder executor) and on a cooldown
//! timer for whole-project COARSE passes. There is NO filesystem watcher — the
//! executor triggers reviews, not file-save events. Phase C's board chip uses the
//! lock-free `censor_count_open` read.
//!
//! LIFECYCLE: the state map lock is NEVER held across blocking IO. Commands clone
//! out the root, release the lock, then do the subprocess/shard work
//! (mirrors `agent_pty.rs`). Teardown signals in-flight one-shot workers via
//! `kill_all_on_exit` so quit never orphans a thread or subprocess.
//! an in-flight tool subprocess.

use super::gemma;
use super::ledger;
use super::orchestrator::{self, GemmaCtx};
use super::schema::{CensorShard, Disposition, Finding, Severity};
use crate::backend::state::BackendState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, State};

/// BLOCKER B: the error returned by the tool-running commands when the project is
/// not trusted for Censor. A stable, content-free message the frontend can detect
/// to render the "Trust this project to run Censor" prompt.
const CENSOR_UNTRUSTED_MSG: &str =
    "Censor is disabled for this project. Trust the project to run Censor.";

/// Managed Censor state: the single active watch handle (if any). Guarded by its
/// own mutex so the managed state is thread-safe.
/// `None` when no project is being watched.
pub struct CensorState {
    /// IDENTITY-KEYED cache of the Censor LLM availability probe: `None` = not yet probed
    /// this session; `Some((cache_identity, available))` = the last probe's FULL client
    /// IDENTITY (`client.cache_identity()` — `"{provider}|{base}|{model}"`) and its result.
    /// The probe is a loopback HTTP round-trip — we pay it ONCE per identity (the first
    /// one-shot for that identity) and reuse the answer for every fine pass + the UI,
    /// rather than re-probing per file. KEYING ON THE FULL IDENTITY (not just the provider)
    /// is what fixes the stale-cache bug. The mutex BOTH stores the result AND serializes
    /// the one-time probe so concurrent starts cannot double-probe. Phase E reads it via
    /// [`CensorState::gemma_status`] (a brief lock, not on the per-file path).
    gemma_probe: Mutex<Option<(String, bool)>>,
    /// WARNING F: shared "keep running" flag for the DETACHED `censor_review_now`
    /// one-shot fallback. That worker runs runners + Gemma
    /// off the command thread and was previously untracked, so app exit could leave
    /// it (and an in-flight tool subprocess) running. The worker passes a CLONE of
    /// this `Arc` straight to the orchestrator as its `running` stop-gate (true =
    /// keep going); the orchestrator re-reads it between passes/runners/before emit,
    /// so flipping it to `false` in `kill_all_on_exit` aborts any in-flight one-shot
    /// at the next checkpoint (no orphan thread / no stale event). Shared (not
    /// per-thread) because a single exit signal must abandon ALL in-flight one-shots,
    /// and there are at most a handful at once.
    oneshot_running: Arc<AtomicBool>,
}

impl Default for CensorState {
    fn default() -> Self {
        Self::new()
    }
}

impl CensorState {
    pub fn new() -> Self {
        Self {
            gemma_probe: Mutex::new(None),
            oneshot_running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// WARNING F: a clone of the shared "keep running" flag for a detached
    /// `censor_review_now` fallback worker to hand to the orchestrator as its
    /// `running` stop-gate. Flipping it to `false` at exit aborts the worker at its
    /// next checkpoint.
    fn oneshot_running_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.oneshot_running)
    }

    /// WARNING F: signal every in-flight one-shot review to stop (called at app exit).
    fn signal_oneshots_stop(&self) {
        self.oneshot_running.store(false, Ordering::SeqCst);
    }

    /// Resolve the cached Gemma availability for `client`'s provider, probing exactly
    /// ONCE PER PROVIDER (the first call for a provider probes via `client` and stores
    /// `(provider, result)`; later calls for the SAME provider reuse it). On a fresh
    /// probe we log ONCE (not per file). Returns `true` iff the model is available.
    ///
    /// IDENTITY-KEYED (max-recall FIX 4): the cache is keyed on `client.cache_identity()`
    /// — the FULL `"{provider}|{base}|{model}"` identity, not just the provider. A warm
    /// cache for one identity is NOT reused for a client whose provider OR base OR model
    /// differs — any mismatch is a cache MISS and re-probed, then the new identity+result is
    /// stored. This prevents a stale availability (for the OLD endpoint) silently driving
    /// (or disabling) the tier after a base/model change within the SAME provider (the bug
    /// a provider-only key missed). The same-identity fast path still hits the cache (no
    /// re-probe, no re-log). The probe is a cheap loopback metadata read; the mutex is held
    /// only across it, never on the per-file path.
    fn ensure_gemma_probed(&self, client: &dyn gemma::GemmaClient) -> bool {
        let identity = client.cache_identity();
        // Serialize the one-time-per-identity probe so concurrent callers cannot both
        // round-trip + log (WARNING 1). A poisoned lock (a prior panic mid-probe) is
        // recovered — the probe is idempotent and side-effect-free beyond the log, so
        // proceeding is safe.
        let mut cache = self.gemma_probe.lock().unwrap_or_else(|p| p.into_inner());
        // CACHE HIT only when the stored entry is for the SAME full identity. A different
        // provider/base/model is a MISS → fall through and re-probe.
        if let Some((cached_identity, available)) = cache.as_ref() {
            if cached_identity == &identity {
                return *available;
            }
        }
        let available = gemma::probe_available(client);
        // Log ONCE at the probe boundary (never per file, never twice for one identity).
        // Identity only — the PROVIDER identity ("ollama"/"omlx") + the model tag ACTUALLY
        // in use (from the live client, NOT the hardcoded GEMMA_MODEL constant); NEVER the
        // base URL (which IS in `identity` — that string stays in-memory, never logged),
        // file content, or any path (privacy header).
        let provider = client.provider_label();
        let model = client.model_label();
        if available {
            eprintln!("censor gemma: model {model} available (local {provider})");
        } else {
            eprintln!(
                "censor gemma: model {model} unavailable via {provider} — local-AI review tier disabled (deterministic linters still run)"
            );
        }
        *cache = Some((identity, available));
        available
    }

    /// PHASE E read API: the current cached Gemma availability as a tri-state token
    /// the UI can render — `"available"`, `"offline"`, or `"unknown"` (not yet
    /// probed, i.e. no watch has started this session). Exposed so Phase E can show
    /// "Gemma layer offline" without itself probing Ollama. Reflects the MOST RECENT
    /// probe regardless of provider (a brief lock, not on the per-file path).
    #[allow(dead_code)] // first caller is Phase E (the Censor UI "Gemma offline" state).
    pub fn gemma_status(&self) -> &'static str {
        match self
            .gemma_probe
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            Some((_, true)) => "available",
            Some((_, false)) => "offline",
            None => "unknown",
        }
    }
}

/// Resolve + validate a project root supplied by the frontend. The root must be an
/// existing directory; everything downstream (shard paths, runner cwd) assumes a
/// real dir.
///
/// CONTRACT: the returned path is CANONICALIZED (`fs::canonicalize`) so a symlinked
/// root resolves to its real target and the shard paths are consistent
/// across calls (a symlink and its target both reduce to one canonical root, so two
/// `censor_start_watch` calls naming the same tree the two different ways are
/// correctly recognized as the same root by the idempotency check). This is not a
/// security boundary (the user selects their own project), just consistency. If
/// canonicalization fails (e.g. a permission quirk) we fall back to the raw path so
/// a watch still starts rather than hard-failing.
fn resolve_root(root: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(root);
    if !path.is_dir() {
        return Err(format!(
            "Project root is not a directory: {}",
            path.display()
        ));
    }
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

/// BLOCKER 2 + WARNING 3 — confine every censor command to a TRUSTED project root.
///
/// `resolve_root` alone only checks "is a directory", so an authenticated-but-
/// malicious caller (or a webview XSS) could pass `root=C:\Users\<me>` +
/// `file=NTUSER.DAT` and read/open/dispose against arbitrary files. This validator
/// canonicalizes `root` and verifies it equals a configured project `root_path`
/// from the trusted project list (`backend::projects::list_projects`, the SAME
/// resolution `polis::commands::project_root_map` uses), so the censor surface can
/// only ever operate inside a real, declared project tree.
///
/// `expected_project_id`:
///   - `None`  → the root must match SOME project (used by the board's project-less
///     reads: `censor_get_findings` / `censor_count_open` / `censor_status`);
///   - `Some(id)` → the root must match THAT SPECIFIC project's configured root
///     (used by the project-scoped commands: dispose / open / watch / review), so a
///     valid-but-wrong root cannot be paired with a foreign project id.
///
/// Returns the canonical root on success. Both the caller's root and each project's
/// declared root are canonicalized before comparison so a symlink/`.`/`..`-laden
/// but legitimate path still matches. A project whose root fails to canonicalize
/// (e.g. it was deleted) simply does not match.
fn validate_censor_root(
    app: &AppHandle,
    backend_state: &State<'_, BackendState>,
    root: &str,
    expected_project_id: Option<&str>,
) -> Result<PathBuf, String> {
    let canonical = resolve_root(root)?;
    let projects = crate::backend::projects::list_projects(app.clone(), backend_state.clone())
        .map_err(|e| format!("Could not load the project list to validate the Censor root: {e}"))?;
    // (id, declared_root) pairs from the trusted list; the pure matcher does the
    // canonicalize-and-compare so it is unit-testable without a Tauri app/state.
    let declared: Vec<(String, String)> = projects
        .into_iter()
        .filter_map(|p| p.root_path.map(|r| (p.id, r)))
        .collect();
    match_censor_root(&canonical, &declared, expected_project_id)
}

/// Pure core of [`validate_censor_root`]: given the caller's CANONICAL root and the
/// trusted `(project_id, declared_root)` pairs, return the canonical root iff it
/// matches an allowed declared root (canonicalized for comparison). `expected`
/// restricts the match to one project id (mismatch → reject). Factored out so the
/// confinement logic is testable without constructing a Tauri `AppHandle`/`State`.
fn match_censor_root(
    canonical: &Path,
    declared: &[(String, String)],
    expected: Option<&str>,
) -> Result<PathBuf, String> {
    for (id, root) in declared {
        if let Some(expected) = expected {
            if id != expected {
                continue;
            }
        }
        // Canonicalize the declared root the same way `resolve_root` does so the two
        // canonical forms are comparable; skip a declared root that no longer exists.
        let declared_canonical = match std::fs::canonicalize(PathBuf::from(root)) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if declared_canonical == *canonical {
            return Ok(canonical.to_path_buf());
        }
    }
    Err(match expected {
        Some(id) => format!(
            "Censor root does not match the configured root for project {id}; refusing access."
        ),
        None => "Censor root is not a known project root; refusing access.".to_string(),
    })
}

/// On-demand review pass. `file = Some(rel)` rechecks one file
/// (its FINE runners); `file = None` runs the whole-project COARSE sweep. Emits
/// `censor://findings-updated`.
///
/// Always runs on a detached worker thread so the command returns immediately;
/// the frontend learns the results via the findings-updated event.
/// timeouts so it cannot hang indefinitely).
#[tauri::command]
pub fn censor_review_now(
    project_id: String,
    root: String,
    file: Option<String>,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    censor: State<'_, CensorState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    // Confine to THIS project's configured root before any runner/shard IO.
    let path = validate_censor_root(&app, &backend_state, &root, Some(&project_id))?;
    // BLOCKER B: an on-demand review runs the project's OWN tool configs (RCE
    // surface). Refuse for an untrusted project so NO deterministic runner OR Gemma
    // is ever spawned for it. (Reading/disposing existing shards stays allowed —
    // those commands do not run tools.)
    if !crate::backend::projects::project_censor_trusted(&app, &project_id)? {
        return Err(CENSOR_UNTRUSTED_MSG.to_string());
    }
    // Validate a supplied file path (reject traversal / argv-injection) BEFORE any
    // runner sees it.
    if let Some(ref rel) = file {
        ledger::validate_rel_path(rel).map_err(|e| e.to_string())?;
    }
    // Always run as a one-shot on a detached worker.
    // WARNING F: hand the detached worker a clone of the shared "keep running" flag
    // so app exit (`kill_all_on_exit`) can flip it and abort the worker between
    // runners/passes rather than leaving an orphan thread + in-flight tool subprocess.
    let worker_app = app.clone();
    // WARNING F: hand the detached worker a clone of the shared "keep running" flag
    // so app exit (`kill_all_on_exit`) can flip it and abort the worker between
    // runners/passes rather than leaving an orphan thread + in-flight tool subprocess.
    let running = censor.oneshot_running_flag();
    let spawned = std::thread::Builder::new()
        .name("censor-review-now-oneshot".into())
        .spawn(move || {
            run_review_now_oneshot(&worker_app, project_id, path, file, running);
        });
    if let Err(e) = spawned {
        // A thread-spawn failure is a hard resource error; surface it rather than
        // silently dropping the requested review.
        return Err(format!("Failed to spawn Censor review worker: {e}"));
    }
    Ok(())
}

/// The detached one-shot `censor_review_now` fallback body.
/// Runs OFF the Tauri command thread (WARNING 4) so a slow Gemma probe/generate +
/// linters never block the IPC caller.
///
/// The Gemma tier uses the process-cached probe via the managed [`CensorState`]
/// (re-resolved from `app` here — we cannot move a `State` borrow across threads):
/// it reuses a prior watch's answer, or probes exactly once (gated) if this is the
/// first Gemma touch this session, so an on-demand recheck gets the same additive
/// Gemma layer as the fine passes. If the managed state is somehow
/// absent (teardown race) the tier is simply disabled (deterministic-only).
fn run_review_now_oneshot(
    app: &AppHandle,
    project_id: String,
    path: PathBuf,
    file: Option<String>,
    running: Arc<AtomicBool>,
) {
    use tauri::Manager;
    // Resolve the provider config ONCE and build the client from it (Ollama default OR
    // oMLX). One snapshot per one-shot run = probe and generate use the SAME provider.
    // `read_censor_local_ai` is fail-safe (missing/invalid ⇒ Ollama default).
    let local_ai = crate::backend::projects::read_censor_local_ai(app);
    let gemma_client = match gemma::build_gemma_client(&local_ai) {
        Ok(client) => Some(client),
        Err(e) => {
            eprintln!("censor gemma: {e}");
            None
        }
    };
    let gemma_available = match (app.try_state::<CensorState>(), gemma_client.as_deref()) {
        (Some(state), Some(client)) => state.ensure_gemma_probed(client),
        _ => false,
    };
    let gemma_params = local_ai.review_params();
    let gemma_ctx = gemma_client.as_deref().map(|client| GemmaCtx {
        client,
        available: gemma_available,
        params: gemma_params,
    });
    // WARNING F: pass the SHARED "keep running" flag straight through as the
    // orchestrator's stop-gate. The orchestrator re-reads it between passes/runners
    // and before each emit, so `kill_all_on_exit` flipping it to `false` aborts an
    // in-flight one-shot at its next checkpoint (no orphan thread, no stale event).
    // `Arc<AtomicBool>` derefs to `&AtomicBool` for the call.
    orchestrator::run_review_now(
        app,
        &project_id,
        &path,
        file.as_deref(),
        gemma_ctx,
        &running,
    );
}

/// Read OPEN findings for the board/panel. `file = Some(rel)` returns that file's
/// shard's open findings; `file = None` returns every shard's open findings across
/// the project. "Open" = `disposition == Open` (judged findings are hidden from the
/// active list but preserved on disk as the audit trail). Lock-free read.
#[tauri::command]
pub fn censor_get_findings(
    root: String,
    file: Option<String>,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<Vec<Finding>, String> {
    backend_state.ensure_unlocked()?;
    // Board read: the root must match SOME trusted project (rejects arbitrary dirs).
    let path = validate_censor_root(&app, &backend_state, &root, None)?;

    let shards: Vec<CensorShard> = match file {
        Some(rel) => {
            ledger::validate_rel_path(&rel).map_err(|e| e.to_string())?;
            ledger::read_shard(&path, &rel)
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect()
        }
        None => ledger::list_shards(&path).map_err(|e| e.to_string())?,
    };

    let mut open: Vec<Finding> = Vec::new();
    for shard in shards {
        for f in shard.findings {
            if f.disposition == Disposition::Open {
                open.push(f);
            }
        }
    }
    Ok(open)
}

/// Cheap count of OPEN findings across the project, for the board chip (Phase C).
/// Lock-free; reads the shard dir and sums open findings. A missing dir → 0.
#[tauri::command]
pub fn censor_count_open(
    root: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<u32, String> {
    backend_state.ensure_unlocked()?;
    // Board chip read: the root must match SOME trusted project.
    let path = validate_censor_root(&app, &backend_state, &root, None)?;
    let shards = ledger::list_shards(&path).map_err(|e| e.to_string())?;
    let count: usize = shards
        .iter()
        .flat_map(|s| s.findings.iter())
        .filter(|f| f.disposition == Disposition::Open)
        .count();
    Ok(count as u32)
}

/// One detected/absent linter for the Censor status payload. `available` reflects
/// a `command_exists` probe of the tool's executable; the UI uses an absent tool
/// to show a "tool not installed — that layer is skipped" hint without implying an
/// error.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CensorToolStatus {
    /// The runner's executable name (e.g. "clippy" surfaces as "cargo", "eslint").
    pub name: String,
    pub available: bool,
}

/// The Censor status payload for the UI: the cached Gemma availability plus the
/// detected/absent deterministic linters relevant to this project's kinds.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CensorStatus {
    /// `"available" | "offline" | "unknown"` from [`CensorState::gemma_status`].
    pub gemma_status: String,
    /// Linters relevant to this project (deduped by executable), each with a
    /// presence flag. Empty when the root is not a recognized project kind.
    pub tools: Vec<CensorToolStatus>,
    /// BLOCKER B: whether the user has trusted this project to RUN Censor. The
    /// panel uses this to show a "Trust this project to run Censor" prompt instead
    /// of (silently) running the repo's tool configs. `false` when no `project_id`
    /// was supplied (a board-level status read) or the project is untrusted.
    pub trusted: bool,
    /// COARSE policy: "off" | "manual" | "auto". Default "auto".
    pub coarse_policy: String,
    /// ISO timestamp of the last COARSE pass, or null if never run.
    pub last_coarse_run: Option<String>,
}

/// Pure: the deduped, order-stable list of linter executables relevant to a set of
/// detected project kinds, mapped through `probe` to a presence flag. Factored out
/// so the tool-detection logic is unit-testable without spawning `where.exe`/`sh`.
/// One entry per distinct EXECUTABLE (clippy/cargo-check/cargo-audit all share the
/// `cargo` program, so they collapse to a single `cargo` row).
fn detect_tools_with(
    kinds: &std::collections::HashSet<super::detect::ProjectKind>,
    probe: impl Fn(&str) -> bool,
) -> Vec<CensorToolStatus> {
    use super::detect::FileLang;
    use super::runners::applicable_runners;
    // Union of the runners that could apply across the languages of the detected
    // kinds, plus the cross-cutting set (always present via any lang). We ask for
    // each kind's representative language so kind-specific runners are included.
    let mut programs: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for lang in [
        FileLang::Rust,
        FileLang::Ts,
        FileLang::Py,
        FileLang::Go,
        FileLang::Cpp,
        FileLang::Html,
        FileLang::Kotlin,
        FileLang::Shell,
        FileLang::Yaml,
        FileLang::Sql,
        FileLang::Dockerfile,
        FileLang::GithubActions,
        FileLang::Css,
        FileLang::Other,
    ] {
        for runner in applicable_runners(kinds, lang) {
            let prog = runner.program().to_string();
            if seen.insert(prog.clone()) {
                programs.push(prog);
            }
        }
    }
    programs
        .into_iter()
        .map(|name| {
            let available = probe(&name);
            CensorToolStatus { name, available }
        })
        .collect()
}

/// UI status read: the cached Gemma availability + which linters are present for
/// this project. Lock-free + cheap (a handful of `command_exists` probes). Used by
/// the Censor panel to render "Gemma layer offline" and optional tool-absent hints.
/// Never starts or probes (it reuses the CACHED Gemma tri-state,
/// so before any watch has started this session it is `"unknown"`).
#[tauri::command]
pub fn censor_status(
    root: String,
    project_id: Option<String>,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    censor: State<'_, CensorState>,
) -> Result<CensorStatus, String> {
    backend_state.ensure_unlocked()?;
    // Status read: the root must match SOME trusted project. When a `project_id` is
    // supplied, confine to THAT project's root so the trust flag we report is for
    // the same project the panel is rendering.
    let path = validate_censor_root(&app, &backend_state, &root, project_id.as_deref())?;
    let kinds = super::detect::detect_project_kinds(&path);
    let tools = detect_tools_with(&kinds, |name| {
        crate::backend::projects::command_exists(name)
    });
    // BLOCKER B: surface the trust flag so the panel can prompt to trust rather than
    // run the repo's tool configs. Only resolvable with a project id; a board-level
    // status read (no id) reports `false` (the panel shows the prompt / no run).
    let trusted = match project_id.as_deref() {
        Some(id) => crate::backend::projects::project_censor_trusted(&app, id)?,
        None => false,
    };
    // COARSE policy: read from `.aspis/coarse_policy` file (default "auto").
    let coarse_policy = std::fs::read_to_string(path.join(".aspis").join("coarse_policy"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && matches!(s.as_str(), "off" | "manual" | "auto"))
        .unwrap_or_else(|| "auto".to_string());
    // Last COARSE run: read the timestamp file.
    let last_coarse_run = std::fs::read_to_string(path.join(".aspis").join("last_coarse_run"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(CensorStatus {
        gemma_status: censor.gemma_status().to_string(),
        tools,
        trusted,
        coarse_policy,
        last_coarse_run,
    })
}

/// Set the COARSE review policy for a project root.
#[tauri::command]
pub fn censor_set_coarse_policy(
    root: String,
    policy: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    if !matches!(policy.as_str(), "off" | "manual" | "auto") {
        return Err(format!(
            "Invalid coarse policy: {policy}. Must be off, manual, or auto."
        ));
    }
    let path = validate_censor_root(&app, &backend_state, &root, None)?;
    let dir = path.join(".aspis");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("coarse_policy"), &policy)
        .map_err(|e| format!("Failed to write coarse policy: {e}"))?;
    Ok(())
}

/// BLOCKER B: set (or clear) a project's Censor trust flag. Trusting a project
/// authorizes Censor to RUN the project's OWN tool configs from its root (eslint
/// plugins, cargo build scripts via clippy/check, custom semgrep rules) — i.e. to
/// execute repo-controlled code — so it must be an explicit user action. Until set,
/// `censor_review_now` stay inert. Persisted via the locked project write path;
/// NO-CHURN (the frontmatter line is omitted when false).
///
/// The filesystem watcher is GONE — no per-file trigger. All Censor runs are
/// on-demand (during mini-coder finalize) or on the coarse cooldown timer.
/// Setting `trusted = true`/`false` persists to project metadata.
#[tauri::command]
pub fn set_censor_trusted(
    project_id: String,
    trusted: bool,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    _censor: State<'_, CensorState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    crate::backend::projects::set_project_censor_trusted(&app, &project_id, trusted)?;
    // No active engine to tear down on revoke (the watcher is gone).
    // The trust flag gates future censor runs (they check it inline).
    Ok(())
}

/// Set a finding's disposition (e.g. mark a false positive) and append a
/// provenance entry, via the A1 LOCKED write path so a concurrent review pass /
/// the Python MCP writer cannot clobber it. Locates the finding by `id` within the
/// file's shard; if absent, returns an error (the UI passes the file the finding
/// belongs to).
#[tauri::command]
pub fn censor_dispose_finding(
    project_id: String,
    root: String,
    file: String,
    id: String,
    disposition: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    // Confine to THIS project's configured root (rejects a foreign/arbitrary root).
    let path = validate_censor_root(&app, &backend_state, &root, Some(&project_id))?;
    ledger::validate_rel_path(&file).map_err(|e| e.to_string())?;
    let new_disposition = parse_disposition(&disposition)?;

    ledger::dispose_finding(
        &path,
        &file,
        &id,
        new_disposition,
        &project_id,
        &super::now_stamp(),
    )
    .map_err(|e| e.to_string())
}

/// Parse a disposition token from the IPC boundary into the enum, rejecting an
/// unknown value (never silently default — a typo'd disposition must surface).
fn parse_disposition(token: &str) -> Result<Disposition, String> {
    match token {
        "open" => Ok(Disposition::Open),
        "fixed" => Ok(Disposition::Fixed),
        "fp" => Ok(Disposition::Fp),
        "wontfix" => Ok(Disposition::Wontfix),
        other => Err(format!("Unknown disposition: {other}")),
    }
}

/// Open a finding's source file in a chosen editor from the Censor panel's
/// clickable `file:line`. REUSES the Polis editor primitives verbatim: the file is
/// validated to be a real, regular file INSIDE the project root via
/// `polis::commands::resolve_editor_target` (canonicalize root + target, reject
/// `..`/absolute/symlink-escape, containment check) and launched through the same
/// fixed editor allowlist + no-shell `launch_editor`. Unlike Polis (which resolves
/// against the last-scanned map root), this takes the project `root` explicitly so
/// the open works whether or not Polis has scanned this project.
///
/// SECURITY: `resolve_editor_target` is the SAME root-containment guard Polis uses;
/// a `file` outside `root` (traversal / symlink) is rejected before any launch.
#[tauri::command]
pub fn censor_open_in_editor(
    project_id: String,
    root: String,
    file: String,
    editor: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
) -> Result<(), String> {
    // Posture match: launches a process against a local file → require unlock.
    backend_state.ensure_unlocked()?;
    let editor = editor.trim();
    if !crate::polis::commands::is_supported_editor(editor) {
        return Err("Unsupported editor".into());
    }
    // WARNING D: confine to THIS project's configured root (not just SOME project's)
    // before resolving the editor target, so a caller cannot pair a valid root for
    // project A with project B's id and open a file in a foreign project tree. An
    // arbitrary `root` + sibling `file` would otherwise pass `resolve_editor_target`'s
    // own containment check while still being outside the intended project.
    let path = validate_censor_root(&app, &backend_state, &root, Some(&project_id))?;
    // Defense in depth: reject the censor rel-path shapes (argv-injection / `..`)
    // BEFORE the editor-target validation also re-checks containment.
    ledger::validate_rel_path(&file).map_err(|e| e.to_string())?;
    let abs_path = crate::polis::commands::resolve_editor_target(&path, &file)?;
    crate::polis::commands::launch_editor(editor, &abs_path)
}

/// APP-EXIT teardown: signal in-flight one-shot workers to stop so quit /
/// dev Ctrl-C never orphans a worker thread or an in-flight tool subprocess.
/// Called from `lib.rs` `RunEvent::Exit`/`ExitRequested` next to the agent_pty +
/// Teardown. Idempotent: a missing managed state is a no-op.
pub fn kill_all_on_exit(app: &AppHandle) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<CensorState>() {
        // WARNING F: signal any in-flight detached one-shot review to stop FIRST, so
        // it aborts at its next orchestrator checkpoint instead of orphaning a thread
        // / tool subprocess past exit.
        state.signal_oneshots_stop();
    }
}


/// Format Censor findings as human-readable text for agent context.
/// Token budget: max 10 findings, max 4096 bytes, sorted by severity.
/// Skips findings that would push output past 4096 bytes (never includes partial findings).
/// Bodies are capped at 2000 chars to bound allocation.
#[allow(dead_code)] // called in Step 5 (inject into mini output)
pub fn format_findings_text(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&Finding> = findings.iter().collect();
    sorted.sort_by_key(|f| std::cmp::Reverse(f.severity.rank()));
    let mut out = String::new();
    for (i, f) in sorted.iter().enumerate() {
        if i >= 10 {
            break;
        }
        let icon = match f.severity {
            Severity::High => "🔴",
            Severity::Medium => "🟡",
            Severity::Low => "🟢",
        };
        let sev_str = match f.severity {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        };
        let cat_str = f.category.id_token();
        let line = f.line.map(|n| format!(":{n}")).unwrap_or_default();
        // Pre-truncate body to bound allocation
        let body: String = f.body.chars().take(2000).collect();
        let entry = format!(
            "{icon} [{sev_str}] {}\n  File: {}{}\n  Source: {} ({cat_str})\n  {}\n\n",
            f.title, f.file, line, f.source, body,
        );
        // Skip finding if it would push output past 4096 bytes
        // (never include a partial finding)
        if out.len() + entry.len() > 4096 {
            break;
        }
        out.push_str(&entry);
    }
    out
}

/// Wait up to `timeout` for Censor findings on the given files.
/// Polls the shard directory every 200ms until findings appear or timeout expires.
/// Capped at 50 findings across all files to bound state size.
#[allow(dead_code)] // called in Step 4 (finalize_finished_mini)
pub fn wait_for_censor_findings(root: &Path, files: &[String], timeout: Duration) -> Vec<Finding> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut all: Vec<Finding> = Vec::new();
        for file in files {
            if let Ok(Some(shard)) = ledger::read_shard(root, file) {
                for f in shard.findings {
                    if f.disposition == Disposition::Open {
                        all.push(f);
                        if all.len() >= 50 {
                            return all; // cap at 50 to bound state/event size
                        }
                    }
                }
            }
        }
        if !all.is_empty() || Instant::now() >= deadline {
            return all;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Drain all pending Censor queue batches. Reads `<root>/.aspis/censor_queue/pending/*.json`,
/// deletes each file after reading, returns deduplicated findings sorted by severity (High first).
pub fn drain_censor_queue(root: &Path) -> Vec<crate::backend::censor::schema::Finding> {
    let queue_dir = root.join(".aspis").join("censor_queue").join("pending");
    if !queue_dir.exists() {
        return vec![];
    }
    let mut batches: Vec<crate::backend::censor::schema::FindingBatch> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&queue_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    match serde_json::from_str::<crate::backend::censor::schema::FindingBatch>(
                        &content,
                    ) {
                        Ok(batch) => batches.push(batch),
                        Err(e) => {
                            eprintln!(
                                "censor queue: skipping corrupt batch {}: {e}",
                                path.display()
                            );
                            // Rename instead of delete so corrupt file is inspectable
                            let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
                            continue; // skip delete below
                        }
                    }
                }
                let _ = std::fs::remove_file(&path); // exactly-once delivery
            }
        }
    }
    batches.sort_by_key(|b| b.timestamp.clone());
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for batch in batches {
        for f in batch.findings {
            if seen.insert(f.id.clone()) {
                out.push(f);
            }
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.severity.rank()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::Category;
    use std::sync::atomic::AtomicU8;

    #[test]
    fn parse_disposition_accepts_known_rejects_unknown() {
        assert_eq!(parse_disposition("open").unwrap(), Disposition::Open);
        assert_eq!(parse_disposition("fp").unwrap(), Disposition::Fp);
        assert_eq!(parse_disposition("fixed").unwrap(), Disposition::Fixed);
        assert_eq!(parse_disposition("wontfix").unwrap(), Disposition::Wontfix);
        assert!(parse_disposition("bogus").is_err());
        assert!(parse_disposition("").is_err());
    }

    #[test]
    // ---- wait_for_censor_findings tests ----
    fn wait_for_censor_findings_returns_empty_when_no_shards() {
        let root = std::env::temp_dir().join(format!("aspis-censor-empty-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let got = wait_for_censor_findings(&root, &["src/a.rs".into()], Duration::from_secs(1));
        assert!(got.is_empty(), "no shards → empty");
    }

    #[test]
    fn wait_for_censor_findings_finds_preexisting_shard() {
        let root = std::env::temp_dir().join(format!("aspis-censor-shard-{}", std::process::id()));
        let censor_dir = root.join(".aspis-censor");
        std::fs::create_dir_all(&censor_dir).unwrap();
        let shard = CensorShard {
            file_rel_path: "src/a.rs".into(),
            content_hash: "h".into(),
            updated_at: "t".into(),
            findings: vec![Finding {
                id: "f1".into(),
                file: "src/a.rs".into(),
                severity: Severity::High,
                category: Category::Correctness,
                source: "clippy".into(),
                title: "unused".into(),
                body: "x is unused".into(),
                disposition: Disposition::Open,
                ..Default::default()
            }],
        };
        ledger::write_shard(&root, &shard);
        let got = wait_for_censor_findings(&root, &["src/a.rs".into()], Duration::from_secs(2));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "unused");
    }

    #[test]
    fn wait_for_censor_findings_skips_non_open_disposition() {
        let root = std::env::temp_dir().join(format!("aspis-censor-skip-{}", std::process::id()));
        let censor_dir = root.join(".aspis-censor");
        std::fs::create_dir_all(&censor_dir).unwrap();
        let shard = CensorShard {
            file_rel_path: "src/a.rs".into(),
            content_hash: "h".into(),
            updated_at: "t".into(),
            findings: vec![
                Finding {
                    id: "f1".into(),
                    disposition: Disposition::Fixed,
                    ..Default::default()
                },
                Finding {
                    id: "f2".into(),
                    disposition: Disposition::Fp,
                    ..Default::default()
                },
                Finding {
                    id: "f3".into(),
                    disposition: Disposition::Wontfix,
                    ..Default::default()
                },
            ],
        };
        ledger::write_shard(&root, &shard);
        let got = wait_for_censor_findings(&root, &["src/a.rs".into()], Duration::from_secs(2));
        assert!(got.is_empty(), "Fixed/Fp/Wontfix must be skipped");
    }

    #[test]
    fn wait_for_censor_findings_times_out_when_no_shard_appears() {
        let root = std::env::temp_dir().join(format!("aspis-censor-poll-{}", std::process::id()));
        let _censor_dir = root.join(".aspis-censor");
        std::fs::create_dir_all(&_censor_dir).unwrap();
        let start = std::time::Instant::now();
        // No shard for "src/x.rs" — must poll until timeout
        let got = wait_for_censor_findings(&root, &["src/x.rs".into()], Duration::from_millis(500));
        let elapsed = start.elapsed();
        assert!(got.is_empty(), "no shard → empty");
        assert!(
            elapsed >= Duration::from_millis(400),
            "must wait at least ~400ms, got {:?}",
            elapsed
        );
    }

    // ---- format_findings_text tests ----

    #[test]
    fn format_findings_text_empty_returns_empty() {
        assert_eq!(format_findings_text(&[]), "");
    }

    #[test]
    fn format_findings_text_sorts_high_before_medium_before_low() {
        let findings = vec![
            Finding {
                severity: Severity::Low,
                title: "low1".into(),
                ..Default::default()
            },
            Finding {
                severity: Severity::High,
                title: "high1".into(),
                ..Default::default()
            },
            Finding {
                severity: Severity::Medium,
                title: "mid1".into(),
                ..Default::default()
            },
        ];
        let text = format_findings_text(&findings);
        let hi = text.find("high1").unwrap();
        let mi = text.find("mid1").unwrap();
        let lo = text.find("low1").unwrap();
        assert!(hi < mi, "High must appear before Medium in output");
        assert!(mi < lo, "Medium must appear before Low in output");
    }

    #[test]
    fn format_findings_text_caps_at_ten_findings() {
        let findings: Vec<Finding> = (0..15)
            .map(|i| Finding {
                title: format!("finding-{i}"),
                severity: Severity::Medium,
                source: "clippy".into(),
                ..Default::default()
            })
            .collect();
        let text = format_findings_text(&findings);
        let count = text.matches("🟡").count();
        assert!(count <= 10, "max 10 findings, got {count}");
    }

    #[test]
    fn format_findings_text_caps_total_size_at_4096_bytes() {
        let big_body = "x".repeat(6000);
        let findings = vec![Finding {
            body: big_body,
            severity: Severity::High,
            source: "clippy".into(),
            ..Default::default()
        }];
        let text = format_findings_text(&findings);
        assert!(
            text.len() <= 4096,
            "must cap at 4096 bytes, got {}",
            text.len()
        );
    }

    // ---- BLOCKER 2 + WARNING 3: censor root confinement (pure matcher) ----

    /// A real, canonicalizable dir under the temp tree (so `canonicalize` succeeds
    /// for both the caller root and the declared root in the matcher).
    fn real_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aspis-censor-root-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn match_censor_root_rejects_root_not_in_project_list() {
        let outside = real_dir("outside");
        let known = real_dir("known");
        let declared = vec![("p1".to_string(), known.to_string_lossy().into_owned())];
        // A root that is a real dir but NOT a declared project root is refused.
        let err = match_censor_root(&outside, &declared, None).unwrap_err();
        assert!(err.contains("not a known project root"), "got: {err}");
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&known);
    }

    #[test]
    fn match_censor_root_rejects_root_project_id_mismatch() {
        // The root is a valid project root, but it belongs to p1 while the caller
        // claims p2 → reject (a valid-but-wrong root cannot be paired with a foreign
        // project id).
        let root_p1 = real_dir("mismatch-p1");
        let root_p2 = real_dir("mismatch-p2");
        let declared = vec![
            ("p1".to_string(), root_p1.to_string_lossy().into_owned()),
            ("p2".to_string(), root_p2.to_string_lossy().into_owned()),
        ];
        let err = match_censor_root(&root_p1, &declared, Some("p2")).unwrap_err();
        assert!(
            err.contains("does not match the configured root for project p2"),
            "got: {err}"
        );
        let _ = std::fs::remove_dir_all(&root_p1);
        let _ = std::fs::remove_dir_all(&root_p2);
    }

    #[test]
    fn match_censor_root_accepts_valid_project_root() {
        let root = real_dir("valid");
        let declared = vec![("p1".to_string(), root.to_string_lossy().into_owned())];
        // Project-less (board) check: any declared root passes.
        assert_eq!(match_censor_root(&root, &declared, None).unwrap(), root);
        // Project-scoped check: the matching id passes.
        assert_eq!(
            match_censor_root(&root, &declared, Some("p1")).unwrap(),
            root
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn match_censor_root_skips_declared_root_that_no_longer_exists() {
        // A declared root that cannot canonicalize (deleted) is skipped, not matched.
        let real = real_dir("exists");
        let gone = std::env::temp_dir().join(format!("aspis-censor-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&gone);
        let declared = vec![
            ("p-gone".to_string(), gone.to_string_lossy().into_owned()),
            ("p-real".to_string(), real.to_string_lossy().into_owned()),
        ];
        // The caller root matches the REAL project, not the gone one.
        assert_eq!(match_censor_root(&real, &declared, None).unwrap(), real);
        // And the gone project id can never match (its root is unresolvable).
        assert!(match_censor_root(&real, &declared, Some("p-gone")).is_err());
        let _ = std::fs::remove_dir_all(&real);
    }

    #[test]
    fn open_in_editor_root_must_match_its_own_project() {
        // WARNING D: `censor_open_in_editor` now passes `Some(project_id)` to the
        // matcher, so a root that is valid for project A cannot be opened under
        // project B's id. (The matcher is the same one the command uses.)
        let root_a = real_dir("open-a");
        let root_b = real_dir("open-b");
        let declared = vec![
            ("a".to_string(), root_a.to_string_lossy().into_owned()),
            ("b".to_string(), root_b.to_string_lossy().into_owned()),
        ];
        // Project A's root paired with project B's id → rejected.
        assert!(match_censor_root(&root_a, &declared, Some("b")).is_err());
        // Project A's root paired with its OWN id → accepted.
        assert_eq!(
            match_censor_root(&root_a, &declared, Some("a")).unwrap(),
            root_a
        );
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    #[test]
    // ---- PHASE E: censor_status tool detection is pure + deduped by executable ----
    #[test]
    fn detect_tools_dedupes_cargo_and_includes_cross_cutting() {
        use super::super::detect::ProjectKind;
        let mut kinds = std::collections::HashSet::new();
        kinds.insert(ProjectKind::Rust);
        // Probe reports everything present so we test the SET + dedup, not presence.
        let tools = detect_tools_with(&kinds, |_| true);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // clippy/cargo-check/cargo-audit all share the `cargo` executable → ONE row.
        assert_eq!(names.iter().filter(|n| **n == "cargo").count(), 1);
        // Cross-cutting tools always present for any kind.
        for cross in ["gitleaks", "jscpd", "lizard", "semgrep"] {
            assert!(names.contains(&cross), "missing cross-cutting tool {cross}");
        }
        assert!(tools.iter().all(|t| t.available));
    }

    #[test]
    fn detect_tools_reflects_absent_probe() {
        use super::super::detect::ProjectKind;
        let mut kinds = std::collections::HashSet::new();
        kinds.insert(ProjectKind::Node);
        // Only `eslint` is "installed"; everything else is absent.
        let tools = detect_tools_with(&kinds, |name| name == "eslint");
        let eslint = tools
            .iter()
            .find(|t| t.name == "eslint")
            .expect("eslint row");
        assert!(eslint.available, "eslint probed present");
        let gitleaks = tools
            .iter()
            .find(|t| t.name == "gitleaks")
            .expect("gitleaks row");
        assert!(!gitleaks.available, "gitleaks probed absent");
        // tsc is a Node runner and must appear (absent here).
        assert!(tools.iter().any(|t| t.name == "tsc" && !t.available));
    }

    #[test]
    fn detect_tools_includes_go_executables_for_go_project() {
        use super::super::detect::ProjectKind;
        let mut kinds = std::collections::HashSet::new();
        kinds.insert(ProjectKind::Go);
        let tools = detect_tools_with(&kinds, |_| true);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // gofmt (its own executable) and `go` (go vet) both appear for a Go project.
        assert!(names.contains(&"gofmt"), "missing gofmt in {names:?}");
        assert!(names.contains(&"go"), "missing go (go vet) in {names:?}");
        // A non-Go project does NOT surface the Go executables.
        let mut rust = std::collections::HashSet::new();
        rust.insert(ProjectKind::Rust);
        let rust_tools = detect_tools_with(&rust, |_| true);
        let rust_names: Vec<&str> = rust_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !rust_names.contains(&"gofmt"),
            "gofmt leaked into a Rust project"
        );
    }

    #[test]
    fn detect_tools_includes_cppcheck_for_cpp_project() {
        use super::super::detect::ProjectKind;
        let mut kinds = std::collections::HashSet::new();
        kinds.insert(ProjectKind::Cpp);
        let tools = detect_tools_with(&kinds, |_| true);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // cppcheck appears for a C/C++ project.
        assert!(names.contains(&"cppcheck"), "missing cppcheck in {names:?}");
        // A non-C/C++ project does NOT surface cppcheck.
        let mut rust = std::collections::HashSet::new();
        rust.insert(ProjectKind::Rust);
        let rust_tools = detect_tools_with(&rust, |_| true);
        let rust_names: Vec<&str> = rust_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !rust_names.contains(&"cppcheck"),
            "cppcheck leaked into a Rust project"
        );
    }

    #[test]
    fn detect_tools_includes_tidy_regardless_of_project_kind() {
        use super::super::detect::ProjectKind;
        // HTML has NO ProjectKind, so tidy is surfaced for EVERY project (and for none) —
        // the probe loop includes FileLang::Html, which applies tidy on the lang alone.
        for kset in [
            std::collections::HashSet::new(),
            std::collections::HashSet::from([ProjectKind::Rust]),
        ] {
            let tools = detect_tools_with(&kset, |_| true);
            let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(names.contains(&"tidy"), "missing tidy for kinds {kset:?}");
        }
    }

    #[test]
    fn detect_tools_includes_shellcheck_yamllint_sqlfluff_regardless_of_project_kind() {
        use super::super::detect::ProjectKind;
        // Shell/YAML/SQL have NO ProjectKind, so their runners surface for EVERY project
        // (and for none) — the probe loop includes FileLang::Shell/Yaml/Sql, which apply
        // their runner on the lang alone.
        for kset in [
            std::collections::HashSet::new(),
            std::collections::HashSet::from([ProjectKind::Rust]),
            std::collections::HashSet::from([ProjectKind::Node, ProjectKind::Python]),
        ] {
            let tools = detect_tools_with(&kset, |_| true);
            let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(
                names.contains(&"shellcheck"),
                "missing shellcheck for {kset:?}"
            );
            assert!(names.contains(&"yamllint"), "missing yamllint for {kset:?}");
            assert!(names.contains(&"sqlfluff"), "missing sqlfluff for {kset:?}");
        }
    }

    #[test]
    fn detect_tools_includes_hadolint_actionlint_stylelint_regardless_of_project_kind() {
        use super::super::detect::ProjectKind;
        // Dockerfile/GithubActions/CSS have NO ProjectKind, so their runners surface for
        // EVERY project (and for none) — the probe loop includes
        // FileLang::Dockerfile/GithubActions/Css, which apply their runner on the lang alone.
        for kset in [
            std::collections::HashSet::new(),
            std::collections::HashSet::from([ProjectKind::Rust]),
            std::collections::HashSet::from([ProjectKind::Node, ProjectKind::Python]),
        ] {
            let tools = detect_tools_with(&kset, |_| true);
            let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(names.contains(&"hadolint"), "missing hadolint for {kset:?}");
            assert!(
                names.contains(&"actionlint"),
                "missing actionlint for {kset:?}"
            );
            assert!(
                names.contains(&"stylelint"),
                "missing stylelint for {kset:?}"
            );
        }
    }

    #[test]
    fn detect_tools_includes_ktlint_for_kotlin_project() {
        use super::super::detect::ProjectKind;
        let mut kinds = std::collections::HashSet::new();
        kinds.insert(ProjectKind::Kotlin);
        let tools = detect_tools_with(&kinds, |_| true);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // ktlint appears for a Kotlin project.
        assert!(names.contains(&"ktlint"), "missing ktlint in {names:?}");
        // A non-Kotlin project does NOT surface ktlint.
        let mut rust = std::collections::HashSet::new();
        rust.insert(ProjectKind::Rust);
        let rust_tools = detect_tools_with(&rust, |_| true);
        let rust_names: Vec<&str> = rust_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !rust_names.contains(&"ktlint"),
            "ktlint leaked into a Rust project"
        );
    }

    #[test]
    fn detect_tools_empty_kinds_is_lang_only_runners_plus_cross_cutting() {
        let kinds = std::collections::HashSet::new();
        let tools = detect_tools_with(&kinds, |_| true);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // No project kind → no KIND-gated runners. The cross-cutting set is seen first
        // (every probed lang appends it). The LANG-ONLY runners (no manifest, so they
        // apply everywhere) are the extras, appended in probe order AFTER the cross-
        // cutting set is already seen: HTML→tidy, Shell→shellcheck, YAML→yamllint,
        // SQL→sqlfluff, Dockerfile→hadolint, GithubActions→actionlint, CSS→stylelint.
        assert_eq!(
            names,
            vec![
                "gitleaks",
                "jscpd",
                "lizard",
                "semgrep",
                "zizmor",
                "tidy",
                "shellcheck",
                "yamllint",
                "sqlfluff",
                "hadolint",
                "actionlint",
                "stylelint"
            ]
        );
    }

    // ---- WARNING 1 / N3: the one-time Gemma probe runs ONCE under concurrency ----

    /// A probe-counting stub client (no network). `probe()` increments a shared
    /// counter so the test can assert it ran exactly once even under contention.
    struct CountingProbeClient {
        result: bool,
        probes: std::sync::Arc<AtomicU8>,
    }
    impl gemma::GemmaClient for CountingProbeClient {
        fn probe(&self) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            // A tiny sleep widens the race window so two threads genuinely contend on
            // the gate (without it both might serialize trivially on the OS scheduler).
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.result
        }
        fn generate(&self, _prompt: &str) -> Result<String, gemma::GemmaError> {
            Ok(String::new())
        }
        fn provider_label(&self) -> &'static str {
            "stub"
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
    }

    #[test]
    fn ensure_gemma_probed_probes_exactly_once_under_concurrency() {
        let st = std::sync::Arc::new(CensorState::new());
        let probes = std::sync::Arc::new(AtomicU8::new(0));

        // Spawn several threads that all hit the cold cache simultaneously. Each owns
        // its own client (the real call site builds one per thread too), all sharing
        // the SAME probe counter, so we count TOTAL probe round-trips across threads.
        let mut handles = Vec::new();
        for _ in 0..8 {
            let st = st.clone();
            let probes = probes.clone();
            handles.push(std::thread::spawn(move || {
                let client = CountingProbeClient {
                    result: true,
                    probes,
                };
                st.ensure_gemma_probed(&client)
            }));
        }
        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Exactly ONE probe happened despite 8 concurrent callers (the gate + the
        // double-check serialize it), and every caller agrees on the answer.
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "the probe must run exactly once"
        );
        assert!(
            results.iter().all(|&r| r),
            "all callers see the cached available=true"
        );
        assert_eq!(st.gemma_status(), "available");

        // A subsequent call uses the fast path — still no new probe.
        let client = CountingProbeClient {
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&client));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "a later call hits the cache, no re-probe"
        );
    }

    #[test]
    fn gemma_status_unknown_before_probe_then_reflects_result() {
        let st = CensorState::new();
        assert_eq!(st.gemma_status(), "unknown", "no probe yet → unknown");
        let probes = std::sync::Arc::new(AtomicU8::new(0));
        let client = CountingProbeClient {
            result: false,
            probes,
        };
        assert!(!st.ensure_gemma_probed(&client));
        assert_eq!(
            st.gemma_status(),
            "offline",
            "an unavailable probe → offline"
        );
    }

    /// F2: a probe-counting stub whose `provider_label()` is configurable, so a test can
    /// drive the SAME `CensorState` cache with two different providers and assert it
    /// re-probes on a provider switch (the cache is provider-keyed).
    struct ProviderStub {
        provider: &'static str,
        result: bool,
        probes: std::sync::Arc<AtomicU8>,
    }
    impl gemma::GemmaClient for ProviderStub {
        fn probe(&self) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.result
        }
        fn generate(&self, _prompt: &str) -> Result<String, gemma::GemmaError> {
            Ok(String::new())
        }
        fn provider_label(&self) -> &'static str {
            self.provider
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
        // Fold the provider into the identity (the model is fixed), so this stub still
        // exercises the provider-switch cache miss exactly as before.
        fn cache_identity(&self) -> String {
            format!("{}|stub-base|stub-model", self.provider)
        }
    }

    /// max-recall FIX 4: a probe-counting stub whose `cache_identity()` varies by BASE
    /// (same provider + model), so a test can prove the cache re-probes when only the
    /// oMLX base changes — the bug a provider-only key missed.
    struct BaseStub {
        base: &'static str,
        result: bool,
        probes: std::sync::Arc<AtomicU8>,
    }
    impl gemma::GemmaClient for BaseStub {
        fn probe(&self) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.result
        }
        fn generate(&self, _prompt: &str) -> Result<String, gemma::GemmaError> {
            Ok(String::new())
        }
        fn provider_label(&self) -> &'static str {
            "omlx"
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
        fn cache_identity(&self) -> String {
            format!("omlx|{}|stub-model", self.base)
        }
    }

    #[test]
    fn ensure_gemma_probed_reprobes_on_base_change_same_provider() {
        // max-recall FIX 4 (stale-cache fix): the cache is keyed on the FULL identity, so
        // changing ONLY the oMLX base (same provider + model) is a cache MISS and re-probes
        // — a stale answer for the OLD base must NOT drive the tier for the NEW base.
        let st = CensorState::new();
        let probes = std::sync::Arc::new(AtomicU8::new(0));

        // 1) Warm the cache for base A (probe #1, available).
        let a = BaseStub {
            base: "http://localhost:8000/v1",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&a));
        assert_eq!(probes.load(Ordering::SeqCst), 1, "base A probed once");

        // 2) A second call on the SAME base hits the cache — NO new probe.
        let a2 = BaseStub {
            base: "http://localhost:8000/v1",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&a2));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "same identity reuses the cache (no re-probe)"
        );

        // 3) Change ONLY the base (same provider+model): must re-probe (#2). Base B is
        //    DOWN (result=false) — a stale reuse of base A's available=true would be the
        //    exact bug (tier driven against a different, down endpoint).
        let b = BaseStub {
            base: "http://127.0.0.1:9000/v1",
            result: false,
            probes: probes.clone(),
        };
        assert!(
            !st.ensure_gemma_probed(&b),
            "a base change must re-probe, not reuse base A's answer"
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "a base change (same provider) re-probes (cache miss on identity mismatch)"
        );
        assert_eq!(
            st.gemma_status(),
            "offline",
            "status reflects the latest (base B) probe"
        );
    }

    #[test]
    fn ensure_gemma_probed_is_provider_keyed_reprobes_on_switch() {
        // F2 (stale-cache fix): a warm cache for one provider must NOT be reused by a
        // client of a DIFFERENT provider; a provider switch re-probes (and stores the new
        // provider's answer), while a SAME-provider call still hits the cache.
        let st = CensorState::new();
        let probes = std::sync::Arc::new(AtomicU8::new(0));

        // 1) Warm the cache for ollama (probe #1).
        let ollama = ProviderStub {
            provider: "ollama",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&ollama));
        assert_eq!(probes.load(Ordering::SeqCst), 1, "ollama probed once");

        // 2) A second ollama call hits the cache — NO new probe.
        let ollama2 = ProviderStub {
            provider: "ollama",
            result: true,
            probes: probes.clone(),
        };
        assert!(st.ensure_gemma_probed(&ollama2));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "same-provider call reuses the cache (no re-probe)"
        );

        // 3) Switch to omlx: the warm ollama answer must NOT be reused — re-probe (#2).
        //    The omlx server is DOWN here (result=false), so a stale reuse of ollama's
        //    available=true would be the exact bug (tier driven against a down daemon).
        let omlx = ProviderStub {
            provider: "omlx",
            result: false,
            probes: probes.clone(),
        };
        assert!(
            !st.ensure_gemma_probed(&omlx),
            "omlx must re-probe, not reuse the ollama answer"
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "a provider switch re-probes (cache miss on provider mismatch)"
        );
        assert_eq!(
            st.gemma_status(),
            "offline",
            "status reflects the latest (omlx) probe"
        );

        // 4) A second omlx call now hits the (re-keyed) cache — NO new probe.
        let omlx2 = ProviderStub {
            provider: "omlx",
            result: false,
            probes: probes.clone(),
        };
        assert!(!st.ensure_gemma_probed(&omlx2));
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "the cache is now keyed to omlx — same-provider call reuses it"
        );
    }

    fn test_root(tag: &str) -> PathBuf {
        // A distinct, existing dir per tag (canonicalization-agnostic — we only need
        // path equality between handle.root() and the stored slot).
        let dir =
            std::env::temp_dir().join(format!("aspis-censor-cmd-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    // ---- BLOCKER 3+4: atomic install — second start same root is a no-op; a
    //                   different root cleanly replaces (one active handle). ----

    #[test]
    #[test]
    #[test]
    #[test]
    #[test]
    #[test]
    fn drain_censor_queue_returns_empty_when_no_dir() {
        let tmp = std::env::temp_dir().join(format!("aspis-dq-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let got = drain_censor_queue(&tmp);
        assert!(got.is_empty());
    }

    #[test]
    fn drain_censor_queue_drains_and_deletes_batches() {
        use crate::backend::censor::schema::{Category, Finding, FindingBatch, Severity};
        let tmp = std::env::temp_dir().join(format!("aspis-dq-drain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let queue_dir = tmp.join(".aspis").join("censor_queue").join("pending");
        std::fs::create_dir_all(&queue_dir).unwrap();
        let f1 = Finding {
            id: "id1".into(),
            severity: Severity::High,
            title: "bug1".into(),
            file: "src/a.rs".into(),
            source: "clippy".into(),
            category: Category::Correctness,
            ..Default::default()
        };
        let f2 = Finding {
            id: "id2".into(),
            severity: Severity::Medium,
            title: "bug2".into(),
            file: "src/b.rs".into(),
            source: "semgrep".into(),
            category: Category::Security,
            ..Default::default()
        };
        let batch = FindingBatch {
            batch_id: "t1".into(),
            timestamp: "2026-06-30T12:00:00Z".into(),
            pass_type: "coarse".into(),
            files: vec!["src/a.rs".into()],
            findings: vec![f1, f2],
        };
        std::fs::write(
            queue_dir.join("t1.json"),
            serde_json::to_string(&batch).unwrap(),
        )
        .unwrap();
        let got = drain_censor_queue(&tmp);
        assert_eq!(got.len(), 2);
        assert!(!queue_dir.join("t1.json").exists());
    }

    #[test]
    fn drain_censor_queue_deduplicates_by_id() {
        use crate::backend::censor::schema::{Category, Finding, FindingBatch, Severity};
        let tmp = std::env::temp_dir().join(format!("aspis-dq-dedup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let queue_dir = tmp.join(".aspis").join("censor_queue").join("pending");
        std::fs::create_dir_all(&queue_dir).unwrap();
        let f = Finding {
            id: "same".into(),
            severity: Severity::High,
            title: "dup".into(),
            file: "src/a.rs".into(),
            source: "clippy".into(),
            category: Category::Correctness,
            ..Default::default()
        };
        for ts in ["a", "b"] {
            let batch = FindingBatch {
                batch_id: ts.into(),
                timestamp: format!("{ts}Z"),
                pass_type: "coarse".into(),
                files: vec!["src/a.rs".into()],
                findings: vec![f.clone()],
            };
            std::fs::write(
                queue_dir.join(format!("{ts}.json")),
                serde_json::to_string(&batch).unwrap(),
            )
            .unwrap();
        }
        let got = drain_censor_queue(&tmp);
        assert_eq!(got.len(), 1, "deduplicated");
    }
}
