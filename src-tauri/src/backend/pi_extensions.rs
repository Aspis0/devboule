//! pi extensions backend — agent-dir resolution, bootstrap, install/remove/list/search.
//!
//! Manages the pi coding-agent extension registry (`settings.json` → `packages` array),
//! the bundled CLI (`pi-sidecar/node_modules/.bin/pi`), and the npm marketplace search.
//!
//! The pi agent dir is resolved in priority order:
//! 1. `DEVBOULE_PI_AGENT_DIR` env (dev override — tilde IS expanded, mirroring the CLI)
//! 2. `~/.pi/agent` IF it exists (a pi user's extensions Just Work — product decision)
//! 3. `<app_data_dir>/pi-agent` (app-managed; created on demand; bootstrap target)
//!
//! Design doc: this module is self-contained — do NOT merge into pi_sidecar.rs.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Curated extension set installed on first launch (app-managed mode only).
const CURATED_EXTENSIONS: &[&str] = &[
    "npm:@tintinweb/pi-subagents",
    "npm:pi-lens",
    "npm:@pi-unipi/compactor",
    "npm:pi-web-access",
];

/// Hard timeout for a single CLI invocation (seconds).
const CLI_TIMEOUT_SECS: u64 = 180;

/// npm registry search URL (FIXED host — no SSRF surface).
const NPM_SEARCH_URL: &str = "https://registry.npmjs.org/-/v1/search";

/// HTTP timeout for marketplace queries.
const MARKETPLACE_TIMEOUT_SECS: u64 = 10;

/// The ecosystem keyword that identifies pi packages on npm.
const PI_PACKAGE_KEYWORD: &str = "pi-package";

/// Cap for the success output tail returned by `run_pi_cli_at`.
const CLI_OUTPUT_TAIL_CHARS: usize = 800;

/// Max body bytes we will read from the npm marketplace search response (2 MB).
const MARKETPLACE_MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Agent dir resolution
// ---------------------------------------------------------------------------

/// Which tier of the agent dir resolution was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentDirMode {
    /// `DEVBOULE_PI_AGENT_DIR` env was set — we use that path (tilde expanded).
    EnvOverride,
    /// `~/.pi/agent` existed on disk — a pi power-user's extensions Just Work.
    Global,
    /// App-managed dir under `<app_data_dir>/pi-agent` (created on demand).
    AppManaged,
}

/// Result of resolving the pi agent directory.
#[derive(Debug, Clone)]
pub struct ResolvedAgentDir {
    pub path: PathBuf,
    pub mode: AgentDirMode,
}

/// Home directory resolution. Mirrors `saved_workflows::home_dir` — env-var
/// based, no external crate.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expand a leading `~/` or bare `~` to the user's home directory, matching
/// the pi CLI's `expandTildePath` (config.js). Absolute paths and anything
/// else pass through unchanged.
fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
        // home unknown — return as-is (will fail downstream on missing dir).
        return p.to_path_buf();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    p.to_path_buf()
}

/// Pure decision core: pick which agent dir to use.
///
/// Priority: env_override → global (if exists) → app_managed.
/// Unit-testable without an AppHandle.
fn pick_agent_dir(
    env_override: Option<PathBuf>,
    global_exists: bool,
    global: PathBuf,
    app_managed: PathBuf,
) -> (PathBuf, AgentDirMode) {
    if let Some(dir) = env_override {
        return (dir, AgentDirMode::EnvOverride);
    }
    if global_exists {
        return (global, AgentDirMode::Global);
    }
    (app_managed, AgentDirMode::AppManaged)
}

/// Resolve the pi agent directory for the current machine.
pub fn resolve_pi_agent_dir(app: &AppHandle) -> Result<ResolvedAgentDir, String> {
    // 1. Dev override via env — tilde IS expanded (mirrors the CLI).
    let env_override = std::env::var("DEVBOULE_PI_AGENT_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| expand_tilde(Path::new(&v)));

    // 2. Global ~/.pi/agent (product decision: pi user's extensions Just Work).
    //    When home_dir() is None, the global dir cannot exist — skip to app-managed.
    let (global, global_exists) = match home_dir() {
        Some(home) => {
            let dir = home.join(".pi").join("agent");
            let exists = dir.is_dir();
            (dir, exists)
        }
        None => (PathBuf::new(), false),
    };

    // 3. App-managed under <app_data_dir>/pi-agent.
    let app_managed = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("App data directory is unavailable: {e}"))?
        .join("pi-agent");

    let (path, mode) = pick_agent_dir(env_override, global_exists, global, app_managed);
    Ok(ResolvedAgentDir { path, mode })
}

// ---------------------------------------------------------------------------
// Bundled CLI resolution
// ---------------------------------------------------------------------------

/// Pure helper: walk an ordered list of candidate `pi-sidecar` directories
/// looking for `target` (relative to each candidate). No sibling check — the
/// CLI binary IS inside node_modules. Returns the first existing candidate,
/// or an error listing every path tried.
fn resolve_cli_candidates(
    candidates: &[PathBuf],
    target: &str,
) -> Result<PathBuf, String> {
    let mut tried = Vec::new();
    for base in candidates {
        let path = base.join(target);
        tried.push(path.display().to_string());
        if path.exists() {
            return Ok(path);
        }
    }
    Err(format!(
        "pi CLI not found (looked for {target} under each candidate dir). \
         Tried: {}. Run `npm install` in pi-sidecar/ first.",
        tried.join(", ")
    ))
}

/// Resolve the path to the bundled `pi` CLI entry point.
///
/// Resolution order (mirrors `resolve_sidecar_script` in pi_sidecar.rs):
/// 1. `cwd/pi-sidecar/node_modules/.bin/pi` — covers repo-root launches and
///    the packaged-exe-with-project-cwd convention.
/// 2. (debug-only) `CARGO_MANIFEST_DIR.parent()/pi-sidecar/node_modules/.bin/pi`
///    — CARGO_MANIFEST_DIR is baked at compile time as the absolute path of
///    src-tauri, so its parent is the repo root. Gated behind debug_assertions
///    so a release build on the build machine does NOT silently shadow
///    `resource_dir()` — the baked source-tree path exists locally and would
///    prevent the packaged-resources leg from being exercised, hiding bundling
///    bugs until end-user machines.
/// 3. `app.path().resource_dir()/pi-sidecar/node_modules/.bin/pi` — the REAL
///    packaged-build location (tauri.conf.json bundles `../pi-sidecar` via
///    bundle.resources).
/// 4. `TAURI_RESOURCE_DIR/pi-sidecar/node_modules/.bin/pi` — NOT a Tauri-provided
///    env var; kept as an explicit manual override / escape hatch.
fn resolve_bundled_pi_cli(app: &AppHandle) -> Result<PathBuf, String> {
    let target = Path::new("node_modules").join(".bin").join("pi");

    let mut candidates = Vec::new();

    // 1. CWD.
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("pi-sidecar"));
    }

    // 2. (debug-only) Compile-time repo root via CARGO_MANIFEST_DIR.
    // Gated behind debug_assertions so a release build on the build machine
    // does NOT shadow resource_dir() with the baked source-tree path.
    #[cfg(debug_assertions)]
    {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        if let Some(repo_root) = manifest_dir.parent() {
            candidates.push(repo_root.join("pi-sidecar"));
        }
    }

    // 3. Tauri resource_dir (real packaged-build location).
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("pi-sidecar"));
    }

    // 4. TAURI_RESOURCE_DIR env override (escape hatch, not Tauri-provided).
    if let Ok(resource_dir) = std::env::var("TAURI_RESOURCE_DIR") {
        candidates.push(PathBuf::from(resource_dir).join("pi-sidecar"));
    }

    resolve_cli_candidates(&candidates, &target.to_string_lossy())
}

// ---------------------------------------------------------------------------
// Web-search config (web-search.json)
// ---------------------------------------------------------------------------

/// pi-web-access reads `<PI_CODING_AGENT_DIR>/web-search.json` for its
/// default-provider setting. The `provider` field there sets which search
/// engine is used when the user does not specify one in the prompt.
///
/// Since we ALWAYS set `PI_CODING_AGENT_DIR` at spawn time (pi_sidecar.rs),
/// the file the extension reads is `<resolved agent dir>/web-search.json`.
///
/// Allowlist mirrors the pi-web-access extension's own accepted values.
/// Unified allowlist for the `provider` field in `web-search.json`.
/// Covers both read (pass-through) and write (set) paths.
const WEBSEARCH_CONFIG_ALLOWLIST: &[&str] = &[
    "auto", "exa", "brave", "tavily", "perplexity", "gemini",
    "openai", "parallel",
];

/// Reject unknown provider ids. Pure — safe in tests.
fn validate_websearch_config_provider(provider: &str) -> Result<(), String> {
    if WEBSEARCH_CONFIG_ALLOWLIST.contains(&provider) {
        Ok(())
    } else {
        Err(format!(
            "Unknown websearch config provider: {provider:?}. \
             Allowed: {}.",
            WEBSEARCH_CONFIG_ALLOWLIST.join(", ")
        ))
    }
}

/// Resolve the path to `web-search.json` inside the resolved agent dir.
fn websearch_config_path_for_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("web-search.json")
}

/// Read the `provider` field from `web-search.json` at the given path.
/// - Missing file → returns `"auto"` (the extension's real default).
/// - Missing field → returns `"auto"`.
/// - Non-object JSON → hard error (never destroy unknown content).
/// - Invalid JSON → hard error.
fn websearch_get_config_inner(path: &Path) -> Result<WebsearchConfig, String> {
    if !path.exists() {
        return Ok(WebsearchConfig {
            provider: "auto".into(),
        });
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read web-search.json: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("web-search.json is not valid JSON: {e}"))?;
    if !parsed.is_object() {
        return Err("web-search.json is not valid JSON: expected an object, got non-object".into());
    }
    let provider = parsed
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .trim()
        .to_string();
    Ok(WebsearchConfig { provider })
}

/// Merge-write the `provider` field into `web-search.json` at the given path.
/// Existing fields are preserved. File created when absent.
/// - Corrupt / non-object JSON → hard error (never silently wipe).
/// - Unknown provider on a SET → hard error.
fn websearch_set_config_inner(path: &Path, provider: &str) -> Result<WebsearchConfig, String> {
    validate_websearch_config_provider(provider)?;
    let mut root: serde_json::Value = if path.exists() {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read web-search.json: {e}"))?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("web-search.json is not valid JSON: {e}"))?;
        if !parsed.is_object() {
            return Err("web-search.json is not valid JSON: expected an object, got non-object".into());
        }
        parsed
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    root["provider"] = serde_json::Value::String(provider.into());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create agent dir: {e}"))?;
    }
    let pretty = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Cannot serialize web-search.json: {e}"))?;
    std::fs::write(path, pretty)
        .map_err(|e| format!("Cannot write web-search.json: {e}"))?;
    websearch_get_config_inner(path)
}

/// Thin AppHandle wrapper for the read path.
pub fn websearch_get_config(app: &AppHandle) -> Result<WebsearchConfig, String> {
    let resolved = resolve_pi_agent_dir(app)?;
    let path = websearch_config_path_for_dir(&resolved.path);
    websearch_get_config_inner(&path)
}

/// Thin AppHandle wrapper for the write path.
pub fn websearch_set_config(app: &AppHandle, provider: &str) -> Result<WebsearchConfig, String> {
    let resolved = resolve_pi_agent_dir(app)?;
    let path = websearch_config_path_for_dir(&resolved.path);
    websearch_set_config_inner(&path, provider)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsearchConfig {
    pub provider: String,
}

// ---------------------------------------------------------------------------
// Source validation (SECURITY — defense in depth)
// ---------------------------------------------------------------------------

/// Validate an extension source string. Accepts ONLY:
/// - `npm:<valid-npm-name>` (scoped ok: `npm:@scope/name`)
/// - `git:github.com/<owner>/<repo>`
/// - `https://github.com/<owner>/<repo>`
///
/// Rejects everything else (whitespace, shell metachars, other hosts).
/// Execution is argv-array (no shell) regardless — this is defense in depth.
fn validate_ext_source(s: &str) -> Result<(), String> {
    use regex::Regex;
    use std::sync::OnceLock;
    static NPM_RE: OnceLock<Regex> = OnceLock::new();
    static GIT_GH_RE: OnceLock<Regex> = OnceLock::new();
    static HTTPS_GH_RE: OnceLock<Regex> = OnceLock::new();
    let npm_re = NPM_RE.get_or_init(|| {
        // Scoped: npm:@scope/name  |  Unscoped: npm:name
        Regex::new(r"^npm:(@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*$")
            .unwrap()
    });
    let git_gh_re = GIT_GH_RE.get_or_init(|| {
        Regex::new(r"^git:github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$").unwrap()
    });
    let https_gh_re = HTTPS_GH_RE.get_or_init(|| {
        Regex::new(r"^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$").unwrap()
    });

    if npm_re.is_match(s) || git_gh_re.is_match(s) || https_gh_re.is_match(s) {
        Ok(())
    } else {
        Err(format!(
            "Invalid extension source: {s:?}. \
             Accepted formats: npm:<name>, npm:@scope/name, \
             git:github.com/owner/repo, https://github.com/owner/repo"
        ))
    }
}

/// Check whether a string is a valid npm package name (same regex as
/// `validate_ext_source`, but only the npm variant). Used by `pi_extensions_list`
/// to validate entries read back from `settings.json` (defense in depth —
/// prevents path-traversal via a malicious `npm:../../etc/passwd` entry).
fn is_valid_npm_name(s: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;
    static NPM_NAME_RE: OnceLock<Regex> = OnceLock::new();
    let re = NPM_NAME_RE.get_or_init(|| {
        Regex::new(r"^(@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*$")
            .unwrap()
    });
    re.is_match(s)
}

// ---------------------------------------------------------------------------
// CLI runner — shared core (BLOCKER 1 fix: reader threads, no pipe-buffer deadlock)
// ---------------------------------------------------------------------------

/// Global serialization lock for CLI invocations. Prevents bootstrap and
/// user-triggered install/remove from running npm concurrently on the same
/// agent dir (which would corrupt the npm state).
static CLI_RUN_LOCK: Mutex<()> = Mutex::new(());

/// Cap for reading child output bytes (prevents OOM from a rogue child).
/// 4 MB — generous for npm output, bounded enough to not matter.
const CLI_READ_CAP_BYTES: u64 = 4 * 1024 * 1024;

/// Core CLI runner. Spawns `node <cli> <verb> <source>` with
/// `PI_CODING_AGENT_DIR` set, drains stdout/stderr on dedicated reader threads
/// (avoids pipe-buffer deadlock — BLOCKER 1 fix), and enforces a hard timeout
/// via a watchdog. Validation is enforced inside (trust boundary).
fn run_pi_cli_at(
    app: &AppHandle,
    agent_dir: &Path,
    verb: &str,
    source: &str,
) -> Result<String, String> {
    // SECURITY: validate inside the runner (trust boundary — issue 6).
    validate_ext_source(source)?;

    let cli_path = resolve_bundled_pi_cli(app)?;

    let mut cmd = std::process::Command::new("node");
    cmd.arg(&cli_path)
        .arg(verb)
        .arg(source)
        .env("PI_CODING_AGENT_DIR", agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn pi CLI: {e}"))?;

    // Take piped handles BEFORE sharing the child, drain on reader threads.
    // EOF arrives when the child exits or is killed — reading concurrently
    // avoids a pipe-buffer deadlock on large npm output.
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(h) = stdout_handle {
            let _ = h.take(CLI_READ_CAP_BYTES).read_to_end(&mut buf);
        }
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(h) = stderr_handle {
            let _ = h.take(CLI_READ_CAP_BYTES).read_to_end(&mut buf);
        }
        buf
    });

    // Poll loop: enforces the deadline itself (no separate watchdog thread).
    // The child is now a plain mut — readers already took stdout/stderr.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(CLI_TIMEOUT_SECS);
    let mut timed_out = false;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if !timed_out && std::time::Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    // Keep looping — next try_wait reaps the exit status.
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                return Err(format!("pi {verb} {source} status check failed: {e}"));
            }
        }
    };

    // Join reader threads — they finished when the child exited (EOF).
    let stdout_bytes = stdout_reader
        .join()
        .unwrap_or_default();
    let stderr_bytes = stderr_reader
        .join()
        .unwrap_or_default();

    // Read as bytes + from_utf8_lossy (issue 9): preserves non-UTF8 npm diagnostics.
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    let combined = format!("{stdout}{stderr}");

    if exit_status.success() {
        // Cap the success output (issue 10).
        Ok(tail_chars(&combined, CLI_OUTPUT_TAIL_CHARS))
    } else {
        let tail = tail_chars(&combined, CLI_OUTPUT_TAIL_CHARS);
        if timed_out {
            Err(format!(
                "pi {verb} {source} timed out after {CLI_TIMEOUT_SECS}s:\n{tail}"
            ))
        } else {
            Err(format!(
                "pi {verb} {source} failed (exit {}):\n{tail}",
                exit_status.code().unwrap_or(-1)
            ))
        }
    }
}

/// Return the last `cap` chars of `s`.
fn tail_chars(s: &str, cap: usize) -> String {
    let len = s.chars().count();
    if len <= cap {
        return s.to_string();
    }
    s.chars().skip(len - cap).collect()
}

// ---------------------------------------------------------------------------
// Types for list / search
// ---------------------------------------------------------------------------

/// A single extension entry returned by `pi_extensions_list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiExtensionInfo {
    pub source: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub installed_ok: bool,
}

/// A marketplace search result entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketEntry {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub date: String,
}

/// settings.json shape: `{"packages": ["npm:pi-lens", ...]}`.
#[derive(Debug, Default, Deserialize)]
struct SettingsJson {
    #[serde(default)]
    packages: Vec<String>,
}

/// Minimal package.json fields we care about.
#[derive(Debug, Default, Deserialize)]
struct PackageJson {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    #[serde(default)]
    author: Option<serde_json::Value>,
}

/// Extract author name from package.json `author` field (string or {name}).
fn extract_author(author: &Option<serde_json::Value>) -> String {
    match author {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Object(m)) => m
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        _ => "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Bootstrap state (issue 4: single Mutex, torn-status fix)
// ---------------------------------------------------------------------------

/// Bootstrap lifecycle, read by `pi_extensions_status`.
static BOOTSTRAP_STATE: Mutex<BootstrapState> = Mutex::new(BootstrapState {
    status: BootstrapStatus::Idle,
    error: None,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapStatus {
    Idle,
    Running,
    Done,
    Failed,
}

#[derive(Debug)]
struct BootstrapState {
    status: BootstrapStatus,
    error: Option<String>,
}



fn set_bootstrap_status(status: BootstrapStatus, error: Option<String>) {
    if let Ok(mut s) = BOOTSTRAP_STATE.lock() {
        s.status = status;
        s.error = error;
    }
}

fn bootstrap_status_str() -> &'static str {
    let snap = BOOTSTRAP_STATE
        .lock()
        .map(|s| s.status)
        .unwrap_or(BootstrapStatus::Idle);
    match snap {
        BootstrapStatus::Idle => "idle",
        BootstrapStatus::Running => "running",
        BootstrapStatus::Done => "done",
        BootstrapStatus::Failed => "failed",
    }
}

fn bootstrap_error() -> Option<String> {
    BOOTSTRAP_STATE
        .lock()
        .ok()
        .and_then(|s| s.error.clone())
}

/// Check if bootstrap is currently running (used by install/remove commands
/// to reject concurrent operations — issue 3b).
fn bootstrap_is_running() -> bool {
    BOOTSTRAP_STATE
        .lock()
        .map(|s| s.status == BootstrapStatus::Running)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Bootstrap (first-launch, app-managed mode only)
// ---------------------------------------------------------------------------

/// Called once at startup from `lib.rs`'s setup. Runs on a detached thread.
pub fn bootstrap_extensions_if_needed(app: &AppHandle) {
    let resolved = match resolve_pi_agent_dir(app) {
        Ok(r) => r,
        Err(_) => return, // Can't resolve → silently skip (pi falls back to ~/.pi/agent).
    };

    // HARD RULE: NEVER write into the user's global ~/.pi/agent or env-override dir.
    let skip_msg = match resolved.mode {
        AgentDirMode::AppManaged => None,
        AgentDirMode::Global => Some("skipped: global dir".to_string()),
        AgentDirMode::EnvOverride => Some("skipped: env-override dir".to_string()),
    };
    if let Some(msg) = skip_msg {
        set_bootstrap_status(BootstrapStatus::Done, Some(msg));
        return;
    }

    // Idempotent: if settings.json already exists, nothing to do.
    if resolved.path.join("settings.json").exists() {
        set_bootstrap_status(BootstrapStatus::Done, None);
        return;
    }

    // Create the dir + install curated extensions.
    set_bootstrap_status(BootstrapStatus::Running, None);
    let _ = std::fs::create_dir_all(&resolved.path);

    for source in CURATED_EXTENSIONS {
        // validate_ext_source is also called inside run_pi_cli_at, but
        // validate here too for a clear error before we lock CLI_RUN_LOCK.
        if let Err(e) = validate_ext_source(source) {
            set_bootstrap_status(
                BootstrapStatus::Failed,
                Some(format!("bad curated source {source}: {e}")),
            );
            return;
        }
        // Acquire the global CLI lock (issue 3a).
        let _guard = CLI_RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = run_pi_cli_at(app, &resolved.path, "install", source) {
            drop(_guard);
            set_bootstrap_status(
                BootstrapStatus::Failed,
                Some(format!("install {source} failed: {e}")),
            );
            return;
        }
        // _guard dropped here, releasing CLI_RUN_LOCK for next iteration.
    }
    set_bootstrap_status(BootstrapStatus::Done, None);
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Status of the pi extensions subsystem.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiExtensionsStatus {
    pub agent_dir: String,
    pub mode: AgentDirMode,
    pub bootstrap: &'static str,
    pub bootstrap_error: Option<String>,
}

/// Read-only status: agent dir location + bootstrap lifecycle.
#[tauri::command]
pub fn pi_extensions_status(app: AppHandle) -> Result<PiExtensionsStatus, String> {
    let resolved = resolve_pi_agent_dir(&app)?;
    Ok(PiExtensionsStatus {
        agent_dir: resolved.path.display().to_string(),
        mode: resolved.mode,
        bootstrap: bootstrap_status_str(),
        bootstrap_error: bootstrap_error(),
    })
}

/// List installed extensions (reads `settings.json` + per-package `package.json`).
#[tauri::command]
pub fn pi_extensions_list(app: AppHandle) -> Result<Vec<PiExtensionInfo>, String> {
    let resolved = resolve_pi_agent_dir(&app)?;
    let settings_path = resolved.path.join("settings.json");

    // Missing settings.json → empty vec (NOT an error).
    let settings: SettingsJson = match std::fs::read_to_string(&settings_path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => return Ok(Vec::new()),
    };

    let mut result = Vec::with_capacity(settings.packages.len());
    for source in &settings.packages {
        if let Some(name) = source.strip_prefix("npm:") {
            // SECURITY (issue 7): validate the npm name read from settings.json
            // before joining into a filesystem path — prevents path-traversal via
            // a malicious entry like `npm:../../etc/passwd`.
            if !is_valid_npm_name(name) {
                result.push(PiExtensionInfo {
                    source: source.clone(),
                    name: source.clone(),
                    version: String::new(),
                    description: String::new(),
                    author: String::new(),
                    installed_ok: false,
                });
                continue;
            }
            let pkg_json_path = resolved
                .path
                .join("npm")
                .join("node_modules")
                .join(name)
                .join("package.json");
            match std::fs::read_to_string(&pkg_json_path) {
                Ok(data) => {
                    let pkg: PackageJson = serde_json::from_str(&data).unwrap_or_default();
                    let installed_ok = pkg.name.is_some();
                    result.push(PiExtensionInfo {
                        source: source.clone(),
                        name: pkg.name.unwrap_or_else(|| name.to_string()),
                        version: pkg.version.unwrap_or_default(),
                        description: pkg.description.unwrap_or_default(),
                        author: extract_author(&pkg.author),
                        installed_ok,
                    });
                }
                Err(_) => {
                    // Missing/unparsable package.json → installedOk false.
                    result.push(PiExtensionInfo {
                        source: source.clone(),
                        name: name.to_string(),
                        version: String::new(),
                        description: String::new(),
                        author: String::new(),
                        installed_ok: false,
                    });
                }
            }
        } else {
            // Non-npm source: name = source, version empty.
            result.push(PiExtensionInfo {
                source: source.clone(),
                name: source.clone(),
                version: String::new(),
                description: String::new(),
                author: String::new(),
                installed_ok: true,
            });
        }
    }
    Ok(result)
}

/// Install an extension by source.
#[tauri::command]
pub async fn pi_extension_install(app: AppHandle, source: String) -> Result<String, String> {
    // Reject if bootstrap is still running (issue 3b).
    if bootstrap_is_running() {
        return Err(
            "Extension bootstrap is still running — retry in a moment.".to_string(),
        );
    }
    let resolved = resolve_pi_agent_dir(&app)?;
    // validate_ext_source is called inside run_pi_cli_at (trust boundary).
    // The MutexGuard must NOT cross the .await, so the entire blocking
    // operation (lock acquisition + CLI run) happens inside spawn_blocking.
    let dir = resolved.path;
    let app_clone = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = CLI_RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        run_pi_cli_at(&app_clone, &dir, "install", &source)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Remove an extension by source.
#[tauri::command]
pub async fn pi_extension_remove(app: AppHandle, source: String) -> Result<String, String> {
    // Reject if bootstrap is still running (issue 3b).
    if bootstrap_is_running() {
        return Err(
            "Extension bootstrap is still running — retry in a moment.".to_string(),
        );
    }
    let resolved = resolve_pi_agent_dir(&app)?;
    let dir = resolved.path;
    let app_clone = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = CLI_RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        run_pi_cli_at(&app_clone, &dir, "remove", &source)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Search the npm marketplace for pi extensions.
#[tauri::command]
pub async fn pi_marketplace_search(
    _app: AppHandle,
    query: Option<String>,
) -> Result<Vec<MarketEntry>, String> {
    let search_term = match query {
        Some(q) if !q.trim().is_empty() => {
            // Percent-encode the query; pin the ecosystem keyword.
            format!(
                "{}+keywords:{}",
                urlencoding::encode(&q),
                PI_PACKAGE_KEYWORD
            )
        }
        _ => format!("keywords:{}", PI_PACKAGE_KEYWORD),
    };

    let url = format!("{NPM_SEARCH_URL}?text={search_term}&size=40");

    // Blocking reqwest call inside spawn_blocking (pattern from oracle_coordinator.rs).
    let entries = tauri::async_runtime::spawn_blocking(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(MARKETPLACE_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("HTTP client error: {e}"))?;

        let resp = client
            .get(&url)
            .send()
            .map_err(|e| format!("npm registry search failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("npm registry returned HTTP {}", resp.status()));
        }

        // Cap the body read (issue 11): mirror the read-cap style in api_fuzz.rs.
        let mut body_bytes = Vec::new();
        let mut reader = resp.take(MARKETPLACE_MAX_BODY_BYTES);
        reader
            .read_to_end(&mut body_bytes)
            .map_err(|e| format!("npm registry read failed: {e}"))?;
        if body_bytes.len() as u64 >= MARKETPLACE_MAX_BODY_BYTES {
            return Err("npm registry response too large (>2 MB)".to_string());
        }

        let body: serde_json::Value =
            serde_json::from_slice(&body_bytes).map_err(|e| format!("Failed to parse npm search response: {e}"))?;

        let objects = body
            .get("objects")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut entries = Vec::with_capacity(objects.len());
        for obj in objects {
            if let Some(pkg) = obj.get("package") {
                let author = pkg
                    .get("author")
                    .and_then(|a| a.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                entries.push(MarketEntry {
                    name: pkg
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string(),
                    version: pkg
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    description: pkg
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    author,
                    date: pkg
                        .get("date")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
        Ok(entries)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?;

    entries
}

// ---------------------------------------------------------------------------
// pi_agents_list — read agent definitions from <agent dir>/agents/*.md
// ---------------------------------------------------------------------------

/// One agent definition extracted from a `.md` file's YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub model: String,
    pub file: String,
}

/// Parse YAML frontmatter from a markdown file. Returns (name, description, model)
/// extracted from simple `key: value` lines between `---` delimiters.
/// Missing fields default to empty string. No serde_yaml dependency.
fn parse_frontmatter(content: &str) -> (String, String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let mut model = String::new();

    // Find the frontmatter block between first and second `---`.
    let lines: Vec<&str> = content.lines().collect();
    let mut in_frontmatter = false;
    for line in &lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            if in_frontmatter {
                break; // End of frontmatter.
            }
            in_frontmatter = true;
            continue;
        }
        if !in_frontmatter {
            continue;
        }
        // Parse simple `key: value` lines.
        if let Some(val) = trimmed.strip_prefix("name:").map(str::trim) {
            name = unquote(val).to_string();
        } else if let Some(val) = trimmed.strip_prefix("description:").map(str::trim) {
            description = unquote(val).to_string();
        } else if let Some(val) = trimmed.strip_prefix("model:").map(str::trim) {
            model = unquote(val).to_string();
        }
    }

    (name, description, model)
}

/// Strip surrounding quotes (single or double) from a value.
fn unquote(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Inner impl for reading agent definitions from a directory (testable without AppHandle).
fn agents_list_inner(agents_dir: &Path) -> Result<Vec<AgentDefinition>, String> {
    if !agents_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut agents = Vec::new();
    let entries = std::fs::read_dir(agents_dir)
        .map_err(|e| format!("Cannot read agents dir: {e}"))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue, // Skip unreadable files.
        };
        let (name, description, model) = parse_frontmatter(&content);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        agents.push(AgentDefinition {
            name: if name.is_empty() { file_name.clone() } else { name },
            description,
            model,
            file: file_name,
        });
    }

    // Sort by name for stable output.
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

/// List agent definitions from the resolved agent dir's `agents/` subdirectory.
#[tauri::command]
pub fn pi_agents_list(app: AppHandle) -> Result<Vec<AgentDefinition>, String> {
    let resolved = resolve_pi_agent_dir(&app)?;
    let agents_dir = resolved.path.join("agents");
    agents_list_inner(&agents_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pick_agent_dir (4 cases) ----

    #[test]
    fn pick_agent_dir_env_override_wins() {
        let env = Some(PathBuf::from("/tmp/custom-pi"));
        let (path, mode) = pick_agent_dir(
            env,
            true,
            "/home/user/.pi/agent".into(),
            "/app/pi-agent".into(),
        );
        assert_eq!(path, PathBuf::from("/tmp/custom-pi"));
        assert_eq!(mode, AgentDirMode::EnvOverride);
    }

    #[test]
    fn pick_agent_dir_global_exists() {
        let (path, mode) = pick_agent_dir(
            None,
            true,
            "/home/user/.pi/agent".into(),
            "/app/pi-agent".into(),
        );
        assert_eq!(path, PathBuf::from("/home/user/.pi/agent"));
        assert_eq!(mode, AgentDirMode::Global);
    }

    #[test]
    fn pick_agent_dir_app_managed_fallback() {
        let (path, mode) = pick_agent_dir(
            None,
            false,
            "/home/user/.pi/agent".into(),
            "/app/pi-agent".into(),
        );
        assert_eq!(path, PathBuf::from("/app/pi-agent"));
        assert_eq!(mode, AgentDirMode::AppManaged);
    }

    #[test]
    fn pick_agent_dir_env_overrides_even_when_global_exists() {
        let env = Some(PathBuf::from("/tmp/x"));
        let (path, mode) = pick_agent_dir(
            env,
            true,
            "/home/user/.pi/agent".into(),
            "/app/pi-agent".into(),
        );
        assert_eq!(path, PathBuf::from("/tmp/x"));
        assert_eq!(mode, AgentDirMode::EnvOverride);
    }

    // ---- expand_tilde (issue 2) ----

    #[test]
    fn expand_tilde_bare_tilde() {
        let result = expand_tilde(Path::new("~"));
        if let Some(home) = home_dir() {
            assert_eq!(result, home);
        } else {
            assert_eq!(result, PathBuf::from("~"));
        }
    }

    #[test]
    fn expand_tilde_slash_x() {
        let result = expand_tilde(Path::new("~/x"));
        if let Some(home) = home_dir() {
            assert_eq!(result, home.join("x"));
        } else {
            assert_eq!(result, PathBuf::new());
        }
    }

    #[test]
    fn expand_tilde_absolute_passthrough() {
        let result = expand_tilde(Path::new("/tmp/x"));
        assert_eq!(result, PathBuf::from("/tmp/x"));
    }

    #[test]
    fn expand_tilde_relative_passthrough() {
        let result = expand_tilde(Path::new("relative/path"));
        assert_eq!(result, PathBuf::from("relative/path"));
    }

    // ---- validate_ext_source ----

    #[test]
    fn validate_ext_source_accepts_npm_pi_lens() {
        assert!(validate_ext_source("npm:pi-lens").is_ok());
    }

    #[test]
    fn validate_ext_source_accepts_npm_scoped() {
        assert!(validate_ext_source("npm:@scope/name").is_ok());
        assert!(validate_ext_source("npm:@tintinweb/pi-subagents").is_ok());
        assert!(validate_ext_source("npm:@pi-unipi/compactor").is_ok());
    }

    #[test]
    fn validate_ext_source_accepts_git_github() {
        assert!(validate_ext_source("git:github.com/user/repo").is_ok());
    }

    #[test]
    fn validate_ext_source_accepts_https_github() {
        assert!(validate_ext_source("https://github.com/user/repo").is_ok());
    }

    #[test]
    fn validate_ext_source_rejects_shell_metachars() {
        assert!(validate_ext_source("npm:foo; rm -rf /").is_err());
    }

    #[test]
    fn validate_ext_source_rejects_space() {
        assert!(validate_ext_source("npm:foo bar").is_err());
    }

    #[test]
    fn validate_ext_source_rejects_empty() {
        assert!(validate_ext_source("").is_err());
    }

    #[test]
    fn validate_ext_source_rejects_other_host() {
        assert!(validate_ext_source("https://evil.com/x/y").is_err());
    }

    #[test]
    fn validate_ext_source_rejects_traversal() {
        assert!(validate_ext_source("npm:../../etc").is_err());
    }

    #[test]
    fn validate_ext_source_rejects_uppercase_npm() {
        assert!(validate_ext_source("npm:Pi-Lens").is_err());
    }

    #[test]
    fn validate_ext_source_rejects_random_prefix() {
        assert!(validate_ext_source("foo:bar").is_err());
    }

    // ---- is_valid_npm_name (issue 7) ----

    #[test]
    fn is_valid_npm_name_accepts_pi_lens() {
        assert!(is_valid_npm_name("pi-lens"));
    }

    #[test]
    fn is_valid_npm_name_accepts_scoped() {
        assert!(is_valid_npm_name("@scope/name"));
        assert!(is_valid_npm_name("@tintinweb/pi-subagents"));
    }

    #[test]
    fn is_valid_npm_name_rejects_traversal() {
        assert!(!is_valid_npm_name("../../etc/passwd"));
    }

    #[test]
    fn is_valid_npm_name_rejects_uppercase() {
        assert!(!is_valid_npm_name("Pi-Lens"));
    }

    #[test]
    fn is_valid_npm_name_rejects_empty() {
        assert!(!is_valid_npm_name(""));
    }

    #[test]
    fn is_valid_npm_name_rejects_shell_metachars() {
        assert!(!is_valid_npm_name("foo; rm -rf /"));
    }

    // ---- package.json → PiExtensionInfo mapping ----

    #[test]
    fn extract_author_from_string() {
        assert_eq!(
            extract_author(&Some(serde_json::Value::String("Alice".into()))),
            "Alice"
        );
    }

    #[test]
    fn extract_author_from_object() {
        let val = serde_json::json!({"name": "Bob", "url": "https://bob.dev"});
        assert_eq!(extract_author(&Some(val)), "Bob");
    }

    #[test]
    fn extract_author_missing() {
        assert_eq!(extract_author(&None), "unknown");
    }

    #[test]
    fn parse_package_json_fields() {
        let json =
            r#"{"name":"pi-lens","version":"0.3.0","description":"Lens for pi","author":"Tin"}"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(pkg.name.as_deref(), Some("pi-lens"));
        assert_eq!(pkg.version.as_deref(), Some("0.3.0"));
        assert_eq!(pkg.description.as_deref(), Some("Lens for pi"));
    }

    #[test]
    fn parse_package_json_missing_fields() {
        let json = r#"{}"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert!(pkg.name.is_none());
        assert!(pkg.version.is_none());
        assert!(pkg.description.is_none());
        assert!(pkg.author.is_none());
    }

    #[test]
    fn parse_package_json_author_object() {
        let json = r#"{"name":"test","author":{"name":"Carol","email":"c@example.com"}}"#;
        let pkg: PackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(extract_author(&pkg.author), "Carol");
    }

    // ---- npm search JSON → Vec<MarketEntry> mapping (canned fixture) ----

    #[test]
    fn parse_npm_search_response() {
        let fixture = serde_json::json!({
            "total": 2,
            "objects": [
                {
                    "package": {
                        "name": "pi-lens",
                        "version": "0.3.0",
                        "description": "A lens for pi",
                        "keywords": ["pi-package"],
                        "author": {"name": "Tin"},
                        "date": "2025-01-15T10:00:00.000Z"
                    }
                },
                {
                    "package": {
                        "name": "pi-web-access",
                        "version": "1.0.0",
                        "description": "Web access for pi",
                        "author": {"name": "Alice"},
                        "date": "2025-02-20T12:00:00.000Z"
                    }
                }
            ]
        });

        let objects = fixture["objects"].as_array().unwrap();
        let mut entries = Vec::new();
        for obj in objects {
            if let Some(pkg) = obj.get("package") {
                let author = pkg
                    .get("author")
                    .and_then(|a| a.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                entries.push(MarketEntry {
                    name: pkg
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string(),
                    version: pkg
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    description: pkg
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    author,
                    date: pkg
                        .get("date")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "pi-lens");
        assert_eq!(entries[0].author, "Tin");
        assert_eq!(entries[1].name, "pi-web-access");
        assert_eq!(entries[1].version, "1.0.0");
    }

    // ---- Bootstrap decision logic (pure part) ----

    #[test]
    fn bootstrap_skips_global_mode() {
        let mode = AgentDirMode::Global;
        assert_ne!(mode, AgentDirMode::AppManaged);
    }

    #[test]
    fn bootstrap_skips_env_override_mode() {
        let mode = AgentDirMode::EnvOverride;
        assert_ne!(mode, AgentDirMode::AppManaged);
    }

    #[test]
    fn bootstrap_skips_when_settings_exists() {
        let dir = std::env::temp_dir().join(format!(
            "pi-ext-bootstrap-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let settings = dir.join("settings.json");
        std::fs::write(&settings, r#"{"packages": []}"#).unwrap();
        assert!(settings.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_json_parse_empty_packages() {
        let json = r#"{"packages": []}"#;
        let s: SettingsJson = serde_json::from_str(json).unwrap();
        assert!(s.packages.is_empty());
    }

    #[test]
    fn settings_json_parse_missing() {
        let json = r#"{}"#;
        let s: SettingsJson = serde_json::from_str(json).unwrap();
        assert!(s.packages.is_empty());
    }

    // ---- tail_chars (issue 10) ----

    #[test]
    fn tail_chars_short_string_unchanged() {
        assert_eq!(tail_chars("hello", 10), "hello");
    }

    #[test]
    fn tail_chars_exact_cap() {
        assert_eq!(tail_chars("abcde", 5), "abcde");
    }

    #[test]
    fn tail_chars_truncates() {
        assert_eq!(tail_chars("abcdefghij", 5), "fghij");
    }

    #[test]
    fn tail_chars_unicode_safe() {
        // Each emoji is 4 bytes but 1 char.
        let s = "ññññññññññ";
        let result = tail_chars(s, 5);
        assert_eq!(result.chars().count(), 5);
        assert!(result.starts_with('ñ'));
    }

    // ---- Settings.json path-traversal defense (issue 7) ----

    #[test]
    fn npm_name_rejects_path_traversal() {
        assert!(!is_valid_npm_name("../../etc/passwd"));
        assert!(!is_valid_npm_name("../foo"));
        assert!(!is_valid_npm_name("a/b/../c"));
    }

    #[test]
    fn npm_name_accepts_valid_scoped() {
        assert!(is_valid_npm_name("@tintinweb/pi-subagents"));
        assert!(is_valid_npm_name("@pi-unipi/compactor"));
    }

    // ---- Websearch config (fix 4: real inner-fn tests) -----------------------

    use super::websearch_get_config_inner;
    use super::websearch_set_config_inner;
    use super::validate_websearch_config_provider;

    /// RAII temp dir for websearch config tests.
    fn websearch_test_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pi-websearch-{suffix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn websearch_config_provider_accepts_all_8() {
        for id in ["auto", "exa", "brave", "tavily", "perplexity", "gemini", "openai", "parallel"] {
            assert!(validate_websearch_config_provider(id).is_ok(), "must accept {id}");
        }
    }

    #[test]
    fn websearch_config_provider_rejects_unknown() {
        assert!(validate_websearch_config_provider("bing").is_err());
        assert!(validate_websearch_config_provider("gemini_search").is_err());
        assert!(validate_websearch_config_provider("").is_err());
        assert!(validate_websearch_config_provider("EXA").is_err());
    }

    #[test]
    fn websearch_get_config_missing_file_returns_auto() {
        let dir = websearch_test_dir("get-missing");
        let path = dir.join("web-search.json");
        let cfg = websearch_get_config_inner(&path).unwrap();
        assert_eq!(cfg.provider, "auto");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_get_config_missing_field_returns_auto() {
        let dir = websearch_test_dir("get-nofield");
        let path = dir.join("web-search.json");
        std::fs::write(&path, r#"{"other": 42}"#).unwrap();
        let cfg = websearch_get_config_inner(&path).unwrap();
        assert_eq!(cfg.provider, "auto");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_get_config_passes_through_openai_parallel() {
        let dir = websearch_test_dir("get-pass");
        let path = dir.join("web-search.json");
        std::fs::write(&path, r#"{"provider": "openai"}"#).unwrap();
        let cfg = websearch_get_config_inner(&path).unwrap();
        assert_eq!(cfg.provider, "openai");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_get_config_corrupt_file_is_hard_error() {
        let dir = websearch_test_dir("get-corrupt");
        let path = dir.join("web-search.json");
        std::fs::write(&path, "not json at all").unwrap();
        let err = websearch_get_config_inner(&path).unwrap_err();
        assert!(err.contains("not valid JSON"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_get_config_non_object_is_hard_error() {
        let dir = websearch_test_dir("get-nonobj");
        let path = dir.join("web-search.json");
        std::fs::write(&path, r#"[1, 2, 3]"#).unwrap();
        let err = websearch_get_config_inner(&path).unwrap_err();
        assert!(err.contains("expected an object"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_set_config_preserves_existing_fields() {
        let dir = websearch_test_dir("set-preserve");
        let path = dir.join("web-search.json");
        std::fs::write(&path, r#"{"provider": "exa", "extra": "kept"}"#).unwrap();
        let cfg = websearch_set_config_inner(&path, "brave").unwrap();
        assert_eq!(cfg.provider, "brave");
        let read_back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back["provider"], "brave");
        assert_eq!(read_back["extra"], "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_set_config_creates_file_when_missing() {
        let dir = websearch_test_dir("set-missing");
        let path = dir.join("web-search.json");
        assert!(!path.exists());
        let cfg = websearch_set_config_inner(&path, "tavily").unwrap();
        assert_eq!(cfg.provider, "tavily");
        let read_back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back["provider"], "tavily");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_set_config_corrupt_file_is_hard_error() {
        let dir = websearch_test_dir("set-corrupt");
        let path = dir.join("web-search.json");
        std::fs::write(&path, "not json").unwrap();
        let err = websearch_set_config_inner(&path, "brave").unwrap_err();
        assert!(err.contains("not valid JSON"));
        // Original corrupt file must be preserved (not wiped).
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_set_config_non_object_file_is_hard_error() {
        let dir = websearch_test_dir("set-nonobj");
        let path = dir.join("web-search.json");
        std::fs::write(&path, r#"[1, 2, 3]"#).unwrap();
        let err = websearch_set_config_inner(&path, "brave").unwrap_err();
        assert!(err.contains("expected an object"));
        // Original file must be preserved (not wiped).
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"[1, 2, 3]"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn websearch_set_config_accepts_openai_parallel() {
        let dir = websearch_test_dir("set-accept");
        let path = dir.join("web-search.json");
        let cfg = websearch_set_config_inner(&path, "openai").unwrap();
        assert_eq!(cfg.provider, "openai");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- resolve_cli_candidates (pure candidate-walk) -------------------------

    #[test]
    fn resolve_cli_candidates_picks_existing() {
        let tmp = std::env::temp_dir().join(format!(
            "pi-cli-cand-1-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let candidate = tmp.join("pi-sidecar").join("node_modules").join(".bin");
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("pi"), "").unwrap();

        let target = Path::new("node_modules").join(".bin").join("pi");
        let result = resolve_cli_candidates(&[tmp.join("pi-sidecar")], &target.to_string_lossy());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), candidate.join("pi"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_cli_candidates_skips_missing() {
        let tmp = std::env::temp_dir().join(format!(
            "pi-cli-cand-2-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Candidate 1: pi-sidecar exists but .bin/pi does not.
        let missing = tmp.join("missing").join("pi-sidecar");
        std::fs::create_dir_all(&missing).unwrap();
        // Candidate 2: has .bin/pi.
        let good = tmp.join("good").join("pi-sidecar");
        let bin = good.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("pi"), "").unwrap();

        let target = Path::new("node_modules").join(".bin").join("pi");
        let result = resolve_cli_candidates(
            &[missing, good],
            &target.to_string_lossy(),
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bin.join("pi"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_cli_candidates_error_lists_all_tried_paths() {
        let tmp = std::env::temp_dir().join(format!(
            "pi-cli-cand-3-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let a = tmp.join("a").join("pi-sidecar");
        let b = tmp.join("b").join("pi-sidecar");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        let target = Path::new("node_modules").join(".bin").join("pi");
        let expected_a = a.join(&target).display().to_string();
        let expected_b = b.join(&target).display().to_string();
        let result = resolve_cli_candidates(&[a, b], &target.to_string_lossy());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains(&expected_a), "must list first tried path: {err}");
        assert!(err.contains(&expected_b), "must list second tried path: {err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ---- parse_frontmatter (line-based YAML frontmatter) ----

    #[test]
    fn parse_frontmatter_extracts_all_fields() {
        let md = "---\nname: my-agent\ndescription: A test agent\nmodel: gpt-4\n---\nBody content here.";
        let (name, desc, model) = parse_frontmatter(md);
        assert_eq!(name, "my-agent");
        assert_eq!(desc, "A test agent");
        assert_eq!(model, "gpt-4");
    }

    #[test]
    fn parse_frontmatter_handles_quoted_values() {
        let md = "---\nname: \"quoted-name\"\ndescription: 'single quoted'\nmodel: unquoted\n---";
        let (name, desc, model) = parse_frontmatter(md);
        assert_eq!(name, "quoted-name");
        assert_eq!(desc, "single quoted");
        assert_eq!(model, "unquoted");
    }

    #[test]
    fn parse_frontmatter_missing_fields_defaults_to_empty() {
        let md = "---\nname: only-name\n---";
        let (name, desc, model) = parse_frontmatter(md);
        assert_eq!(name, "only-name");
        assert_eq!(desc, "");
        assert_eq!(model, "");
    }

    #[test]
    fn parse_frontmatter_no_frontmatter_returns_empty() {
        let md = "Just some markdown content without frontmatter.";
        let (name, desc, model) = parse_frontmatter(md);
        assert_eq!(name, "");
        assert_eq!(desc, "");
        assert_eq!(model, "");
    }

    #[test]
    fn parse_frontmatter_empty_content() {
        let (name, desc, model) = parse_frontmatter("");
        assert_eq!(name, "");
        assert_eq!(desc, "");
        assert_eq!(model, "");
    }

    #[test]
    fn parse_frontmatter_ignores_unknown_keys() {
        let md = "---\nname: agent\ntools: [read, bash]\nthinking: high\n---";
        let (name, desc, model) = parse_frontmatter(md);
        assert_eq!(name, "agent");
        assert_eq!(desc, "");
        assert_eq!(model, "");
    }

    // ---- agents_list_inner ----

    fn agents_test_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pi-agents-{suffix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn agents_list_inner_reads_agent_files() {
        let dir = agents_test_dir("read");
        let agents_dir = dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("alpha.md"),
            "---\nname: alpha\ndescription: First agent\nmodel: gpt-4\n---\nBody.",
        ).unwrap();
        std::fs::write(
            agents_dir.join("beta.md"),
            "---\nname: beta\ndescription: Second agent\nmodel: claude-sonnet\n---\nBody.",
        ).unwrap();
        let agents = agents_list_inner(&agents_dir).unwrap();
        assert_eq!(agents.len(), 2);
        // Sorted by name.
        assert_eq!(agents[0].name, "alpha");
        assert_eq!(agents[0].model, "gpt-4");
        assert_eq!(agents[0].file, "alpha.md");
        assert_eq!(agents[1].name, "beta");
        assert_eq!(agents[1].model, "claude-sonnet");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_list_inner_missing_dir_returns_empty() {
        let dir = agents_test_dir("missing");
        let agents = agents_list_inner(&dir).unwrap();
        assert!(agents.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_list_inner_skips_non_md_files() {
        let dir = agents_test_dir("skip-nonmd");
        let agents_dir = dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("readme.txt"), "not an agent").unwrap();
        std::fs::write(agents_dir.join("agent.json"), "{}" ).unwrap();
        let agents = agents_list_inner(&agents_dir).unwrap();
        assert!(agents.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_list_inner_uses_filename_when_name_missing() {
        let dir = agents_test_dir("fallback-name");
        let agents_dir = dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("fallback.md"),
            "---\ndescription: No name field\n---",
        ).unwrap();
        let agents = agents_list_inner(&agents_dir).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "fallback.md");
        assert_eq!(agents[0].description, "No name field");
        assert_eq!(agents[0].file, "fallback.md");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
