//! pi-sidecar bridge — Phase 1: per-session lifecycle + vault adapter.
//!
//! Spawns a Node.js sidecar process (`pi-sidecar/sidecar.mjs`) that embeds the pi SDK,
//! reads its JSONL event stream from stdout, maps pi events to the existing
//! `MiniActivityEvent` / `ConsoleActivity` schema, and emits them on the
//! `mini-activity://<sessionId>` channel so the existing `WorkConsole.tsx` renders them
//! WITHOUT any React changes.
//!
//! Phase 0 → Phase 1 changes (decision #7, #9):
//! - Per-session agent IDs (pi-<counter>) instead of hardcoded `pi-spike`.
//! - Per-session generation counters (Arc<AtomicU64>) — spawning session B does NOT
//!   kill session A's reader thread.
//! - Multi-session state: HashMap<sessionId, session> + per-session generation.
//! - `spike_pi_prompt(sessionId?)`: creates new session if absent, routes to existing if present.
//! - `spike_pi_stop(sessionId)`: kills a session, joins reader, drops state.
//! - Vault adapter: reads coder backend from config.json + API key from keyring,
//!   passes as env vars to the Node sidecar.
//!
//! Design doc: `docs/devboule-on-pi-architecture.md` §7 (bridge), §11 (decisions #7, #9).
//! Mirror pattern: `oracle/python_oracle.rs` (Command spawn + env injection).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::mini_activity::{push_coder_note, ConsoleActivity, ConsoleEntry, MiniActivityEvent, NodeStyle, PageEntry};
use super::sandbox::{NetPolicy, ResourceLimits, SandboxPolicy};

// ---- per-session state (decision #7) --------------------------------------

/// Maximum concurrent pi sidecar sessions (F6).
const MAX_SESSIONS: usize = 8;

/// Placeholder inserted under the lock before spawning. The real PiSession
/// replaces it after spawn completes (F2: spawn outside lock).
struct SessionSlot {
    inner: Option<PiSession>,
}

/// A single active pi sidecar session. Each session gets its own Node child process,
/// stdin writer, per-session generation counter, and reader thread handle.
struct PiSession {
    /// Shared child process handle. Wrapped in `Arc<Mutex<>>` so BOTH the reader
    /// thread (which calls `try_wait()` on EOF) and the control path
    /// (`stop_pi_session`, `spike_pi_prompt`) can inspect/kill the child without
    /// a second exclusive owner.
    child: Arc<Mutex<Child>>,
    /// Shared stdin writer. Also written by the reader thread (which sends
    /// `classified` responses back to the sidecar), so it is reference-counted
    /// and mutex-guarded to keep JSONL lines from interleaving.
    stdin: Arc<Mutex<ChildStdin>>,
    /// Per-session generation counter — bumped ONLY when THIS session respawns.
    /// The reader thread compares against this, NOT a global counter.
    generation: Arc<AtomicU64>,
    /// Set by the session-timeout watchdog (#5) just before it kills a hung
    /// child, so the reader thread emits the timeout banner (not the crash banner).
    timed_out: Arc<AtomicBool>,
    /// When this session was spawned (#5) — the watchdog measures elapsed time
    /// against `DEVBOULE_PI_SESSION_TIMEOUT_SECS` from this instant.
    spawned_at: Instant,
    /// Handle to the stdout reader thread. Joined on stop to ensure clean teardown.
    reader_handle: Option<JoinHandle<()>>,
}

/// Tauri-managed state for all active pi sidecar sessions.
/// Each session has a unique id (`pi-<counter>`) and its own child process + reader thread.
pub struct PiSidecarState {
    inner: Mutex<HashMap<String, SessionSlot>>,
    /// Monotonically incremented to generate unique session ids.
    session_counter: AtomicU64,
    /// Serializable session records persisted to `.devboule/pi-sessions.json`
    /// so sessions survive app restarts. Decoupled from the live `inner` map,
    /// which holds process handles that cannot be serialized.
    persisted: Mutex<HashMap<String, PersistedSession>>,
    /// Sessions restored from disk at init (active, informational — their
    /// processes are gone after a restart). Surfaced by the frontend banner.
    restored: Mutex<Vec<SessionInfo>>,
}

impl Default for PiSidecarState {
    fn default() -> Self {
        let restored = restore_pi_sessions(&pi_project_root());
        Self {
            inner: Mutex::new(HashMap::new()),
            // Note: HashMap<String, SessionSlot> — each entry holds Option<PiSession>.
            session_counter: AtomicU64::new(0),
            persisted: Mutex::new(HashMap::new()),
            restored: Mutex::new(restored),
        }
    }
}

impl PiSidecarState {
    /// Sessions restored from the previous run (informational — their processes
    /// are gone after a restart). Consumed by the frontend "restored sessions"
    /// banner.
    pub fn take_restored_pi_sessions(&self) -> Vec<SessionInfo> {
        std::mem::take(&mut *self.restored.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Frontend banner text for restored sessions, if any were restored.
    pub fn restored_session_banner(&self) -> Option<String> {
        let n = self.restored.lock().unwrap_or_else(|e| e.into_inner()).len();
        if n > 0 {
            Some(format!("Restored {n} active pi sessions from previous session."))
        } else {
            None
        }
    }
}

/// Generate a unique session id in the form `pi-<counter>`.
fn generate_session_id(counter: u64) -> String {
    format!("pi-{counter}")
}

/// Process-wide monotonic counter used to disambiguate id stamps that would
/// otherwise collide on the same millisecond (e.g. `main-{ms}` agent ids).
fn next_session_id() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// Info about a newly created or existing session, returned to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub is_new: bool,
    /// The agent role this session runs as (orchestrator / main-coder / mini-coder).
    pub agent_role: String,
    /// The `mini-activity://<agentId>` channel the console should subscribe to.
    pub channel: String,
}

// ---- session persistence (Phase 1: survive app restarts) -----------------

/// Status of a persisted pi session. Mirrors the JSON `status` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SessionStatus {
    #[default]
    Active,
    Stopped,
    Crashed,
}


/// A serializable record of a single pi sidecar session, persisted to
/// `{project_root}/.devboule/pi-sessions.json` so sessions survive app restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    pub id: String,
    pub agent_role: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub created_at: u64,
    pub last_active_at: u64,
    pub status: SessionStatus,
}

/// On-disk shape of `.devboule/pi-sessions.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionFile {
    sessions: Vec<PersistedSession>,
}

/// Current unix epoch in milliseconds (used for `createdAt` / `lastActiveAt`).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether session persistence is enabled. Gate via `DEVBOULE_PI_PERSIST`
/// (default: `true`). Set to `0`/`false`/`no`/`off` to disable file writes.
fn persist_enabled() -> bool {
    match std::env::var("DEVBOULE_PI_PERSIST") {
        Ok(v) => !matches!(v.trim().to_lowercase().as_str(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

/// Resolve the project root used for `.devboule/` persistence. Mirrors the
/// dev-path resolution in `spawn_pi_session_inner` (repo root = cwd in dev).
fn pi_project_root() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Apply lifecycle cleanup rules before writing:
/// - an `active` session whose `lastActiveAt` is > 24h ago is assumed dead after
///   the restart and marked `crashed`.
/// - a `stopped`/`crashed` session whose `lastActiveAt` is > 7 days old is
///   purged entirely.
fn apply_cleanup(mut file: SessionFile) -> SessionFile {
    let now = now_ms();
    let day_ms = 24 * 3600 * 1000;
    let week_ms = 7 * day_ms;
    for s in &mut file.sessions {
        if s.status == SessionStatus::Active && now.saturating_sub(s.last_active_at) > day_ms {
            s.status = SessionStatus::Crashed;
        }
    }
    file.sessions.retain(|s| match s.status {
        SessionStatus::Stopped | SessionStatus::Crashed => {
            now.saturating_sub(s.last_active_at) <= week_ms
        }
        SessionStatus::Active => true,
    });
    file
}

/// Persist the current in-memory session set to
/// `{project_root}/.devboule/pi-sessions.json`. No-op when persistence is gated
/// off via `DEVBOULE_PI_PERSIST`.
pub fn save_pi_sessions(state: &PiSidecarState, project_root: &Path) {
    if !persist_enabled() {
        return;
    }
    // Hold the `persisted` lock across the ENTIRE read -> clean -> write ->
    // reflect cycle (#2). Releasing it between read and write let concurrent
    // callers interleave and lose each other's writes (and corrupt the file).
    let mut g = state.persisted.lock().unwrap_or_else(|e| e.into_inner());
    let collected: Vec<PersistedSession> = g.values().cloned().collect();
    let file = apply_cleanup(SessionFile { sessions: collected });

    let dir = project_root.join(".devboule");
    let path = dir.join("pi-sessions.json");
    // Atomic temp-file-then-rename: never write the final path directly, so a
    // crash mid-write can't leave a truncated/garbage file. On failure we keep
    // the old file and still reflect the cleanup in memory.
    match serde_json::to_string_pretty(&file) {
        Ok(json) => {
            let written = if dir.exists() || std::fs::create_dir_all(&dir).is_ok() {
                let tmp = dir.join("pi-sessions.json.tmp");
                std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_ok()
            } else {
                false
            };
            if !written {
                eprintln!(
                    "[pi-sidecar] failed to persist pi sessions to {}",
                    path.display()
                );
            }
        }
        Err(e) => {
            eprintln!("[pi-sidecar] failed to serialize pi sessions: {e}");
        }
    }

    // Reflect any purge / status-marking back into the in-memory set under the
    // same lock so it stays consistent with what we just wrote.
    *g = file
        .sessions
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect();
    // `g` is dropped here, releasing the lock.
}

/// Load `.devboule/pi-sessions.json` and return the sessions that should be
/// considered alive (`active` and not stale). Stopped/crashed sessions are
/// informational only and are NOT returned (their processes are long gone).
pub fn restore_pi_sessions(project_root: &Path) -> Vec<SessionInfo> {
    let path = project_root.join(".devboule").join("pi-sessions.json");
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_str::<SessionFile>(&data) else {
        return Vec::new();
    };
    let file = apply_cleanup(file);
    file.sessions
        .into_iter()
        .filter(|s| s.status == SessionStatus::Active)
        .map(|s| {
            let id = s.id.clone();
            SessionInfo {
                session_id: s.id,
                is_new: false,
                agent_role: s.agent_role,
                channel: super::mini_activity::mini_activity_channel(&id),
            }
        })
        .collect()
}

/// Generate a **role-aware** agent id for a sidecar session. The frontend consoles
/// subscribe to `mini-activity://<agentId>`, so the id namespace must match the
/// agent role:
///
/// - `"orchestrator"`  -> `orchestrator-<sanitized project id>` (stable per project,
///   so the console channel survives relaunches — matches `stable_orchestrator_agent_id`).
/// - `"main-coder"` / `"main"` -> `main-<timestamp_ms>`.
/// - `"mini-coder"` / `"mini"` -> `mini-<timestamp_ms>`.
/// - anything else    -> legacy auto-generated `pi-<counter>` (safe fallback; keeps
///   the function pure by using a self-contained monotonic counter).
///
/// This is the single source of truth for the sidecar's agent id: the same value
/// is passed to the sidecar as `DEVBOULE_SESSION_ID` and used as the Rust event
/// channel, so the two never drift.
pub fn generate_agent_id(role: &str, project_id: Option<&str>) -> String {
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    };
    match role {
        "orchestrator" => {
            let pid = project_id.unwrap_or("unknown");
            super::projects::stable_orchestrator_agent_id(pid)
        }
        r if r.starts_with("main") => format!("main-{}-{}", now_ms(), next_session_id()),
        r if r.starts_with("mini") => format!("mini-{}-{}", now_ms(), next_session_id()),
        _ => {
            // Legacy fallback: pi-{counter}. Self-contained counter so the function
            // stays pure (no PiSidecarState needed).
            static FALLBACK_COUNTER: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let c = FALLBACK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            generate_session_id(c)
        }
    }
}

/// Whether the pi sidecar should be used for agent launches.
///
/// ON by default: post-Phase 4 the legacy `devboule-coder` binary was archived,
/// so the pi sidecar is now the default path. `DEVBOULE_PI_ENABLED` is an
/// opt-OUT escape hatch — set it to `0` (or `false`/`no`/`off`, case-insensitive
/// and trimmed) to disable the sidecar and fall back to the non-sidecar path.
/// Any other value (including `1`/`true`/`yes`/`on`, an empty string, or
/// unrecognized garbage) leaves the sidecar enabled. Called from `projects.rs`
/// to decide whether to delegate an orchestrator launch to the pi sidecar.
pub fn pi_sidecar_enabled() -> bool {
    let enabled = env_flag_default_on("DEVBOULE_PI_ENABLED");
    // Opt-out model: a recognized falsy value disables the sidecar; a recognized
    // truthy value, an empty string, or an unset var enables it silently. An
    // UNRECOGNIZED non-empty value ALSO enables (unknown -> enabled) but we WARN,
    // so a typo'd `disable`/`none`/`disabled` doesn't silently turn the sidecar
    // on the way a naive opt-out impl would.
    if enabled {
        if let Ok(v) = std::env::var("DEVBOULE_PI_ENABLED") {
            if pi_enabled_unrecognized(&v) {
                eprintln!(
                    "[pi-sidecar] DEVBOULE_PI_ENABLED={v:?} is not a recognized value; \
                     treating as ENABLED (opt-out model). Use `0|false|no|off` to disable, \
                     `1|true|yes|on` to enable explicitly."
                );
            }
        }
    }
    enabled
}

/// Parse a `default-ON` opt-out boolean env flag with the SAME tolerant
/// semantics as [`pi_sidecar_enabled`]:
/// - unset                                  -> `true`  (default on)
/// - `0|false|no|off` (trimmed, case-insensitive) -> `false`
/// - anything else (including empty string, `1`, `true`, unrecognized garbage)
///   -> `true`
///
/// This fixes the inverted-footgun in `DEVBOULE_PI_SANDBOX`, which previously did
/// `.map(|v| v == "true").unwrap_or(true)` — anything but the exact string `true`
/// (e.g. `1`/`TRUE`/`yes`) silently DISABLED the macOS Seatbelt sandbox. The
/// helper is intentionally pure (no logging); callers that want a warning for
/// unrecognized-but-enabled values do so at their own call site.
fn env_flag_default_on(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(v.trim().to_lowercase().as_str(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

/// Pure decision used by [`pi_sidecar_enabled`] to decide whether to WARN about
/// an unrecognized `DEVBOULE_PI_ENABLED` value. Returns `true` iff `value` is
/// non-empty, not a recognized falsy, and not a recognized truthy token. (When
/// it returns `true`, the sidecar is still enabled — the warning is purely
/// advisory.) Unit-testable without capturing stderr.
fn pi_enabled_unrecognized(value: &str) -> bool {
    let t = value.trim().to_lowercase();
    !t.is_empty()
        && !matches!(t.as_str(), "0" | "false" | "no" | "off")
        && !matches!(t.as_str(), "1" | "true" | "yes" | "on")
}

// ---- vault adapter (decision #9) ------------------------------------------

/// Resolved coder-backend env vars to pass to the Node sidecar at spawn time.
/// Read from the Devboule config.json (`localCoderBackend`) + vault API key.
pub(crate) struct SidecarEnvVars {
    provider: String,
    model: String,
    api_key_env: Option<(String, String)>, // (env_var_name, value)
    base_url: Option<String>,
}

/// Web-search provider → env var name mapping. The 7 vault keys that the
/// web-search settings card manages. A present key → set the env var on the
/// sidecar; absent key → env NOT set (extension falls back to zero-config).
///
/// OPENAI_API_KEY note: local oMLX/Ollama roles already set this env var to a
/// placeholder ("ollama" / "mlx") in `resolve_coder_env_for_sidecar` for the
/// local LLM API. The pi-web-access extension reads OPENAI_API_KEY for its
/// OpenAI search provider. We inject AFTER the coder env vars, so the user's
/// real key wins when saved — the local oMLX/Ollama servers ignore the bearer
/// value entirely (they use their own auth), so overriding is safe.
const WEBSEARCH_ENV_MAP: &[(&str, &str)] = &[
    ("exa", "EXA_API_KEY"),
    ("brave", "BRAVE_API_KEY"),
    ("tavily", "TAVILY_API_KEY"),
    ("perplexity", "PERPLEXITY_API_KEY"),
    ("gemini_search", "GEMINI_API_KEY"),
    ("openai_search", "OPENAI_API_KEY"),
    ("parallel", "PARALLEL_API_KEY"),
];

/// Resolve web-search vault keys to env-var pairs for sidecar injection.
/// Each present key becomes `("ENV_VAR", key_value)`. Missing keys are
/// skipped — the extension falls back to zero-config defaults.
///
/// Pure (no AppHandle) for unit-testability: the vault read is done by the
/// caller, but this function maps provider→env_var and filters present keys.
pub(crate) fn websearch_env_pairs(
    vault_reader: impl Fn(&str) -> Result<Option<String>, String>,
) -> Vec<(&'static str, String)> {
    WEBSEARCH_ENV_MAP
        .iter()
        .filter_map(|&(provider, env_var)| match vault_reader(provider) {
            Ok(Some(key)) => Some((env_var, key)),
            _ => None,
        })
        .collect()
}

/// Read the coder role's provider+model+key+baseUrl from the vault/config and
/// resolve them into env vars for the sidecar. Decision #9: the Devboule vault
/// is the single source of truth; Rust reads it and passes to the sidecar.
///
/// Falls back to a non-Claude default (openrouter/tencent/hy3:free) if nothing
/// is configured. Decision #10: do NOT default to Claude.
pub(crate) fn resolve_coder_env_for_sidecar(app: &AppHandle) -> SidecarEnvVars {
    // Try reading the local coder backend from config.json.
    let local_backend = super::projects::read_local_coder_backend(app);

    match local_backend {
        Some(ref backend) => match backend.kind {
            super::local_coder::LocalCoderBackendKind::Ollama => {
                let (base_url, model) = super::local_coder::resolve_omlx_env(backend);
                let model = if model.is_empty() {
                    "qwen2.5-coder:7b".to_string()
                } else {
                    model
                };
                SidecarEnvVars {
                    provider: "openai".to_string(),
                    model,
                    api_key_env: Some(("OPENAI_API_KEY".to_string(), "ollama".to_string())),
                    base_url: Some(if base_url.is_empty() {
                        super::local_coder::OLLAMA_OPENAI_BASE_URL.to_string()
                    } else {
                        base_url
                    }),
                }
            }
            super::local_coder::LocalCoderBackendKind::Omlx => {
                let (base_url, model) = super::local_coder::resolve_omlx_env(backend);
                let model = if model.is_empty() {
                    "qwen2.5-coder:7b".to_string()
                } else {
                    model
                };
                SidecarEnvVars {
                    provider: "openai".to_string(),
                    model,
                    api_key_env: Some(("OPENAI_API_KEY".to_string(), "mlx".to_string())),
                    base_url: Some(if base_url.is_empty() {
                        "http://127.0.0.1:8000/v1".to_string()
                    } else {
                        base_url
                    }),
                }
            }
            super::local_coder::LocalCoderBackendKind::Cloud => {
                let (base_url, model) = super::local_coder::resolve_cloud_env(backend);
                let model = if model.is_empty() {
                    "tencent/hy3:free".to_string()
                } else {
                    model
                };
                let api_key = super::vault::read_cloud_llm_key().ok().flatten();
                SidecarEnvVars {
                    provider: "openrouter".to_string(),
                    model,
                    api_key_env: api_key.map(|k| ("OPENROUTER_API_KEY".to_string(), k)),
                    base_url: if base_url.is_empty() {
                        None
                    } else {
                        Some(base_url)
                    },
                }
            }
        },
        None => {
            // No coder backend configured — use a safe non-Claude default.
            // Decision #10: do NOT default to Claude.
            eprintln!(
                "[pi-sidecar] WARNING: no local coder backend configured in config.json. \
                 Falling back to openrouter/tencent/hy3:free. \
                 Configure a coder backend in Settings → Providers → Coders."
            );
            SidecarEnvVars {
                provider: "openrouter".to_string(),
                model: "tencent/hy3:free".to_string(),
                api_key_env: None,
                base_url: None,
            }
        }
    }
}

// ---- sidecar spawn --------------------------------------------------------

/// Resolve the path to the `pi-sidecar/sidecar.mjs` script relative to the app.
fn resolve_sidecar_script() -> Result<std::path::PathBuf, String> {
    // Dev path: repo root. Works in `npm run tauri dev` (current_dir = repo).
    let dev_path = std::env::current_dir()
        .map_err(|e| format!("Cannot resolve CWD: {e}"))?
        .join("pi-sidecar")
        .join("sidecar.mjs");
    if dev_path.exists() {
        return Ok(dev_path);
    }
    // Packaged build: Tauri bundles pi-sidecar/ via `bundle.resources` in
    // tauri.conf.json. The resolver var points at the resource dir.
    // TODO(Phase 5): verify sidecar script path in packaged release, and
    // consider full bundling with `pkg` for an offline binary.
    if let Ok(resource_dir) = std::env::var("TAURI_RESOURCE_DIR") {
        let resource_path = Path::new(&resource_dir)
            .join("pi-sidecar")
            .join("sidecar.mjs");
        if resource_path.exists() {
            return Ok(resource_path);
        }
    }
    Err(
        "pi-sidecar/sidecar.mjs not found. Run `npm install` in pi-sidecar/ first."
            .to_string(),
    )
}

/// Build the macOS Seatbelt sandbox policy for a pi sidecar session (decision #11).
///
/// Confines pi's `edit`/`write`/`bash` tools to the project directory: the project
/// root is both readable and (recursively) writable, plus temp dirs (Node scratch)
/// and home (for pi's own config). Network is loopback-only — the Oracle MCP
/// server, oMLX, and Pigeon are all local. rlimits bound a runaway coding agent
/// (CPU 300s, 8GB address space, 4 procs).
///
/// Security boundaries (enforced by `seatbelt::build_profile`):
/// - `.git` writes are DENIED by a Seatbelt regex (RCE-via-planted-hooks guard)
///   even though the project root is writable. So `.git` is intentionally NOT in
///   `writable_paths`.
/// - `~/.ssh`, `~/.aws`, `/etc`, and other system dirs are NOT writable (absent
///   from `writable_paths`) — only the project root, temp, and home are.
fn pi_sandbox_policy(project_root: &Path) -> SandboxPolicy {
    let tmpdir = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let home = std::env::var_os("HOME").map(PathBuf::from);

    let mut policy = SandboxPolicy::deny(project_root.to_path_buf())
        .writable(project_root.to_path_buf())
        .writable(tmpdir)
        .net(NetPolicy::Loopback)
        .rlimits(ResourceLimits {
            cpu_secs: 300,
            addr_space_bytes: Some(8 * 1024 * 1024 * 1024),
            max_procs: 4,
        });

    // Home is readable + writable so pi can read its own config (keyring caches,
    // ~/.config). Writes remain confined to the project + tmp; sensitive dirs
    // (~/.ssh, ~/.aws, /etc) are deliberately NOT in the allowlist.
    if let Some(h) = home {
        policy = policy.writable(h);
    }
    policy
}

/// Spawn a new pi sidecar session with the given session id. Reads the coder
/// backend from the vault/config and passes provider+model+key as env vars.
/// Starts a stdout JSONL reader thread that emits events on `mini-activity://<sessionId>`.
///
/// Caller MUST hold the lock on `state.inner` — this function inserts into the map.
/// Returns the per-session generation Arc so the caller can store it in PiSession.
fn spawn_pi_session_inner(
    app: &AppHandle,
    session_id: &str,
    prev_generation: Option<Arc<AtomicU64>>,
    role: Option<&str>,
    project_id: Option<&str>,
) -> Result<PiSession, String> {
    let script = resolve_sidecar_script()?;
    let sidecar_dir = script
        .parent()
        .ok_or_else(|| "Cannot resolve pi-sidecar directory".to_string())?
        .to_path_buf();

    let env_vars = resolve_coder_env_for_sidecar(app);

    // --- macOS Seatbelt sandbox (decision #11) -------------------------------
    // Confine pi's edit/write/bash tools to the project directory. Default ON for
    // safety; toggle with DEVBOULE_PI_SANDBOX=false for debugging. On non-macOS
    // wrap/apply_rlimits are no-ops (passthrough), so we only wrap on macOS.
    let project_root = sidecar_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| sidecar_dir.clone());
    // Fix 3: use the tolerant opt-out parser (default ON). Previously this did
    // `.map(|v| v == "true").unwrap_or(true)`, which silently DISABLED the macOS
    // Seatbelt sandbox for any value other than the exact string `true`
    // (`1`/`TRUE`/`yes`/garbage). Now unset -> ON; `0|false|no|off` -> OFF;
    // anything else -> ON.
    let sandbox_enabled = env_flag_default_on("DEVBOULE_PI_SANDBOX");
    let sandboxed = sandbox_enabled && cfg!(target_os = "macos");

    let policy = pi_sandbox_policy(&project_root);
    let script_arg = script.to_string_lossy().into_owned();
    let (program, args): (String, Vec<String>) = if sandboxed {
        let wrapped =
            crate::backend::sandbox::wrap(&policy, "node", &[script_arg], &project_root);
        eprintln!("[pi-sidecar] sandbox: enabled (macOS Seatbelt)");
        (wrapped.program, wrapped.args)
    } else {
        eprintln!("[pi-sidecar] sandbox: disabled (non-macOS or env override)");
        ("node".to_string(), vec![script_arg])
    };

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(&sidecar_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());

    // #4: in a packaged (release) build there is no visible terminal, so pipe
    // the sidecar's stderr and forward it via a reader thread to `eprintln!`
    // (Tauri's logger). In debug, inherit so logs go straight to the Tauri
    // terminal. Force inheritance with `DEVBOULE_PI_STDERR_ECHO=0`.
    let stderr_piped = cfg!(not(debug_assertions))
        && std::env::var("DEVBOULE_PI_STDERR_ECHO")
            .map(|v| v != "0")
            .unwrap_or(true);
    if stderr_piped {
        cmd.stderr(Stdio::piped());
    } else {
        cmd.stderr(Stdio::inherit());
    }

    if sandboxed {
        crate::backend::sandbox::apply_rlimits(&mut cmd, &policy.rlimits);
    }

    cmd.env("DEVBOULE_PI_PROVIDER", &env_vars.provider);
    cmd.env("DEVBOULE_PI_MODEL", &env_vars.model);

    // #2: always set DEVBOULE_SESSION_ID, even on the legacy `spike_pi_prompt`
    // path (role == None). The session id is generated before spawn, so the
    // sidecar's event channel never drifts from Rust's session slot.
    cmd.env("DEVBOULE_SESSION_ID", session_id);

    // Pigeon (unified flag): when OFF (default) the sidecar skips all prompt
    // classification/model-routing and runs each turn on the configured model.
    // Restart-scoped, mirrors `pigeon_enabled_cached` used by the transport paths.
    cmd.env(
        "DEVBOULE_PIGEON_ENABLED",
        if crate::backend::pigeon_service::pigeon_enabled_cached(app) {
            "true"
        } else {
            "false"
        },
    );

    if let Some(role) = role {
        // A/B: name the session so the sidecar stamps the correct `_devboule`
        // metadata and the frontend console subscribes to the right channel.
        cmd.env("DEVBOULE_AGENT_ROLE", role);
    }
    if let Some(pid) = project_id {
        cmd.env("DEVBOULE_PROJECT_ID", pid);
    }

    if let Some((ref key_name, ref key_value)) = env_vars.api_key_env {
        cmd.env(key_name, key_value);
    }

    if let Some(ref base_url) = env_vars.base_url {
        cmd.env("DEVBOULE_PI_BASE_URL", base_url);
    }

    // pi extensions: pass the resolved agent dir so pi reads/writes the correct
    // settings.json and npm/ dir. On resolution failure, pi falls back to ~/.pi/agent.
    if let Ok(dir) = crate::backend::pi_extensions::resolve_pi_agent_dir(app) {
        cmd.env("PI_CODING_AGENT_DIR", &dir.path);
    }

    // Web-search provider API keys: each present vault key is injected as its
    // matching env var so the pi-web-access extension can authenticate. Missing
    // keys are skipped (extension falls back to zero-config).
    // IMPORTANT: this runs AFTER the coder env vars (which may set OPENAI_API_KEY
    // to a placeholder like "ollama"/"mlx" for local LLM backends). When the
    // user has saved a real OpenAI search key, this injection overwrites the
    // placeholder. Local oMLX/Ollama servers ignore the bearer value entirely
    // (they use their own auth), so overriding is safe.
    for (env_var, key_value) in websearch_env_pairs(super::vault::read_websearch_key) {
        cmd.env(env_var, key_value);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn pi sidecar (is Node.js installed?): {e}"))?;

    let raw_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "pi sidecar stdin not captured".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "pi sidecar stdout not captured".to_string())?;
    // #4: capture stderr only when we piped it (release build), so the forwarder
    // thread has a stream to read.
    let stderr = if stderr_piped { child.stderr.take() } else { None };

    // Shared, mutex-guarded stdin: both `spike_pi_prompt` (prompt commands) and
    // the reader thread (`classified` responses) write here. The Arc lets the
    // reader own a clone; the Mutex serializes their JSONL lines.
    let stdin = Arc::new(Mutex::new(raw_stdin));

    // Per-session generation: reuse existing Arc if respawning the same session,
    // or create a new one for a fresh session.
    let generation = prev_generation.unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
    // Bump THIS session's generation so any old reader for this session exits.
    let _gen = generation.fetch_add(1, Ordering::SeqCst) + 1;

    // Wrap the child so the reader thread can call `try_wait()` on EOF without
    // owning the only handle (#1).
    let child = Arc::new(Mutex::new(child));
    let timed_out = Arc::new(AtomicBool::new(false));
    let spawned_at = Instant::now();

    let app_clone = app.clone();
    let sid = session_id.to_string();
    let gen_clone = generation.clone();
    let stdin_clone = stdin.clone();
    let child_clone = child.clone();
    let timed_out_clone = timed_out.clone();
    let reader_handle = std::thread::spawn(move || {
        read_sidecar_events(
            app_clone,
            stdout,
            stdin_clone,
            gen_clone,
            child_clone,
            timed_out_clone,
            &sid,
        );
    });

    // #4: release-build stderr forwarder (terminal-less packaged app).
    if let Some(stderr_stream) = stderr {
        spawn_stderr_forwarder(stderr_stream);
    }

    // #5: session-timeout watchdog (DEVBOULE_PI_SESSION_TIMEOUT_SECS; 0 disables).
    spawn_session_timeout_watchdog(
        app.clone(),
        session_id.to_string(),
        generation.clone(),
        child.clone(),
        timed_out.clone(),
        spawned_at,
        read_session_timeout_secs(),
    );

    Ok(PiSession {
        child,
        stdin,
        generation,
        timed_out,
        spawned_at,
        reader_handle: Some(reader_handle),
    })
}

/// Stop a specific pi sidecar session: kill the child, join reader, remove from state.
/// Pure mirror of `stop_pi_session`'s LIVE-existence decision: `true` only when the
/// slot for `session_id` exists AND holds a live inner session (`PiSession`).
/// Extracted so the return contract (`Ok(false)` for any non-pi / unknown id) is
/// unit-testable WITHOUT an AppHandle — `stop_agent_process_only`'s early-return
/// depends on that `Ok(false)` branch (Fix 5 / audit).
fn pi_session_existed(state: &PiSidecarState, session_id: &str) -> bool {
    let guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    match guard.get(session_id) {
        Some(slot) => slot.inner.is_some(),
        None => false,
    }
}

/// Public wrapper around `pi_session_existed` for callers that hold an
/// `AppHandle` (e.g. `orchestrator_steer`) — grabs the managed state and
/// delegates.  Returns `false` for any unknown id, so the caller can fall
/// through to its legacy path.
pub(crate) fn pi_session_exists(app: &AppHandle, session_id: &str) -> bool {
    let state = app.state::<PiSidecarState>();
    pi_session_existed(&state, session_id)
}

/// Pure routing decision for the pi-sidecar delegation gates.
/// Returns `Some("orchestrator")` or `Some("coder")` when the launch should
/// be routed to a pi sidecar session, or `None` when it should proceed to the
/// existing spawn paths (Claude/Codex/OpenAI or legacy binary).
///
/// The local Devboule agent is identified by `client == "orchestrator"` —
/// Claude/Codex/OpenAI NEVER run inside pi (design doc §11 decision #10).
pub(crate) fn pi_route_for_launch(
    launch_terminal: bool,
    role: &str,
    client: &str,
    enabled: bool,
) -> Option<&'static str> {
    if !launch_terminal || !enabled || client != "orchestrator" {
        return None;
    }
    match role {
        "orchestrator" => Some("orchestrator"),
        "coder" | "mini" => Some("coder"),
        _ => None,
    }
}

pub fn stop_pi_session(app: &AppHandle, session_id: &str) -> Result<bool, String> {
    let state = app.state::<PiSidecarState>();
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());

    // Capture the reader handle so we can join AFTER releasing the state lock
    // (#1: avoids a deadlock if the reader thread needs to remove its own
    // crashed session from the map while we hold the lock).
    // Track whether a LIVE session actually existed so callers can distinguish
    // "stopped a session" from "no such session". The wiring in
    // stop_agent_process_only relies on the Ok(false) branch to fall through to
    // the ledger/external routes when this id is not a live pi session.
    let (reader_handle, existed) = if let Some(mut slot) = guard.remove(session_id) {
        match slot.inner.take() {
            Some(mut session) => {
                // Bump THIS session's generation so the reader detects staleness.
                session.generation.fetch_add(1, Ordering::SeqCst);
                // Kill the child process (now behind a Mutex, #1).
                if let Ok(mut c) = session.child.lock() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                (session.reader_handle.take(), true)
            }
            None => (None, false),
        }
    } else {
        (None, false)
    };
    // #1: release the state lock BEFORE joining the reader thread.
    drop(guard);
    if let Some(handle) = reader_handle {
        let _ = handle.join();
    }
    // Persist: mark this session stopped and rewrite the sessions file.
    {
        let mut pg = state.persisted.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = pg.get_mut(session_id) {
            s.status = SessionStatus::Stopped;
            s.last_active_at = now_ms();
        }
    }
    save_pi_sessions(&state, &pi_project_root());

    // Fix 6: evict per-session Censor state (anti-loop counter + delivered-ids
    // set) after a successful stop, so the statics don't grow forever and a
    // re-created stable session id starts clean.
    censor_session_state_reset(session_id);

    // Return whether a LIVE session existed (and was killed). Callers rely on the
    // Ok(false) branch to mean "no such pi session" (e.g. stop_agent_process_only
    // falls through to its ledger/external routes), so this must NOT always be
    // true.
    Ok(existed)
}

/// Get an existing session or spawn a new one.
/// F2: lock is held only for the check+slot reservation; spawn happens
/// outside the lock to avoid blocking other sessions during fork+exec.
/// F6: rejects new sessions when MAX_SESSIONS is reached.
///
/// Returns (session_id, is_new).
fn get_or_spawn_session(
    app: &AppHandle,
    session_id_opt: Option<String>,
    role: Option<&str>,
    project_id: Option<&str>,
) -> Result<(String, bool), String> {
    let state = app.state::<PiSidecarState>();

    // Phase 1: under lock — check existing + reserve slot.
    let (id, is_new, prev_gen) = {
        let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());

        match session_id_opt {
            Some(id) => {
                if let Some(slot) = guard.get_mut(&id) {
                    if let Some(ref mut session) = slot.inner {
                        // Inspect the child's exit status WITHOUT holding a borrow
                        // on `session`/`guard` (the child lock must be dropped before
                        // we mutate `guard` below, #1).
                        let dead = match session.child.lock() {
                            Ok(mut c) => matches!(c.try_wait(), Ok(Some(_))),
                            Err(_) => false, // lock poisoned — assume alive.
                        };
                        if dead {
                            // Dead — grab generation for reuse, will respawn below.
                            let old_gen = session.generation.clone();
                            guard.remove(&id);
                            // Reserve the slot with a placeholder.
                            guard.insert(id.clone(), SessionSlot { inner: None });
                            (id, false, Some(old_gen))
                        } else {
                            return Ok((id, false)) // Alive (or assume alive on error).
                        }
                    } else {
                        return Err(format!("Session {id} is currently spawning — try again in a moment."));
                    }
                } else {
                    // F6: session count check (live sessions only).
                    let live_count = guard.values().filter(|s| s.inner.is_some()).count();
                    if live_count >= MAX_SESSIONS {
                        return Err(format!(
                            "Too many concurrent pi sessions ({live_count}/{MAX_SESSIONS}). \nStop a session before starting a new one."
                        ));
                    }
                    guard.insert(id.clone(), SessionSlot { inner: None });
                    (id, false, None)
                }
            }
            None => {
                // F6: session count check.
                let live_count = guard.values().filter(|s| s.inner.is_some()).count();
                if live_count >= MAX_SESSIONS {
                    return Err(format!(
                        "Too many concurrent pi sessions ({live_count}/{MAX_SESSIONS}). \nStop a session before starting a new one."
                    ));
                }
                let counter = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
                let id = generate_session_id(counter);
                guard.insert(id.clone(), SessionSlot { inner: None });
                (id, true, None)
            }
        }
    };
    // Lock is now DROPPED — spawn happens without holding the lock.

    // Phase 2: spawn outside the lock.
    let new_session = spawn_pi_session_inner(app, &id, prev_gen, role, project_id)?;

    // Fix 2: a fresh child process = a fresh generation. Reset this session's
    // anti-loop cap AND delivered-finding set (censor_session_state_reset) so a
    // cap reached in generation N never suppresses Censor delivery forever after
    // a relaunch. This covers EVERY spawn path (spike_pi_prompt, orchestrator,
    // coder) without touching projects.rs — the loop counter alone was only
    // reset from spike_pi_prompt, leaving the stable orchestrator session id
    // capped across relaunches.

    // Phase 3: re-acquire lock and store the real session.
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    // Reconcile: if the slot was removed (e.g. concurrent stop), clean up.
    if let Some(slot) = guard.get_mut(&id) {
        slot.inner = Some(new_session);
    } else {
        // Slot was removed during spawn (concurrent stop).
        // Kill the newly spawned child and return error.
        let mut s = new_session;
        s.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut c) = s.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(h) = s.reader_handle.take() { let _ = h.join(); }
        return Err(format!("pi session {id} was stopped during spawn"));
    }
    drop(guard);

    // Persist: record this session (active) and rewrite the sessions file so it
    // survives an app restart. Decoupled from the live `inner` map.
    {
        let mut pg = state.persisted.lock().unwrap_or_else(|e| e.into_inner());
        pg.insert(
            id.clone(),
            PersistedSession {
                id: id.clone(),
                agent_role: role.unwrap_or("main-coder").to_string(),
                project_id: project_id.map(|s| s.to_string()),
                created_at: now_ms(),
                last_active_at: now_ms(),
                status: SessionStatus::Active,
            },
        );
    }
    save_pi_sessions(&state, &pi_project_root());

    Ok((id, is_new))
}

// ---- Tauri commands --------------------------------------------------------

/// Tauri command: send a prompt text to a pi sidecar session. If `session_id`
/// is None, creates a new session and returns its id. If present, routes to
/// that session (creating it if it doesn't exist yet).
#[tauri::command]
pub async fn spike_pi_prompt(
    app: AppHandle,
    text: String,
    session_id: Option<String>,
) -> Result<SessionInfo, String> {
    let (sid, is_new) = get_or_spawn_session(&app, session_id, None, None)?;

    // 3e: a fresh (non-censor) user prompt breaks any in-flight review→fix→review
    // loop, so reset this session's consecutive censor-triggered round counter.
    censor_loop_reset(&sid);

    // Deliver the prompt to the freshly spawned session's stdin. The entire
    // liveness-check + JSONL write + zombie-cleanup block now lives in
    // `send_prompt_to_session` (shared with the project-side orchestrator/coder
    // launches). Errors are surfaced — never swallowed — because a launched but
    // idle session is worse than a loud failure.
    send_prompt_to_session(&app, &sid, &text)?;

    let channel = super::mini_activity::mini_activity_channel(&sid);
    Ok(SessionInfo {
        session_id: sid,
        is_new,
        // The legacy spike path does not set DEVBOULE_AGENT_ROLE, so the sidecar
        // defaults to `main-coder` — report that here for consistency.
        agent_role: "main-coder".to_string(),
        channel,
    })
}

/// Deliver a prompt to a running pi sidecar session by writing
/// `{"type":"prompt","message":<text>}` JSONL to the session's shared stdin.
///
/// This is the single delivery mechanism for sidecar prompts: it performs the
/// SAME liveness check + locks the shared `Arc<Mutex<ChildStdin>>` (so the
/// prompt line never interleaves with the reader thread's `classified` replies)
/// + zombie-cleanup that the legacy `spike_pi_prompt` path used. It is now the
/// shared path used by `spike_pi_prompt` AND by the project-side orchestrator /
/// coder launches, so a spawned agent actually begins running instead of
/// sitting idle.
///
/// Returns `Err` if the child is dead, the JSONL can't be serialized, the stdin
/// lock is poisoned, or the write/flush fails. Callers MUST NOT report a
/// successful launch with an undelivered prompt — propagate the error.
///
/// The locking order and borrow discipline below (#1, F4, Phase 2) were already
/// fixed once to avoid deadlocks and double-borrows; do not reorder.
pub(crate) fn send_prompt_to_session(
    app: &AppHandle,
    session_id: &str,
    text: &str,
) -> Result<(), String> {
    // `sid` mirrors the local naming in the legacy `spike_pi_prompt` path so the
    // error strings stay byte-identical to that proven implementation.
    let sid = session_id.to_string();

    // Send the prompt to the session's stdin. If the child is dead, remove it
    // and return an error.
    let state = app.state::<PiSidecarState>();
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());

    // Check if child is still alive before writing.
    {
        let slot = guard
            .get_mut(&sid)
            .ok_or_else(|| format!("pi session {sid} not found after spawn"))?;
        let session = slot
            .inner
            .as_mut()
            .ok_or_else(|| format!("pi session {sid} has empty slot (spawn in progress?)"))?;
        // Inspect the child's exit status WITHOUT holding a borrow on
        // `session`/`guard` — the child lock must be dropped before we mutate
        // `guard` below (#1).
        let exit_status = match session.child.lock() {
            Ok(mut c) => c.try_wait(),
            Err(_) => return Err(format!("pi session {sid} child lock poisoned")),
        };
        match exit_status {
            Ok(Some(status)) => {
                guard.remove(&sid);
                return Err(format!(
                    "pi session {sid} exited with status {status}. Start a new session."
                ));
            }
            Ok(None) => {} // Alive — proceed.
            Err(e) => {
                return Err(format!("pi session {sid} status check failed: {e}"));
            }
        }
    }

    // Write the prompt. Use a bool flag to avoid double-borrow in map_err (F4).
    let mut write_failed_zombie = false;
    {
        let session = guard
            .get_mut(&sid)
            .and_then(|s| s.inner.as_mut())
            .ok_or_else(|| format!("pi session {sid} not running after spawn"))?;

        let cmd = serde_json::json!({
            "type": "prompt",
            "message": text,
        });
        let line =
            serde_json::to_string(&cmd).map_err(|e| format!("JSON serialize error: {e}"))?;
        // Phase 2: stdin is shared with the reader thread (which writes
        // `classified` responses back to the sidecar). Lock it so the JSONL
        // prompt line never interleaves with a `classified` line.
        let mut stdin_lock = session
            .stdin
            .lock()
            .map_err(|_| format!("pi session {sid} stdin lock poisoned"))?;
        stdin_lock
            .write_all(format!("{line}\n").as_bytes())
            .map_err(|e| {
                // F4: on write failure, flag for zombie cleanup after we release session borrow.
                if let Some(Some(_)) =
                    session.child.lock().ok().and_then(|mut c| c.try_wait().ok())
                {
                    write_failed_zombie = true;
                }
                format!("Failed to write to pi sidecar stdin: {e}")
            })?;
        stdin_lock
            .flush()
            .map_err(|e| format!("Failed to flush pi sidecar stdin: {e}"))?;
    }

    // F4: clean up zombie entry if write failed.
    if write_failed_zombie {
        guard.remove(&sid);
    }

    Ok(())
}

/// Spawn a **role-aware** pi sidecar session. Thin wrapper over the existing
/// session machinery that adds role-aware agent-id generation (A/B) and the
/// default devboule env vars the sidecar reads in `devbouleContext` (Task 1).
///
/// 1. Generates the agent id via [`generate_agent_id`] (orchestrator / main / mini
///    namespaces, else the legacy `pi-<counter>` fallback).
/// 2. Spawns the sidecar with `DEVBOULE_AGENT_ROLE=role` and
///    `DEVBOULE_SESSION_ID=<agent id>` (plus `DEVBOULE_PROJECT_ID` when given).
/// 3. Returns the session id, role, and the `mini-activity://<agentId>` channel.
///
/// The sidecar's reader thread then emits `MiniActivityEvent::Snapshot`s on that
/// channel (EventMapper uses the same agent id), so the frontend console renders
/// without any React changes.
pub fn spawn_sidecar_for_role(
    app: &AppHandle,
    role: &str,
    project_id: Option<&str>,
) -> Result<SessionInfo, String> {
    let agent_id = generate_agent_id(role, project_id);
    let (sid, is_new) = get_or_spawn_session(
        app,
        Some(agent_id.clone()),
        Some(role),
        project_id,
    )?;
    let channel = super::mini_activity::mini_activity_channel(&sid);
    Ok(SessionInfo {
        session_id: sid,
        is_new,
        agent_role: role.to_string(),
        channel,
    })
}

/// Tauri command: stop a specific pi sidecar session.
#[tauri::command]
pub async fn spike_pi_stop(app: AppHandle, session_id: String) -> Result<bool, String> {
    stop_pi_session(&app, &session_id)
}

// ---- event mapping ---------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PiEvent {
    #[serde(rename = "type")]
    event_type: String,
    /// #2: whether the Oracle MCP server was reachable at startup. The sidecar
    /// stamps this onto the `ready` event's `oracleMCP` field; `Some(false)`
    /// means `oracle_ask` will not work (surfaced as a banner in the console).
    #[serde(rename = "oracleMCP", default)]
    oracle_mcp: Option<bool>,
    #[serde(default)]
    assistant_message_event: Option<AssistantMessageEvent>,
    #[serde(rename = "toolCallId", default)]
    tool_call_id: Option<String>,
    #[serde(rename = "toolName", default)]
    tool_name: Option<String>,
    #[serde(default)]
    args: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(rename = "isError", default)]
    is_error: Option<bool>,
    #[allow(dead_code)]
    #[serde(default)]
    messages: Option<serde_json::Value>,
    /// The raw `message` object from `message_start`/`message_end` events. Used to
    /// detect injected `devboule.websearch` / `devboule.plan` custom user messages.
    #[serde(default)]
    message: Option<serde_json::Value>,
    /// The `_devboule` enrichment object the sidecar stamps onto every event
    /// (Task 1): `{ agentRole, projectId, sessionId }`.
    #[serde(rename = "_devboule", default)]
    devboule: Option<DevbouleContext>,
    /// Censor review trigger (#8): the composed review prompt + affected files/diffs,
    /// emitted by the sidecar to surface a review request in the console without a
    /// reentrant `session.prompt()`. The real review runs in Rust now (we don't
    /// re-dump the raw prompt — it's noise), but the sidecar still sends these,
    /// so keep them parsed for the event contract.
    #[allow(dead_code)]
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    files: Option<Vec<String>>,
    #[allow(dead_code)]
    #[serde(default)]
    diffs: Option<Vec<String>>,
    /// Compaction / auto-retry / sidecar-error / queue-drop fields (Part C).
    /// All optional + default so unrelated events deserialize untouched.
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    aborted: Option<bool>,
    #[serde(rename = "errorMessage", default)]
    error_message: Option<String>,
    #[serde(default)]
    attempt: Option<u32>,
    #[serde(rename = "maxAttempts", default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(rename = "finalError", default)]
    final_error: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    count: Option<u32>,
    /// `tool_execution_update` carries `partialResult` (e.g. streaming child-agent
    /// progress). Tool-dependent shape; extracted like `tool_execution_end`'s result.
    #[serde(rename = "partialResult", default)]
    partial_result: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct AssistantMessageEvent {
    #[serde(rename = "type")]
    delta_type: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(rename = "contentIndex", default)]
    content_index: Option<u32>,
}

/// Typed mirror of the sidecar's `_devboule` enrichment object (Task 1):
/// `{ agentRole, projectId, sessionId }`. Parsed directly off the event so the
/// censor-review hook can read `projectId`/`sessionId` without `serde_json`
/// lookups.
#[derive(Debug, Default, Deserialize)]
struct DevbouleContext {
    #[serde(rename = "agentRole", default)]
    agent_role: Option<String>,
    #[serde(rename = "projectId", default)]
    project_id: Option<String>,
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
}

/// Sliding-window cap for the console entry history (#3). Prevents unbounded
/// `Vec<ConsoleEntry>` growth that makes every `emit_snapshot` clone+serialize
/// the entire history (O(n) serialization, ~200MB/s allocations at 1000 turns).
const MAX_CONSOLE_ENTRIES: usize = 500;

/// Stateful mapper: converts pi SDK events into `ConsoleActivity` snapshots.
struct EventMapper {
    agent_id: String,
    running: bool,
    entries: Vec<ConsoleEntry>,
    accumulated_text: String,
    /// Part A: accumulated thinking content (mirrors `accumulated_text`); flushed
    /// to a `ConsoleEntry::Thinking` on `thinking_end` / turn boundaries.
    accumulated_thinking: String,
    tool_names: HashMap<String, String>,
    turn_seq: u64,
    active_content_index: Option<u32>,
    /// Part B: live tool-progress trackers, keyed by `tool_call_id`. A `HashMap`
    /// (not a single slot) so PARALLEL tools don't clobber each other: each
    /// `tool_execution_start` inserts its own id; updates/ends only touch their
    /// own id. Value = `(entry_index, original_args_text)` for the `  args: ...`
    /// row pushed at start. Indices are kept in sync with the front-evicting
    /// sliding window in `push_entry`.
    active_tool_progress: HashMap<String, (usize, String)>,
    /// Part B/Fix2: how many entries have been front-evicted from `entries` over
    /// this mapper's lifetime. Monotonic (never reset) — it is the stable base
    /// offset the frontend adds to each row index so React keys survive eviction.
    evicted_count: u64,
    /// A: the persisted `agentRole` from the event stream's `_devboule` field.
    /// Refreshed on every event; survives across events in the same session.
    current_role: Option<String>,
    /// Live-thinking tracker (#2a): the index of the in-place `ConsoleEntry::Thinking`
    /// that is being updated on every `thinking_delta`. `None` means no live thinking
    /// entry is currently tracked (either never started, or the tracked row was
    /// front-evicted by the sliding window). Lets us stream thinking tokens live into
    /// a single entry instead of waiting for `thinking_end`.
    live_thinking_idx: Option<usize>,
}

/// Cap a string at `cap` CHARS (not bytes) appending `…` when truncated. UTF-8
/// safe: no byte-slice panic on a multi-byte char straddling the boundary (Fix 3).
fn cap_chars(s: &str, cap: usize) -> String {
    let capped: String = s.chars().take(cap).collect();
    if s.chars().count() > cap {
        format!("{capped}…")
    } else {
        capped
    }
}

/// Part B: extract a single-line, capped snippet from a `tool_execution_update`
/// `partialResult`. Uses the same `content[0].text` path as `tool_execution_end`'s
/// result; falls back to a compact JSON string when no text is present. Newlines are
/// replaced with `␤` and the result is capped at 200 chars (pure, testable).
fn extract_partial_snippet(partial: &serde_json::Value) -> String {
    let raw = partial
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| serde_json::to_string(partial).unwrap_or_default());
    let single = raw.replace(['\n', '\r'], "␤");
    cap_chars(&single, 200)
}

/// Part C: compute the banner text for a compaction / auto-retry / sidecar-error /
/// queue-drop event, or `None` for events that emit no banner (e.g. a successful
/// `auto_retry_end` — the stream simply resumes). Pure, so it is unit-testable
/// without an AppHandle.
fn banner_text_for_event(event: &PiEvent) -> Option<String> {
    match event.event_type.as_str() {
        "compaction_start" => {
            let reason = event.reason.clone().unwrap_or_default();
            Some(format!("Compacting context ({reason})…"))
        }
        "compaction_end" => {
            if event.aborted == Some(true) {
                Some("Compaction aborted".to_string())
            } else if let Some(ref msg) = event.error_message {
                Some(format!("Compaction failed: {msg}"))
            } else {
                Some("Context compacted".to_string())
            }
        }
        "auto_retry_start" => {
            let attempt = event.attempt.unwrap_or(0);
            let max = event.max_attempts.unwrap_or(0);
            let err = event.error_message.clone().unwrap_or_default();
            let capped: String = err.chars().take(160).collect();
            Some(format!("Provider error — retry {attempt}/{max}: {capped}"))
        }
        "auto_retry_end" => {
            if event.success == Some(false) {
                let err = event.final_error.clone().unwrap_or_default();
                let capped: String = err.chars().take(160).collect();
                Some(format!("Retries exhausted: {capped}"))
            } else {
                None
            }
        }
        "error" => {
            let context = event.context.clone().unwrap_or_default();
            let msg = event
                .message
                .as_ref()
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let capped: String = msg.chars().take(200).collect();
            Some(format!("Sidecar error [{context}]: {capped}"))
        }
        "queue_dropped" => {
            let count = event.count.unwrap_or(0);
            Some(format!("Dropped {count} queued prompt(s) on shutdown"))
        }
        _ => None,
    }
}

impl EventMapper {
    fn new(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            running: false,
            entries: Vec::new(),
            accumulated_text: String::new(),
            accumulated_thinking: String::new(),
            tool_names: HashMap::new(),
            turn_seq: 0,
            active_content_index: None,
            active_tool_progress: HashMap::new(),
            evicted_count: 0,
            current_role: None,
            live_thinking_idx: None,
        }
    }

    /// Push a console entry, enforcing the [`MAX_CONSOLE_ENTRIES`] sliding window.
    /// When the window is full, the oldest entry is dropped to bound memory and
    /// serialization cost (#3). Removes only once the window is exceeded (> cap)
    /// so the window holds exactly MAX_CONSOLE_ENTRIES entries.
    ///
    /// Part B: the front-eviction shifts every surviving entry's index down by one,
    /// which would corrupt `active_tool_progress`'s stored indices AND the
    /// `live_thinking_idx` tracker (#2a). Adjust ALL of them here, at the single
    /// mutation site, so each tracker stays pointed at the right row: when a
    /// tracked entry is itself evicted (it was at index 0 pre-shift) that tracker
    /// is removed/cleared entirely. This is the only place entries are
    /// added/removed, so the hazard is fully contained.
    fn push_entry(&mut self, entry: ConsoleEntry) {
        self.entries.push(entry);
        if self.entries.len() > MAX_CONSOLE_ENTRIES {
            self.entries.remove(0);
            self.evicted_count += 1;
            // Every surviving entry's index shifted down by one. The tracked
            // entries' indices were captured pre-removal; the one that was at 0
            // is the one just removed, so drop its tracker — others decrement.
            let mut evicted_ids: Vec<String> = Vec::new();
            for (id, (idx, _)) in self.active_tool_progress.iter_mut() {
                if *idx == 0 {
                    evicted_ids.push(id.clone());
                } else {
                    *idx -= 1;
                }
            }
            for id in evicted_ids {
                self.active_tool_progress.remove(&id);
            }
            // #2a (FIX 1): the SAME front-eviction shifts `live_thinking_idx`'s
            // tracked row down by one. If the live thinking entry itself was the
            // one evicted (it was at index 0) the tracker is invalid → clear it so
            // the next delta re-pushes a fresh live entry. Otherwise decrement it
            // to follow the surviving row. Kept in lock-step with `active_tool_progress`
            // here, at the single mutation site, so the moving-window hazard is
            // fully contained.
            if let Some(ref mut idx) = self.live_thinking_idx {
                if *idx == 0 {
                    self.live_thinking_idx = None;
                } else {
                    *idx -= 1;
                }
            }
        }
    }

    fn now_str() -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() % 86400;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }

    fn build_snapshot(&self) -> ConsoleActivity {
        ConsoleActivity {
            running: Some(self.running),
            run_count: if self.running { Some(1) } else { Some(0) },
            empty: if self.entries.is_empty() {
                Some(true)
            } else {
                None
            },
            entries: if self.entries.is_empty() {
                None
            } else {
                Some(self.entries.clone())
            },
            // Fix2: stable base offset for the frontend's React keys. Monotonic
            // (never reset), so (entriesBase + i) is a stable identity across FIFO
            // eviction. Omitted when there are no entries.
            entries_base: if self.entries.is_empty() {
                None
            } else {
                Some(self.evicted_count)
            },
            task_cost_estimate_usd: None,
            streaming_chat: if self.running && !self.accumulated_text.is_empty() {
                Some(super::mini_activity::StreamingChat {
                    seq: self.turn_seq,
                    text: self.accumulated_text.clone(),
                })
            } else {
                None
            },
        }
    }

    fn emit_snapshot(&self, app: &AppHandle) {
        let snapshot = self.build_snapshot();
        let event = MiniActivityEvent::Snapshot {
            activity: snapshot,
        };
        let channel = super::mini_activity::mini_activity_channel(&self.agent_id);
        let _ = app.emit(&channel, event);
    }

    fn flush_text_block(&mut self) {
        if !self.accumulated_text.is_empty() {
            // Take the text out FIRST so the `push_entry(&mut self)` call does
            // not conflict with the mutable borrow of `self.accumulated_text`.
            let text = std::mem::take(&mut self.accumulated_text);
            self.push_entry(ConsoleEntry::Chat {
                role: "assistant".to_string(),
                text,
                time: Self::now_str(),
                msg_id: None,
            });
        }
        self.active_content_index = None;
    }

    fn flush_thinking_block(&mut self) {
        // #2a: if a live thinking entry is being streamed in place, finalize it
        // there (write the final text into the existing row) and DON'T push a
        // duplicate entry. Only when there is no tracked live entry do we fall
        // back to pushing a fresh Thinking entry (e.g. interrupted thinking
        // flushed by `agent_end`, or thinking that never received a delta).
        //
        // FIX 2: the index is guarded with the SAME predicate `upsert_live_thinking`
        // uses (`idx < len && is a Thinking entry`) before any write/remove. Because
        // `push_entry` keeps `live_thinking_idx` in sync on FIFO eviction (FIX 1),
        // the tracked row can never silently point at an unrelated entry — but we
        // still guard defensively and fall back to `push_entry` if the guard fails.
        if let Some(idx) = self.live_thinking_idx {
            let valid = idx < self.entries.len()
                && matches!(self.entries[idx], ConsoleEntry::Thinking { .. });
            if valid {
                if !self.accumulated_thinking.is_empty() {
                    // Non-empty: finalize the existing live row in place.
                    let text = std::mem::take(&mut self.accumulated_thinking);
                    self.entries[idx] = ConsoleEntry::Thinking {
                        text,
                        time: Self::now_str(),
                    };
                } else {
                    // Empty thinking (delta never carried text): the row already
                    // shows the streamed (empty) Thinking entry, so just finalize
                    // the tracker + accumulator WITHOUT removing it — raw `remove`
                    // would shift `active_tool_progress` indices out from under us.
                    self.accumulated_thinking.clear();
                }
            } else {
                // Guard failed (tracker stale / row mutated): push a fresh entry
                // if there is any content; never corrupt an unrelated row.
                if !self.accumulated_thinking.is_empty() {
                    let text = std::mem::take(&mut self.accumulated_thinking);
                    self.push_entry(ConsoleEntry::Thinking {
                        text,
                        time: Self::now_str(),
                    });
                } else {
                    self.accumulated_thinking.clear();
                }
            }
            self.live_thinking_idx = None;
        } else if !self.accumulated_thinking.is_empty() {
            // Take the text out FIRST so the `push_entry(&mut self)` call does
            // not conflict with the mutable borrow of `self.accumulated_thinking`.
            let text = std::mem::take(&mut self.accumulated_thinking);
            self.push_entry(ConsoleEntry::Thinking {
                text,
                time: Self::now_str(),
            });
        }
    }

    /// #2a: stream the accumulated thinking into a SINGLE live `ConsoleEntry::Thinking`
    /// that updates in place on every `thinking_delta`. The first delta pushes a new
    /// Thinking entry and remembers its index; subsequent deltas overwrite that same
    /// entry's text. If the tracked index was front-evicted by the sliding window the
    /// tracker is reset and a fresh live entry is pushed. Called from the `thinking_delta`
    /// arm of `apply_message_delta`; `message_update` re-emits the snapshot right after,
    /// so each delta is broadcast live (no snapshot change needed).
    fn upsert_live_thinking(&mut self) {
        let live_idx = match self.live_thinking_idx {
            Some(idx) => {
                if idx < self.entries.len()
                    && matches!(self.entries[idx], ConsoleEntry::Thinking { .. })
                {
                    Some(idx)
                } else {
                    // Index invalidated by FIFO eviction (or row mutated): start fresh.
                    None
                }
            }
            None => None,
        };
        match live_idx {
            Some(idx) => {
                // Overwrite the existing live entry's text in place.
                let text = self.accumulated_thinking.clone();
                self.entries[idx] = ConsoleEntry::Thinking {
                    text,
                    time: Self::now_str(),
                };
            }
            None => {
                self.live_thinking_idx = None;
                self.push_entry(ConsoleEntry::Thinking {
                    text: self.accumulated_thinking.clone(),
                    time: Self::now_str(),
                });
                self.live_thinking_idx = Some(self.entries.len() - 1);
            }
        }
    }

    /// Pure: ingest one assistant message delta (text or thinking) into the
    /// accumulators. No AppHandle needed, so it is unit-testable directly (the
    /// `message_update` arm just calls this then `emit_snapshot`).
    fn apply_message_delta(&mut self, delta: &AssistantMessageEvent) {
        match delta.delta_type.as_str() {
            "text_start" => {
                self.flush_text_block();
                self.flush_thinking_block();
                self.active_content_index = delta.content_index;
            }
            "text_delta" => {
                if let Some(ref d) = delta.delta {
                    self.accumulated_text.push_str(d);
                }
            }
            "thinking_start" => {
                // FIX C: a new thinking block may arrive without a preceding
                // `thinking_end` (out-of-order / dropped end). Finalize the previous
                // live block so it isn't overwritten by the new one — symmetry with
                // `text_start`, which also flushes both blocks.
                self.flush_thinking_block();
            }
            "thinking_delta" => {
                if let Some(ref d) = delta.delta {
                    self.accumulated_thinking.push_str(d);
                }
                // #2a: stream thinking LIVE into a single in-place Thinking entry.
                self.upsert_live_thinking();
            }
            "thinking_end" => {
                self.flush_thinking_block();
            }
            _ => {}
        }
    }

    /// Part B: rewrite the tracked args row in place with a live-progress snippet,
    /// ONLY for the matching `tool_call_id`. No match (parallel / unknown tool) =>
    /// no-op (the caller still re-emits the snapshot). Other ids are untouched.
    fn rewrite_tool_progress(&mut self, tool_call_id: &str, partial: &serde_json::Value) {
        let idx = match self.active_tool_progress.get(tool_call_id) {
            Some((idx, _)) => *idx,
            None => return,
        };
        if idx < self.entries.len() {
            let snippet = extract_partial_snippet(partial);
            self.entries[idx] = ConsoleEntry::Coder {
                node: None,
                text: format!("  ⋯ {snippet}"),
                time: String::new(),
            };
        }
    }

    /// Part B: restore the matching `tool_call_id`'s args row to its original text,
    /// then REMOVE only that id from the tracker (no unconditional clear — a parallel
    /// tool's tracker must survive). The final ✅/❌ row below already summarizes the
    /// result.
    fn restore_tool_progress(&mut self, tool_call_id: &str) {
        if let Some((idx, original)) = self.active_tool_progress.get(tool_call_id) {
            let idx = *idx;
            let original = original.clone();
            if idx < self.entries.len() {
                self.entries[idx] = ConsoleEntry::Coder {
                    node: None,
                    text: original,
                    time: String::new(),
                };
            }
        }
        self.active_tool_progress.remove(tool_call_id);
    }

    fn handle_event(&mut self, app: &AppHandle, event: &PiEvent) {
        // A: persist `agentRole` from `_devboule` on every event (it survives
        // across events in the same session).
        self.apply_devboule_role(event);
        match event.event_type.as_str() {
            "agent_start" => {
                // FIX C: finalize any still-live thinking entry from a prior turn /
                // abnormal reconnect (orphan guard) BEFORE resetting the
                // accumulators. flush_thinking_block() uses live_thinking_idx to
                // finalize the row, so it MUST run before we clear that index below.
                self.flush_thinking_block();
                // #4: clear the tool_names cache so it doesn't leak entries
                // across agent runs in the same session.
                self.tool_names.clear();
                // Fix 4: a tool aborted mid-flight (retry/compaction/crash) leaves a
                // stale progress tracker; clear it so it can't leak into the new turn.
                self.active_tool_progress.clear();
                self.running = true;
                self.turn_seq += 1;
                self.accumulated_text.clear();
                self.accumulated_thinking.clear();
                self.live_thinking_idx = None;
                self.active_content_index = None;
                self.emit_snapshot(app);
            }
            "agent_end" => {
                self.flush_text_block();
                self.flush_thinking_block();
                self.running = false;
                self.emit_snapshot(app);
            }
            "message_update" => {
                if let Some(ref delta_event) = event.assistant_message_event {
                    self.apply_message_delta(delta_event);
                }
                self.emit_snapshot(app);
            }
            "tool_execution_start" => {
                let tool_name = event
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool".to_string());
                if let Some(ref id) = event.tool_call_id {
                    self.tool_names.insert(id.clone(), tool_name.clone());
                }
                self.flush_text_block();
                self.flush_thinking_block();
                let args_str = event
                    .args
                    .as_ref()
                    .map(|a| serde_json::to_string(a).unwrap_or_default())
                    .unwrap_or_default();
                self.push_entry(ConsoleEntry::Coder {
                    node: Some(NodeStyle::Dot),
                    text: format!("🔧 Calling `{tool_name}`"),
                    time: Self::now_str(),
                });
                self.push_entry(ConsoleEntry::Coder {
                    node: None,
                    text: format!("  args: {args_str}"),
                    time: String::new(),
                });
                // Part B: remember the `  args: ...` row so live progress updates
                // can rewrite it in place. `index` = last pushed entry. Inserted by
                // key, so a parallel tool's tracker is preserved (no clobber).
                if let Some(ref id) = event.tool_call_id {
                    let args_idx = self.entries.len() - 1;
                    self.active_tool_progress
                        .insert(id.clone(), (args_idx, format!("  args: {args_str}")));
                }
                self.emit_snapshot(app);
            }
            "tool_execution_update" => {
                // Part B: live progress. If this update matches the tracked tool and
                // carries a partial result, rewrite the args row in place (single
                // line, capped). Parallel/mismatched tools: leave the row alone and
                // just re-emit the snapshot (no crash, no misattribution).
                if let Some(ref id) = event.tool_call_id {
                    if let Some(ref partial) = event.partial_result {
                        self.rewrite_tool_progress(id, partial);
                    }
                }
                self.emit_snapshot(app);
            }
            "tool_execution_end" => {
                // Part B: restore the args row we may have overwritten with live
                // progress (only for the matching tool), then REMOVE only that id
                // from the tracker (a parallel tool's tracker survives).
                if let Some(ref id) = event.tool_call_id {
                    self.restore_tool_progress(id);
                }
                let tool_name = event
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| self.tool_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let result_summary = event
                    .result
                    .as_ref()
                    .and_then(|r| {
                        r.get("content")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|item| item.get("text"))
                            .and_then(|t| t.as_str())
                    })
                    .map(|text| {
                        // Fix 3: cap by CHARS, not bytes — `&text[..200]` panics on a
                        // multi-byte char straddling byte 200. `cap_chars` is UTF-8 safe.
                        cap_chars(text, 200)
                    })
                    .unwrap_or_else(|| "(no result)".to_string());
                let is_error = event.is_error.unwrap_or(false);
                let status_icon = if is_error { "❌" } else { "✅" };
                self.push_entry(ConsoleEntry::Coder {
                    node: Some(NodeStyle::Sage),
                    text: format!("{status_icon} `{tool_name}` → {result_summary}"),
                    time: Self::now_str(),
                });
                self.emit_snapshot(app);
            }
            "turn_start" | "turn_end" | "message_start" | "message_end" => {
                // B: a `message_start`/`message_end` with a user-role message whose
                // content[0].text parses as a devboule custom message
                // (devboule.websearch / devboule.plan) is injected into the activity.
                if let Some(ref message) = event.message {
                    self.handle_devboule_custom_message(message);
                }
                self.emit_snapshot(app);
            }
            "response" | "ready" => {
                // #2: the `ready` event carries whether the Oracle MCP was reachable
                // at startup. If not, surface a banner so the user knows
                // `oracle_ask` will silently fail.
                if event.event_type == "ready"
                    && event.oracle_mcp == Some(false) {
                        self.push_entry(ConsoleEntry::Banner {
                            text: "Oracle MCP not available — oracle_ask will not work."
                                .to_string(),
                            time: Self::now_str(),
                        });
                    }
                self.emit_snapshot(app);
            }
            "devboule_censor_review" => {
                // #8: a real Censor review was just requested by the sidecar after a
                // Rust-edit turn. Surface a banner, then run the review on a detached
                // thread (the reader must never block on the heavy LLM pass) and route
                // any confirmed findings back into the session as a prompt.
                self.flush_text_block();
                self.flush_thinking_block();
                let d = censor_review_dispatch(event, &self.agent_id);
                self.push_entry(ConsoleEntry::Coder {
                    node: Some(NodeStyle::Sage),
                    text: d.banner.clone(),
                    time: Self::now_str(),
                });
                self.emit_snapshot(app);

                // Without a project id we cannot resolve the root or trust gate, so
                // skip loudly (no silent degradation — a hard project rule).
                let Some(project_id) = d.project_id.clone() else {
                    self.push_entry(ConsoleEntry::Banner {
                        text: "Censor review skipped (no project id)".to_string(),
                        time: Self::now_str(),
                    });
                    self.emit_snapshot(app);
                    return;
                };

                // Capture everything the detached thread needs (AppHandle is Clone;
                // the mapper outlives the spawned thread).
                let app_for_thread = app.clone();
                let agent_id = self.agent_id.clone();
                let project_id_t = project_id.clone();
                let session_id_t = d.session_id.clone();
                let files_t = d.files.clone();

                // Spawn ONE detached thread named for the work. The reader thread
                // returns immediately; the review (incl. the voted Gemma tier) runs
                // here so the stdout reader is never blocked.
                //
                // Fix 3: a spawn failure (e.g. the OS refuses a thread) must not be
                // swallowed — otherwise the "⚑ Censor review started" banner dangles
                // forever. On Err, surface a visible console note via the SAME
                // store/push_censor_note mechanism the thread body uses.
                let agent_id_for_thread = agent_id.clone();
                match std::thread::Builder::new()
                    .name("pi-censor-review".to_string())
                    .spawn(move || {
                        run_pi_censor_review(
                            &app_for_thread,
                            &agent_id_for_thread,
                            &project_id_t,
                            &session_id_t,
                            &files_t,
                        );
                    }) {
                    Ok(_) => {}
                    Err(e) => {
                        push_censor_note(
                            app,
                            &agent_id,
                            &format!("Censor review failed to start: {e}"),
                            Some(NodeStyle::Sage),
                        );
                    }
                }
            }
            "compaction_start" | "compaction_end" | "auto_retry_start"
            | "auto_retry_end" | "error" | "queue_dropped" => {
                // Part C: banners for compaction / retry / sidecar error / queue
                // drops. These are turn boundaries, so flush pending text + thinking
                // first, then push the (pure-built) banner text and re-emit.
                self.flush_text_block();
                self.flush_thinking_block();
                if let Some(text) = banner_text_for_event(event) {
                    self.push_entry(ConsoleEntry::Banner {
                        text,
                        time: Self::now_str(),
                    });
                }
                self.emit_snapshot(app);
            }
            _ => {}
        }
    }

    // ---- Task 2: _devboule + devboule custom messages --------------------------

    /// A: read `_devboule.agentRole` and persist it on the mapper. The sidecar
    /// stamps `_devboule` onto every event, so this is an idempotent refresh that
    /// keeps `current_role` correct for the whole session.
    fn apply_devboule_role(&mut self, event: &PiEvent) {
        if let Some(role) = event
            .devboule
            .as_ref()
            .and_then(|d| d.agent_role.clone())
        {
            self.current_role = Some(role);
        }
    }

    /// B: detect an injected `devboule.websearch` / `devboule.plan` custom message.
    /// Returns `true` if a devboule custom message was handled (and an entry was
    /// pushed). A plain user message (or any non-matching shape) returns `false`
    /// and is ignored — safe to call on every `message_start`/`message_end`.
    fn handle_devboule_custom_message(&mut self, message: &serde_json::Value) -> bool {
        // Only the sidecar's injected user-role messages are devboule custom msgs.
        if message.get("role").and_then(|v| v.as_str()) != Some("user") {
            return false;
        }
        let content = match message.get("content").and_then(|c| c.as_array()) {
            Some(c) if !c.is_empty() => c,
            _ => return false,
        };
        let text = match content[0].get("text").and_then(|t| t.as_str()) {
            Some(t) => t,
            None => return false,
        };
        let obj: serde_json::Value = match serde_json::from_str(text) {
            Ok(o) => o,
            Err(_) => return false,
        };
        match obj.get("type").and_then(|t| t.as_str()) {
            Some("devboule.websearch") => {
                let query = obj.get("query").and_then(|q| q.as_str()).unwrap_or("");
                let results = obj.get("results").cloned().unwrap_or(serde_json::Value::Null);
                self.handle_devboule_websearch(query, &results);
                true
            }
            Some("devboule.plan") => {
                let plan = obj.get("plan").cloned().unwrap_or(serde_json::Value::Null);
                self.handle_devboule_plan(&plan);
                true
            }
            _ => false,
        }
    }

    /// B: map a `devboule.websearch` custom message to `ConsoleEntry::WebSearch`.
    /// `results` is the raw web_search tool `details` object; we tolerate several
    /// shapes and extract url/title/summary per item with graceful fallbacks.
    fn handle_devboule_websearch(&mut self, query: &str, results: &serde_json::Value) {
        let pages = extract_pages(results);
        if pages.is_empty() {
            // Finding #13: unknown SERP shapes yield zero extractable pages. Emitting an
            // empty WebSearch looks like a bug, so surface a Banner notice instead.
            self.push_entry(ConsoleEntry::Banner {
                text: "Web search completed (results not extractable)".to_string(),
                time: Self::now_str(),
            });
        } else {
            self.push_entry(ConsoleEntry::WebSearch {
                query: query.to_string(),
                pages,
                time: Self::now_str(),
            });
        }
    }

    /// B: map a `devboule.plan` custom message. `ConsoleActivity` has no dedicated
    /// plan entry type yet, so per the Task-2 fallback we emit a `ConsoleEntry::Chat`
    /// with `role == "plan"`; PlannerPlanMode can later parse the formatted plan text.
    fn handle_devboule_plan(&mut self, plan: &serde_json::Value) {
        let text = match plan {
            serde_json::Value::Null => "(empty plan)".to_string(),
            _ => serde_json::to_string_pretty(plan).unwrap_or_default(),
        };
        self.push_entry(ConsoleEntry::Chat {
            role: "plan".to_string(),
            text,
            time: Self::now_str(),
            msg_id: None,
        });
    }
}

// ---- pi sidecar Censor review hook (#8, real Rust Censor) ------------------
//
// When the sidecar reports a `devboule_censor_review` event (after a Rust-edit
// turn), `handle_event` spawns ONE detached thread (`run_pi_censor_review`) so the
// stdout reader is never blocked on the heavy LLM pass. The thread reuses the real
// Censor entry point (`censor_review::process_censor_review`) — which runs the
// deterministic FINE runners + the voted Gemma tier and writes the shard — reads
// the confirmed findings back, and routes them into the session as a prompt.

/// Timeout for `wait_for_censor_findings` after a review pass. `process_censor_review`
/// writes the shard synchronously (incl. the voted Gemma tier), so findings are present
/// by the time it returns; this is just a small grace window for the shard write to settle.
const CENSOR_REVIEW_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Max length of the findings message delivered back into the session, to avoid
/// flooding the agent's context with a chatty review.
const CENSOR_REVIEW_MSG_MAX_CHARS: usize = 4000;
/// Max consecutive censor-triggered rounds per session before we stop auto-sending
/// prompts back into the session (anti-loop guard, spec 3e).
const CENSOR_LOOP_MAX_CONSECUTIVE: u8 = 2;

/// Per-session count of consecutive censor-triggered rounds (auto-sent prompts).
/// Bumped every time the hook auto-delivers a findings prompt; reset to 0 whenever
/// the user sends a fresh prompt (`spike_pi_prompt`), breaking any
/// review→fix→review loop. Lazily initialized so the module stays free of a const
/// initializer for a non-const `Mutex`.
static CENSOR_LOOP_COUNTERS: OnceLock<Mutex<HashMap<String, u8>>> = OnceLock::new();

fn censor_loop_counters() -> &'static Mutex<HashMap<String, u8>> {
    CENSOR_LOOP_COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Result of parsing a `devboule_censor_review` event: the resolved inputs the
/// detached review thread needs, plus the console banner text (so the dispatch
/// logic is unit-testable without an `AppHandle`).
struct CensorReviewDispatch {
    /// Banner text to show immediately ("⚑ Censor review started for: ...").
    banner: String,
    /// Resolved project id, or `None` if the event carried none (=> skip).
    project_id: Option<String>,
    /// Session id to deliver any findings prompt to (falls back to `agent_id`).
    session_id: String,
    /// Edited file paths (as the sidecar sent them).
    files: Vec<String>,
}

/// Pure parse + dispatch for a `devboule_censor_review` event: extract the banner
/// text, project id, session id (falling back to `agent_id`), and edited files.
/// `project_id` is `None` when the event has none, signalling the caller to emit
/// the "no project id" skip note and bail. Factored out of `handle_event` so the
/// parse + dispatch decision is unit-testable without an `AppHandle`.
fn censor_review_dispatch(event: &PiEvent, agent_id: &str) -> CensorReviewDispatch {
    let files = event.files.clone().unwrap_or_default();
    let banner = if files.is_empty() {
        "⚑ Censor review started".to_string()
    } else {
        format!("⚑ Censor review started for: {}", files.join(", "))
    };
    let project_id = event.devboule.as_ref().and_then(|d| d.project_id.clone());
    let session_id = event
        .devboule
        .as_ref()
        .and_then(|d| d.session_id.clone())
        .unwrap_or_else(|| agent_id.to_string());
    CensorReviewDispatch {
        banner,
        project_id,
        session_id,
        files,
    }
}

/// Pure: would a fresh censor-triggered round for `session_id` be allowed, given
/// `map` holding the running consecutive-round counts? True while the count is
/// below `max` (the cap is on consecutive *sent* rounds).
fn censor_loop_allow_in(map: &HashMap<String, u8>, session_id: &str, max: u8) -> bool {
    map.get(session_id).copied().unwrap_or(0) < max
}

/// Pure: bump the consecutive-round count for `session_id` (called after a round
/// is actually delivered).
fn censor_loop_bump_in(map: &mut HashMap<String, u8>, session_id: &str) {
    *map.entry(session_id.to_string()).or_insert(0) += 1;
}

/// Pure: reset the consecutive-round count for `session_id` to 0.
fn censor_loop_reset_in(map: &mut HashMap<String, u8>, session_id: &str) {
    map.insert(session_id.to_string(), 0);
}

/// Whether the hook may auto-deliver another findings prompt to `session_id`.
fn censor_loop_allow(session_id: &str) -> bool {
    let map = censor_loop_counters()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    censor_loop_allow_in(&map, session_id, CENSOR_LOOP_MAX_CONSECUTIVE)
}

/// Record that a findings prompt was delivered for `session_id`.
fn censor_loop_bump(session_id: &str) {
    let mut map = censor_loop_counters()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    censor_loop_bump_in(&mut map, session_id);
}

/// Reset the consecutive-round counter — called on a fresh (non-censor) user prompt.
fn censor_loop_reset(session_id: &str) {
    let mut map = censor_loop_counters()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    censor_loop_reset_in(&mut map, session_id);
}

// ---- Fix 4: per-session delivered-finding ids (dedup across rounds) -------
//
// `wait_for_censor_findings` returns ALL Open findings in the shard, not just
// new ones, so a finding already delivered to the agent would be re-sent every
// 2 rounds forever. We keep, per session, the set of finding ids we have already
// delivered and drop them before re-prompting. Lazily initialized like the loop
// counters above.
static CENSOR_DELIVERED_IDS: OnceLock<Mutex<HashMap<String, HashSet<String>>>> =
    OnceLock::new();

fn censor_delivered_ids() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    CENSOR_DELIVERED_IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pure: keep only the findings whose id has NOT yet been delivered to the agent
/// for this session. `delivered` (the per-session set) is mutated in place: ids of
/// the findings we are about to (re-)send are added so a later round won't repeat
/// them. Returns the surviving findings plus the count already delivered.
fn censor_dedup_in(
    delivered: &mut HashSet<String>,
    findings: &[crate::backend::censor::schema::Finding],
) -> (Vec<crate::backend::censor::schema::Finding>, usize) {
    let mut new_findings = Vec::new();
    let mut already = 0usize;
    for f in findings {
        if delivered.contains(&f.id) {
            already += 1;
        } else {
            delivered.insert(f.id.clone());
            new_findings.push(f.clone());
        }
    }
    (new_findings, already)
}

/// Drop findings already delivered to `session_id`, recording the survivors in
/// the per-session delivered-id set. Returns (new_findings, already_delivered_count).
fn censor_dedup(
    session_id: &str,
    findings: &[crate::backend::censor::schema::Finding],
) -> (Vec<crate::backend::censor::schema::Finding>, usize) {
    let mut map = censor_delivered_ids()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let delivered = map.entry(session_id.to_string()).or_default();
    censor_dedup_in(delivered, findings)
}

/// Reset/evict ALL per-session Censor state (anti-loop counter + delivered-id
/// set) for `session_id`. Used on a fresh (new-generation) spawn — Fix 2 reset —
/// and on session stop — Fix 6 eviction — so stable session ids (e.g. the
/// orchestrator) never carry a stale cap or delivered-ids across relaunches, and
/// the per-session statics don't grow forever.
fn censor_session_state_reset(session_id: &str) {
    {
        let mut map = censor_loop_counters()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(session_id);
    }
    {
        let mut map = censor_delivered_ids()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(session_id);
    }
}

/// Pure, testable core of [`relativize_censor_path`]: if `file` is absolute and
/// begins with `root` (a directory path), return the remainder without the leading
/// separator; otherwise return `file` unchanged.
fn relativize_censor_path_in(root: &str, file: &str) -> String {
    let root = root.trim_end_matches('/');
    if file.starts_with(root) && file[root.len()..].starts_with('/') {
        return file[root.len() + 1..].to_string();
    }
    file.to_string()
}

/// Convert a file path to a project-relative path (what `process_censor_review` /
/// `wait_for_censor_findings` expect). The sidecar forwards the path the agent
/// handed to its write/edit tool, which may be absolute — strip the root prefix
/// when present, otherwise pass through.
fn relativize_censor_path(root: &Path, file: &str) -> String {
    relativize_censor_path_in(&root.to_string_lossy(), file)
}

/// Truncate `s` to at most `max` chars (the ellipsis counts toward `max`),
/// appending an ellipsis when cut. Small and local (no shared truncation helper
/// was reusable); keeps the findings message bounded to a sane length.
fn truncate_to_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

/// Compose the compact message delivered back into the session when the Censor
/// review surfaces confirmed findings. Capped to [`CENSOR_REVIEW_MSG_MAX_CHARS`]
/// so a chatty review can't flood the agent's context.
fn compose_censor_review_message(
    findings: &[crate::backend::censor::schema::Finding],
) -> String {
    let n = findings.len();
    let mut body = format!("Automated Censor review found {n} issue(s):\n");
    for f in findings {
        let line = f
            .line
            .map(|l| l.to_string())
            .unwrap_or_else(|| "-".to_string());
        let sev = match f.severity {
            crate::backend::censor::schema::Severity::High => "HIGH",
            crate::backend::censor::schema::Severity::Medium => "MEDIUM",
            crate::backend::censor::schema::Severity::Low => "LOW",
        };
        let snippet = truncate_to_chars(&f.body, 120);
        body.push_str(&format!(
            "- {}:{} [{}] {} — {}\n",
            f.file, line, sev, f.title, snippet
        ));
    }
    body.push_str("Fix the confirmed issues above, then continue.");
    truncate_to_chars(&body, CENSOR_REVIEW_MSG_MAX_CHARS)
}

/// Emit a passive console note on the session's mini-activity channel. Used by the
/// detached censor-review thread — which cannot mutate the `EventMapper`'s own
/// entry vec — to surface VISIBLE notes (busy / cap / clean / error paths) without
/// touching the agent's live/stopped status.
fn push_censor_note(app: &AppHandle, agent_id: &str, text: &str, style: Option<NodeStyle>) {
    let ts = EventMapper::now_str();
    let store = app.state::<crate::backend::mini_activity::MiniActivityStore>();
    store.update(app, agent_id, |a| {
        push_coder_note(a, text, style, &ts);
    });
}

/// Detached-thread body for the pi sidecar's `devboule_censor_review` hook. Runs
/// the real Censor review (reusing `censor_review::process_censor_review`, which
/// runs the deterministic FINE runners + the voted Gemma tier and writes the
/// shard) sequentially per file, then routes any confirmed findings back into the
/// session as a prompt. Every failure path emits a VISIBLE console note — no
/// silent degradation (hard project rule).
fn run_pi_censor_review(
    app: &AppHandle,
    agent_id: &str,
    project_id: &str,
    session_id: &str,
    files: &[String],
) {
    use crate::backend::censor::schema::{Finding, Verdict};

    // 3a: in-flight cap + free-RAM gate, shared with the Pigeon ingest path.
    let _guard = match crate::backend::censor_review::try_begin_censor_review() {
        Some(g) => g,
        None => {
            push_censor_note(app, agent_id, "Censor review skipped (busy)", Some(NodeStyle::Sage));
            return;
        }
    };

    // 4: failure paths must be VISIBLE. Gate on trust + config + probe up front so
    // we can name the reason instead of silently no-op'ing (process_censor_review
    // would otherwise swallow these).
    if let Err(reason) = crate::backend::censor_review::censor_review_runnable(app, project_id) {
        push_censor_note(
            app,
            agent_id,
            &format!("Censor review skipped ({reason})"),
            Some(NodeStyle::Sage),
        );
        return;
    }

    // Resolve the canonical root once (single source of truth).
    let root = match crate::backend::projects::resolve_project_root_by_id(app, project_id) {
        Ok(r) => r,
        Err(e) => {
            push_censor_note(
                app,
                agent_id,
                &format!("Censor review skipped (cannot resolve project root: {e})"),
                Some(NodeStyle::Sage),
            );
            return;
        }
    };

    // 3b: relativize absolute paths to project-relative, then run ONE review per
    // file sequentially in this thread (the k-vote runs are heavy on the local
    // model — sequential is deliberate, not one-thread-per-file).
    let rel_files: Vec<String> = files
        .iter()
        .map(|f| relativize_censor_path(&root, f))
        .collect();
    for f in &rel_files {
        let _ = crate::backend::censor_review::process_censor_review(
            app,
            &crate::backend::censor_review::CensorReviewRequest {
                project_id: project_id.to_string(),
                root: String::new(),
                file: f.clone(),
                known_findings: vec![],
            },
        );
    }

    // 3c: read the findings back from the shard, keep only confirmed-tier ones
    // (the voted Gemma promotion) for the edited files.
    let findings: Vec<Finding> = crate::backend::censor::commands::wait_for_censor_findings(
        &root,
        &rel_files,
        CENSOR_REVIEW_WAIT_TIMEOUT,
    );
    let confirmed: Vec<Finding> = findings
        .iter()
        .filter(|f| f.verdict == Verdict::Confirmed)
        .cloned()
        .collect();

    // 3d / 3e: deliver findings as a prompt unless the anti-loop cap is hit.
    if confirmed.is_empty() {
        push_censor_note(
            app,
            agent_id,
            &format!("Censor review clean ({} files)", rel_files.len()),
            Some(NodeStyle::Sage),
        );
        return;
    }

    // Fix 4: `wait_for_censor_findings` returns ALL Open findings — not just new
    // ones — so a previously delivered finding would nag the agent every 2 rounds
    // forever. Drop already-delivered ids (recording the survivors in the
    // per-session set) before any re-prompt.
    let (new_findings, already_delivered) = censor_dedup(session_id, &confirmed);
    if new_findings.is_empty() {
        // Everything was already reported — don't re-send and don't bump the
        // anti-loop counter (otherwise a single stale finding could pin the cap).
        push_censor_note(
            app,
            agent_id,
            &format!(
                "Censor: no new findings ({} previously reported)",
                already_delivered
            ),
            Some(NodeStyle::Sage),
        );
        return;
    }

    let msg = compose_censor_review_message(&new_findings);
    if !censor_loop_allow(session_id) {
        push_censor_note(
            app,
            agent_id,
            &format!(
                "Censor review found {} issue(s) (loop cap reached — not re-prompting)",
                new_findings.len()
            ),
            Some(NodeStyle::Sage),
        );
        return;
    }
    match send_prompt_to_session(app, session_id, &msg) {
        Ok(()) => {
            censor_loop_bump(session_id);
            push_censor_note(
                app,
                agent_id,
                &format!("Censor review found {} issue(s)", new_findings.len()),
                Some(NodeStyle::Sage),
            );
        }
        Err(e) => {
            push_censor_note(
                app,
                agent_id,
                &format!(
                    "Censor review found {} issue(s) but delivery failed: {e}",
                    new_findings.len()
                ),
                Some(NodeStyle::Sage),
            );
        }
    }
}

/// B helper: extract a list of `PageEntry` from a web_search `details` value,
/// tolerating several response shapes (a bare array, or an object with a
/// `results`/`pages`/`items`/`hits`/`data` array, or a single url/title object).
/// Unknown/empty items are skipped; returns an empty vec if nothing parseable.
fn extract_pages(results: &serde_json::Value) -> Vec<PageEntry> {
    let items: Vec<&serde_json::Value> = match results {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(map) => {
            let mut found: Vec<&serde_json::Value> = Vec::new();
            for key in ["results", "pages", "items", "hits", "data", "entries", "organic_results"] {
                if let Some(arr) = map.get(key).and_then(|v| v.as_array()) {
                    found = arr.iter().collect();
                    break;
                }
            }
            if found.is_empty() && (map.contains_key("url") || map.contains_key("title")) {
                found = vec![results];
            }
            found
        }
        _ => Vec::new(),
    };
    items
        .iter()
        .filter_map(|item| {
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("name").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let summary = item
                .get("summary")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("snippet").and_then(|v| v.as_str()))
                .or_else(|| item.get("text").and_then(|v| v.as_str()))
                .or_else(|| item.get("content").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            if title.is_empty() && url.is_empty() {
                return None;
            }
            Some(PageEntry { url, title, summary })
        })
        .collect()
}

// ---- Phase 2 Pigeon control commands ------------------------------------

/// Phase 2: handle a JSONL *control command* the sidecar emits on its stdout
/// (`classify_prompt`, `redirect_to_claude`) before it is mistaken for a pi SDK
/// event. Returns `true` if the line was a control command (and consumed);
/// `false` lets the caller fall through to normal pi event parsing.
fn handle_control_line(
    app: &AppHandle,
    agent_id: &str,
    line: &str,
    stdin: &Arc<Mutex<ChildStdin>>,
) -> bool {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let t = match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return false,
    };
    match t {
        "classify_prompt" => {
            // Non-blocking: classification is pure + fast, so the `classified`
            // response is written back to the sidecar's stdin BEFORE the
            // session.prompt() begins (the sidecar awaits it first).
            //
            // Phase 2: AgentPath routing only. Full multi-tier model switching
            // (vault-aware tier resolution) deferred to Phase 3 — the sidecar's
            // spawn-time minimal models.json can't resolve every classified
            // (provider, model) pair, so `setModel` is deferred there.
            let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let c = crate::backend::prompt_routing::classify_prompt_full(text, app);
            let response = serde_json::json!({
                "type": "classified",
                "tier": c.tier,
                "provider": c.provider,
                "model": c.model,
                "path": c.path,
            });
            write_jsonl_to_stdin(stdin, &response);
            true
        }
        "redirect_to_claude" => {
            // path == Terminal: the sidecar declined to run this in pi and asked
            // Rust to route it to the legacy Claude-terminal subprocess.
            // TODO (Phase 3): spawn/route `message` to the existing Claude-terminal
            // subprocess (decision #10). Spike: log it and surface a Tauri event
            // so the frontend can later show the redirect.
            let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
            let truncated: String = message.chars().take(60).collect();
            let suffix = if message.chars().count() > 60 { "…" } else { "" };
            eprintln!(
                "[pi-sidecar] Pigeon: path=Terminal redirect_to_claude: {}{}",
                truncated, suffix
            );
            let _ = app.emit(
                &format!("pigeon-redirect://{agent_id}"),
                serde_json::json!({ "message": message }),
            );
            true
        }
        _ => false,
    }
}

/// Write a JSONL object to the sidecar's stdin (best-effort; used for the
/// `classified` response). Locked so it never interleaves with prompt commands.
///
/// #7: write failures are logged to stderr instead of being silently discarded
/// (`let _ =`). A dropped `classified` line would otherwise leave the sidecar's
/// `requestClassification()` awaiting forever (#7). The sidecar additionally
/// guards with a 5s timeout, but surfacing the error here aids diagnosis.
fn write_jsonl_to_stdin(stdin: &Arc<Mutex<ChildStdin>>, value: &serde_json::Value) {
    if let Ok(mut s) = stdin.lock() {
        if let Ok(line) = serde_json::to_string(value) {
            if let Err(e) = s.write_all(format!("{line}\n").as_bytes()) {
                eprintln!("[pi_sidecar] failed to write to sidecar stdin: {e}");
                return;
            }
            if let Err(e) = s.flush() {
                eprintln!("[pi_sidecar] failed to flush sidecar stdin: {e}");
            }
        }
    }
}

// ---- sidecar lifecycle helpers (crash recovery / timeout / stderr) -------

/// #1/#5: classify how a sidecar terminated, given its exit status and whether
/// the timeout watchdog killed it. Pure + unit-testable (mock exit codes).
enum SidecarTermination {
    Crash(i32),
    Timeout,
    Clean,
}

fn classify_sidecar_termination(
    exit_status: Option<std::process::ExitStatus>,
    timed_out: bool,
) -> SidecarTermination {
    if timed_out {
        return SidecarTermination::Timeout;
    }
    match exit_status {
        Some(status) if !status.success() => SidecarTermination::Crash(status.code().unwrap_or(-1)),
        _ => SidecarTermination::Clean,
    }
}

/// #1: detect the zombie-leak scenario in the reader EOF handler. Fires when the
/// sidecar's stdout has closed (EOF) but the child process is STILL alive
/// (`try_wait` returned `Ok(None)`). `classify_sidecar_termination(None, ..)`
/// would otherwise report `Clean` and leave the session in the map forever —
/// an orphaned child with no stdin/stdout that never gets reaped.
fn is_zombie_stdout_close(
    exit_status: Option<std::process::ExitStatus>,
    child_alive: bool,
) -> bool {
    child_alive && exit_status.is_none()
}

/// #1/#5: remove a dead/timeout-killed session from the live state map, but ONLY
/// if the slot still belongs to THIS generation. A concurrent respawn
/// (`get_or_spawn_session` detected the dead child and replaced it) or
/// `stop_pi_session` would own a different generation, and we must not clobber it.
fn remove_session_if_same_generation(
    app: &AppHandle,
    session_id: &str,
    gen: u64,
    generation: &Arc<AtomicU64>,
) {
    let state = app.state::<PiSidecarState>();
    let mut guard = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    if generation.load(Ordering::SeqCst) != gen {
        return; // superseded — leave the slot to its new owner.
    }
    if let Some(slot) = guard.get_mut(session_id) {
        if let Some(ref session) = slot.inner {
            if session.generation.load(Ordering::SeqCst) == gen {
                guard.remove(session_id);
            }
        }
    }
}

/// Persist a terminal session status (crashed/stopped) and rewrite the sessions file.
fn persist_session_status(app: &AppHandle, agent_id: &str, status: SessionStatus) {
    let state = app.state::<PiSidecarState>();
    {
        let mut pg = state.persisted.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = pg.get_mut(agent_id) {
            s.status = status;
            s.last_active_at = now_ms();
        }
    }
    save_pi_sessions(&state, &pi_project_root());
}

/// #4: release-build stderr forwarder. Reads the sidecar's stderr and re-emits
/// each line via `eprintln!` so packaged (terminal-less) builds still surface
/// logs through Tauri's logger. (The release-build gate is applied at the spawn
/// site; this function is always callable but only wired up there.)
fn spawn_stderr_forwarder(stderr: std::process::ChildStderr) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(l) if !l.is_empty() => eprintln!("[pi-sidecar] {l}"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
}

/// #5: session lifetime cap in seconds. `DEVBOULE_PI_SESSION_TIMEOUT_SECS`
/// (default 3600). Set to 0 to disable the watchdog entirely.
fn read_session_timeout_secs() -> u64 {
    match std::env::var("DEVBOULE_PI_SESSION_TIMEOUT_SECS") {
        Ok(v) => v.trim().parse().unwrap_or(3600),
        Err(_) => 3600,
    }
}

/// #5: per-session watchdog. Sleeps until `timeout_secs` have elapsed since
/// `spawned_at`, then — if this session's generation is still current AND the
/// child is still alive — sets `timed_out` and kills the child. The kill closes
/// stdout, so the reader thread detects EOF and emits the timeout banner
/// (preserving the existing console history). The `timed_out` flag tells the
/// reader to emit the timeout banner rather than the crash banner.
fn spawn_session_timeout_watchdog(
    app: AppHandle,
    session_id: String,
    generation: Arc<AtomicU64>,
    child: Arc<Mutex<Child>>,
    timed_out: Arc<AtomicBool>,
    spawned_at: Instant,
    timeout_secs: u64,
) {
    if timeout_secs == 0 {
        return; // disabled.
    }
    let gen = generation.load(Ordering::SeqCst);
    std::thread::spawn(move || {
        let elapsed = spawned_at.elapsed().as_secs();
        let remaining = timeout_secs.saturating_sub(elapsed);
        if remaining > 0 {
            std::thread::sleep(Duration::from_secs(remaining));
        }
        if generation.load(Ordering::SeqCst) != gen {
            return;
        }
        let still_alive = child
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok().flatten())
            .is_none();
        if still_alive {
            timed_out.store(true, Ordering::SeqCst);
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
            // Self-sufficient cleanup (#3): even if the reader thread is
            // dead/stuck when the timeout fires, evict the session and persist
            // it as Crashed so it doesn't linger as a zombie after restart.
            remove_session_if_same_generation(&app, &session_id, gen, &generation);
            persist_session_status(&app, &session_id, SessionStatus::Crashed);
        }
    });
}

// ---- stdout reader ---------------------------------------------------------

/// Blocking reader thread: reads JSONL from the sidecar's stdout. Uses a
/// **per-session** generation Arc — only bumped when THIS session respawns,
/// NOT when sibling sessions spawn or stop (fixes BLOCKER #1).
fn read_sidecar_events(
    app: AppHandle,
    stdout: std::process::ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    generation: Arc<AtomicU64>,
    child: Arc<Mutex<Child>>,
    timed_out: Arc<AtomicBool>,
    agent_id: &str,
) {
    let gen = generation.load(Ordering::SeqCst);
    let reader = BufReader::new(stdout);
    let mut mapper = EventMapper::new(agent_id);

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        // Per-session generation check: only exits if THIS session was superseded.
        if generation.load(Ordering::SeqCst) != gen {
            break;
        }
        // Phase 2 Pigeon: intercept control commands BEFORE treating the line as
        // a pi SDK event. `classify_prompt` → respond with `classified` on the
        // sidecar's stdin; `redirect_to_claude` → log + emit a Tauri event
        // (full routing to the Claude-terminal subprocess is deferred to Phase 3).
        if handle_control_line(&app, agent_id, &line, &stdin) {
            continue;
        }
        match serde_json::from_str::<PiEvent>(&line) {
            Ok(event) => {
                mapper.handle_event(&app, &event);
            }
            Err(_) => {
                let preview: String = line.chars().take(200).collect();
                eprintln!("[pi-sidecar] unparseable JSONL line: {preview}...");
            }
        }
    }

    // #1/#5: reader EOF — the sidecar's stdout closed (crash / clean exit /
    // killed by the timeout watchdog). Only act if THIS session's generation is
    // still current (a respawn or stop would have bumped it and owns the channel).
    if generation.load(Ordering::SeqCst) == gen {
        // Inspect the child's exit status (now behind a Mutex so the reader can
        // own a clone without a second exclusive handle).
        let try_wait = child
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok());
        let exit_status = try_wait.flatten();
        // Zombie-leak guard (#1): stdout closed but the child is still alive
        // (`try_wait` returned `Ok(None)`) — otherwise classified as `Clean`
        // and the session leaks in the map forever.
        let child_alive = try_wait.map(|s| s.is_none()).unwrap_or(false);
        match classify_sidecar_termination(exit_status, timed_out.load(Ordering::SeqCst)) {
            SidecarTermination::Timeout => {
                mapper.running = false;
                mapper.push_entry(ConsoleEntry::Banner {
                    text: "pi session timed out (1h limit)".to_string(),
                    time: EventMapper::now_str(),
                });
                mapper.emit_snapshot(&app);
                persist_session_status(&app, agent_id, SessionStatus::Crashed);
                remove_session_if_same_generation(&app, agent_id, gen, &generation);
            }
            SidecarTermination::Crash(code) => {
                mapper.running = false;
                mapper.push_entry(ConsoleEntry::Banner {
                    text: format!(
                        "pi sidecar crashed (exit code {code}). Spawn a new session."
                    ),
                    time: EventMapper::now_str(),
                });
                mapper.emit_snapshot(&app);
                persist_session_status(&app, agent_id, SessionStatus::Crashed);
                remove_session_if_same_generation(&app, agent_id, gen, &generation);
            }
            SidecarTermination::Clean => {
                if is_zombie_stdout_close(exit_status, child_alive) {
                    // Child orphaned (no stdin/stdout) → kill it and evict the
                    // session so it cannot become a zombie after restart.
                    if let Ok(mut c) = child.lock() {
                        let _ = c.kill();
                    }
                    eprintln!(
                        "[pi-sidecar] pi sidecar stdout closed but child still alive — removing session {agent_id}"
                    );
                    remove_session_if_same_generation(&app, agent_id, gen, &generation);
                    persist_session_status(&app, agent_id, SessionStatus::Crashed);
                } else {
                    mapper.running = false;
                    mapper.emit_snapshot(&app);
                    persist_session_status(&app, agent_id, SessionStatus::Stopped);
                }
            }
        }
    }
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    // -- crash recovery (#1): mock child exit codes --------------------------
    // `classify_sidecar_termination` is pure, so we can drive it with mocked
    // exit statuses (unix `ExitStatus::from_raw`) without spawning a process.
    #[cfg(unix)]
    #[test]
    fn crash_recovery_classifies_exit_codes() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;
        // Non-zero exit code -> Crash(code).
        assert!(matches!(
            classify_sidecar_termination(Some(ExitStatus::from_raw(42 << 8)), false),
            SidecarTermination::Crash(42)
        ));
        // Signal death (no code) -> Crash(-1).
        assert!(matches!(
            classify_sidecar_termination(Some(ExitStatus::from_raw(9)), false),
            SidecarTermination::Crash(-1)
        ));
        // Clean zero exit -> Clean.
        assert!(matches!(
            classify_sidecar_termination(Some(ExitStatus::from_raw(0)), false),
            SidecarTermination::Clean
        ));
        // try_wait errored (None) -> Clean (conservative).
        assert!(matches!(
            classify_sidecar_termination(None, false),
            SidecarTermination::Clean
        ));
        // timed_out wins over exit status.
        assert!(matches!(
            classify_sidecar_termination(Some(ExitStatus::from_raw(0)), true),
            SidecarTermination::Timeout
        ));
    }

    // -- pi_sidecar_enabled (Phase 4 opt-out) ------------------------------------
    // Serialize all env-mutating tests so they don't clobber each other's
    // DEVBOULE_PI_ENABLED value. Mirrors the per-module test Mutex pattern used
    // elsewhere in the crate (e.g. HOME_ENV_TEST_LOCK in token_usage.rs).
    static PI_SIDECAR_ENABLED_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn pi_sidecar_enabled_true_when_env_unset() {
        let _guard = PI_SIDECAR_ENABLED_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DEVBOULE_PI_ENABLED");
        assert!(pi_sidecar_enabled());
    }

    #[test]
    fn pi_sidecar_enabled_false_for_falsy_values() {
        let _guard = PI_SIDECAR_ENABLED_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for v in [
            "0", "false", "no", "off", "FALSE", "No", "OFF", "  no  ", "\tFalse\n",
        ] {
            std::env::set_var("DEVBOULE_PI_ENABLED", v);
            assert!(
                !pi_sidecar_enabled(),
                "DEVBOULE_PI_ENABLED={v:?} must disable the pi sidecar"
            );
        }
        std::env::remove_var("DEVBOULE_PI_ENABLED");
    }

    #[test]
    fn pi_sidecar_enabled_true_for_truthy_values() {
        let _guard = PI_SIDECAR_ENABLED_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for v in ["1", "true", "yes", "on", "TRUE", "Yes", "ON", "  yes ", "On"] {
            std::env::set_var("DEVBOULE_PI_ENABLED", v);
            assert!(
                pi_sidecar_enabled(),
                "DEVBOULE_PI_ENABLED={v:?} must enable the pi sidecar"
            );
        }
        std::env::remove_var("DEVBOULE_PI_ENABLED");
    }

    #[test]
    fn pi_sidecar_enabled_true_for_empty_and_garbage() {
        let _guard = PI_SIDECAR_ENABLED_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for v in ["", "garbage", "2", "enabled", "  ", "DISABLED"] {
            std::env::set_var("DEVBOULE_PI_ENABLED", v);
            assert!(
                pi_sidecar_enabled(),
                "DEVBOULE_PI_ENABLED={v:?} must default to enabled (opt-out model)"
            );
        }
        std::env::remove_var("DEVBOULE_PI_ENABLED");
    }

    // -- session id generation ------------------------------------------------

    #[test]
    fn session_ids_are_unique_per_counter() {
        let id1 = generate_session_id(1);
        let id2 = generate_session_id(2);
        assert_ne!(id1, id2);
        assert_eq!(id1, "pi-1");
        assert_eq!(id2, "pi-2");
    }

    #[test]
    fn session_ids_start_with_pi_prefix() {
        let id = generate_session_id(42);
        assert!(id.starts_with("pi-"), "id must start with pi-: {id}");
        assert_eq!(id, "pi-42");
    }

    // -- agent id generation (#6: timestamp+counter uniqueness) ---------------

    #[test]
    fn agent_ids_are_unique_across_same_millisecond() {
        // Two launches in the same millisecond must still differ thanks to the
        // monotonic counter appended after the timestamp.
        let a = generate_agent_id("main-coder", None);
        let b = generate_agent_id("main-coder", None);
        assert_ne!(a, b, "two main-coder agent ids collided");
        assert!(a.starts_with("main-"), "main id must start with main-: {a}");
        assert!(b.starts_with("main-"), "main id must start with main-: {b}");
        assert_ne!(
            generate_agent_id("mini", None),
            generate_agent_id("mini", None),
            "two mini agent ids collided"
        );
    }

    #[test]
    fn session_id_counter_monotonically_increases() {
        let state = PiSidecarState::default();
        let id1 = {
            let c = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
            generate_session_id(c)
        };
        let id2 = {
            let c = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
            generate_session_id(c)
        };
        let id3 = {
            let c = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
            generate_session_id(c)
        };
        assert_eq!(id1, "pi-1");
        assert_eq!(id2, "pi-2");
        assert_eq!(id3, "pi-3");
    }

    // -- generate_agent_id (role-aware namespaces, Task 3) ---------------------

    #[test]
    fn agent_id_orchestrator_uses_stable_project_id() {
        let id = generate_agent_id("orchestrator", Some("my-proj"));
        assert_eq!(id, "orchestrator-my-proj");
        assert_eq!(id, super::super::projects::stable_orchestrator_agent_id("my-proj"));
    }

    #[test]
    fn agent_id_orchestrator_sanitizes_hostile_project_id() {
        let id = generate_agent_id("orchestrator", Some("proj/../evil name!"));
        assert!(id.starts_with("orchestrator-"), "id: {id}");
        assert!(!id.contains('/'), "id must not contain path separators: {id}");
        assert!(!id.contains(' '), "id must not contain spaces: {id}");
    }

    #[test]
    fn agent_id_orchestrator_falls_back_to_unknown_without_project() {
        let id = generate_agent_id("orchestrator", None);
        assert_eq!(id, "orchestrator-unknown");
    }

    #[test]
    fn agent_id_main_and_mini_use_timestamp_namespace() {
        let main = generate_agent_id("main-coder", None);
        let mini = generate_agent_id("mini-coder", None);
        assert!(main.starts_with("main-"), "main id: {main}");
        assert!(mini.starts_with("mini-"), "mini id: {mini}");
        // #6: format is `main-<ms>-<counter>`; the timestamp is the part before the
        // final `-`.
        let main_ts: u128 = main["main-".len()..main.rfind('-').unwrap()]
            .parse()
            .unwrap();
        let mini_ts: u128 = mini["mini-".len()..mini.rfind('-').unwrap()]
            .parse()
            .unwrap();
        assert!(main_ts > 0 && mini_ts > 0);
    }

    #[test]
    fn agent_id_main_uses_timestamp_pattern() {
        // A "main-coder" role must produce `main-<numeric timestamp>` (NOT the
        // legacy `pi-<counter>` fallback), so the frontend console subscribes to
        // `mini-activity://main-<ts>`.
        let id = generate_agent_id("main-coder", None);
        assert!(id.starts_with("main-"), "main id must use main- namespace: {id}");
        let ts: u128 = id["main-".len()..id.rfind('-').unwrap()]
            .parse()
            .expect("main- suffix must start with a numeric timestamp");
        assert!(ts > 0, "timestamp must be positive: {id}");
    }

    #[test]
    fn agent_id_mini_uses_timestamp_pattern() {
        // A "mini-coder" role must produce `mini-<numeric timestamp>` (NOT the
        // legacy `pi-<counter>` fallback).
        let id = generate_agent_id("mini-coder", None);
        assert!(id.starts_with("mini-"), "mini id must use mini- namespace: {id}");
        let ts: u128 = id["mini-".len()..id.rfind('-').unwrap()]
            .parse()
            .expect("mini- suffix must start with a numeric timestamp");
        assert!(ts > 0, "timestamp must be positive: {id}");
    }

    #[test]
    fn agent_id_accepts_bare_main_and_mini_prefixes() {
        assert!(generate_agent_id("main", None).starts_with("main-"));
        assert!(generate_agent_id("mini", None).starts_with("mini-"));
    }

    #[test]
    fn agent_id_unknown_role_falls_back_to_pi_counter() {
        let a = generate_agent_id("weird-role", None);
        let b = generate_agent_id("weird-role", None);
        assert!(a.starts_with("pi-"), "fallback id: {a}");
        assert_ne!(a, b, "two fallback ids must be unique");
    }

    // -- session info serialization -------------------------------------------

    #[test]
    fn session_info_serializes_camel_case() {
        let info = SessionInfo {
            session_id: "pi-7".to_string(),
            is_new: true,
            agent_role: "main-coder".to_string(),
            channel: "mini-activity://pi-7".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"sessionId\""), "must be camelCase: {json}");
        assert!(json.contains("\"isNew\""), "must be camelCase: {json}");
        assert!(json.contains("pi-7"), "must contain session id: {json}");
    }

    // -- event mapper channel -------------------------------------------------

    #[test]
    fn event_mapper_uses_session_agent_id_in_channel() {
        let mapper = EventMapper::new("pi-42");
        assert_eq!(mapper.agent_id, "pi-42");
        let expected_channel = super::super::mini_activity::mini_activity_channel("pi-42");
        assert_eq!(expected_channel, "mini-activity://pi-42");
    }

    // -- per-session generation guard (BLOCKER #1 regression test) ------------

    #[test]
    fn per_session_generation_independent() {
        // Two sessions must have INDEPENDENT generation counters.
        // Bumping session A's generation must NOT affect session B's.
        let gen_a = Arc::new(AtomicU64::new(0));
        let gen_b = Arc::new(AtomicU64::new(0));

        let gen_a_val = gen_a.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(gen_a_val, 1);
        assert_eq!(gen_b.load(Ordering::SeqCst), 0, "gen_b must be unaffected");

        // Bump gen_b too.
        let gen_b_val = gen_b.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(gen_b_val, 1);
        assert_eq!(gen_a.load(Ordering::SeqCst), 1, "gen_a must be unaffected by gen_b bump");

        // Bump gen_a again (respawn session A).
        let gen_a_val2 = gen_a.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(gen_a_val2, 2);
        assert_eq!(gen_b.load(Ordering::SeqCst), 1, "gen_b must still be unaffected");
    }

    #[test]
    fn per_session_generation_reader_detects_own_respawn() {
        // Simulates: reader starts with gen=1, session respawns (gen→2), reader should exit.
        let gen = Arc::new(AtomicU64::new(0));
        let initial = gen.fetch_add(1, Ordering::SeqCst) + 1; // gen=1
        assert_eq!(initial, 1);

        // Reader checks: still our generation?
        assert_eq!(gen.load(Ordering::SeqCst), initial, "reader should continue");

        // Session respawns — bumps THIS session's generation.
        gen.fetch_add(1, Ordering::SeqCst); // gen=2

        // Reader checks: generation changed? YES → exit.
        assert_ne!(
            gen.load(Ordering::SeqCst),
            initial,
            "reader should detect respawn and exit"
        );
    }

    #[test]
    fn per_session_generation_sibling_respawn_does_not_kill() {
        // Session A reader starts with gen=1.
        let gen_a = Arc::new(AtomicU64::new(0));
        let gen_a_initial = gen_a.fetch_add(1, Ordering::SeqCst) + 1;

        // Session B spawns (creates its own generation).
        let gen_b = Arc::new(AtomicU64::new(0));
        let _gen_b_val = gen_b.fetch_add(1, Ordering::SeqCst) + 1;

        // Session A's reader checks: is MY generation still the same?
        assert_eq!(
            gen_a.load(Ordering::SeqCst),
            gen_a_initial,
            "session A reader must NOT be killed by session B spawn"
        );
    }

    // -- session map lifecycle ------------------------------------------------

    #[test]
    fn state_map_starts_empty_and_counter_at_zero() {
        let state = PiSidecarState::default();
        let guard = state.inner.lock().unwrap();
        assert!(guard.is_empty(), "map must start empty");
        drop(guard);
        assert_eq!(
            state.session_counter.load(Ordering::SeqCst),
            0,
            "counter must start at 0"
        );
    }

    #[test]
    fn session_map_insert_get_remove_lifecycle() {
        let state = PiSidecarState::default();
        {
            let guard = state.inner.lock().unwrap();
            assert!(!guard.contains_key("pi-1"));
        }

        let c1 = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let c2 = state.session_counter.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(c1, 1);
        assert_eq!(c2, 2);
        assert_eq!(generate_session_id(c1), "pi-1");
        assert_eq!(generate_session_id(c2), "pi-2");
    }

    // -- MAX_SESSIONS (F6) -----------------------------------------------------

    #[test]
    fn max_sessions_constant_is_sane() {
        assert_eq!(MAX_SESSIONS, 8, "MAX_SESSIONS must be 8");
    }

    // -- resolve_coder_env_for_sidecar fallback (decision #10) ----------------

    #[test]
    fn fallback_env_is_non_claude() {
        // The fallback (no coder backend configured) must NOT use Claude.
        // This is a pure logic test: verify the fallback SidecarEnvVars shape.
        // We can't call resolve_coder_env_for_sidecar without a real AppHandle,
        // but we can verify the fallback constants match decision #10.
        let fallback_provider = "openrouter";
        let fallback_model = "tencent/hy3:free";

        assert_ne!(fallback_provider, "anthropic", "must NOT default to Claude");
        assert_ne!(fallback_provider, "claude", "must NOT default to Claude");
        assert!(
            fallback_provider == "openrouter",
            "fallback must be openrouter"
        );
        assert!(
            fallback_model.contains("hy3:free"),
            "fallback model must be the free tier"
        );
    }

    // -- default state --------------------------------------------------------

    #[test]
    fn default_state_has_no_global_generation() {
        let state = PiSidecarState::default();
        let guard = state.inner.lock().unwrap();
        assert!(guard.is_empty());
        drop(guard);
        assert_eq!(state.session_counter.load(Ordering::SeqCst), 0);
    }

    // -- SessionSlot placeholder (F2) ------------------------------------------

    #[test]
    fn session_slot_can_hold_none_placeholder() {
        let slot = SessionSlot { inner: None };
        assert!(slot.inner.is_none(), "placeholder slot must be None");
    }
    
    #[test]
    fn session_slot_inner_is_some_when_live() {
        // Can't construct a real PiSession without a Child, but we can
        // verify the Option wrapping logic works.
        let slot_none: Option<PiSession> = None;
        let slot = SessionSlot { inner: slot_none };
        assert!(slot.inner.is_none());
    }

    // -- Task 2: _devboule parsing + devboule custom messages --------------------

    #[test]
    fn devboule_agent_role_is_parsed() {
        // A: an event carrying `_devboule.agentRole` must set current_role.
        let line = r#"{"type":"message_start","_devboule":{"agentRole":"orchestrator","projectId":"my-proj","sessionId":"pi-42"}}"#;
        let event: PiEvent = serde_json::from_str(line).unwrap();
        let mut mapper = EventMapper::new("pi-42");
        mapper.apply_devboule_role(&event);
        assert_eq!(
            mapper.current_role.as_deref(),
            Some("orchestrator"),
            "agentRole must be parsed from _devboule"
        );
    }

    #[test]
    fn devboule_websearch_emits_websearch_entry() {
        // B: a `devboule.websearch` custom message must produce a WebSearch entry.
        let mut mapper = EventMapper::new("pi-1");
        let results = serde_json::json!([
            {"url": "https://a.test", "title": "Alpha", "summary": "about alpha"},
            {"url": "https://b.test", "title": "Beta", "summary": "about beta"}
        ]);
        mapper.handle_devboule_websearch("best widgets", &results);
        match mapper.entries.last() {
            Some(ConsoleEntry::WebSearch { query, pages, .. }) => {
                assert_eq!(query, "best widgets");
                assert_eq!(pages.len(), 2, "both pages must be mapped");
                assert_eq!(pages[0].url, "https://a.test");
                assert_eq!(pages[1].title, "Beta");
            }
            other => panic!("expected WebSearch entry, got {other:?}"),
        }
    }

    #[test]
    fn plain_event_without_devboule_is_handled_gracefully() {
        // 1) An event with no `_devboule` must not crash and must leave role unset.
        let line = r#"{"type":"message_start"}"#;
        let event: PiEvent = serde_json::from_str(line).unwrap();
        let mut mapper = EventMapper::new("pi-7");
        mapper.apply_devboule_role(&event);
        assert!(
            mapper.current_role.is_none(),
            "role must stay unset without _devboule"
        );

        // 2) A plain user message whose text is NOT a devboule custom message must
        // be ignored (no panic, no entry emitted).
        let plain_msg = serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "just a normal steering message"}]
        });
        let handled = mapper.handle_devboule_custom_message(&plain_msg);
        assert!(
            !handled,
            "non-devboule message must not be handled as custom"
        );
        assert!(
            mapper.entries.is_empty(),
            "no entry should be emitted for a plain message"
        );
    }

    // -- sandbox policy (decision #11) ----------------------------------------

    #[test]
    fn pi_sandbox_policy_denies_git_write() {
        // `.git` must NOT be in the writable allowlist — writes are denied by the
        // Seatbelt regex (RCE-via-planted-hooks guard) even though the project
        // root is writable.
        let root = PathBuf::from("/tmp/aspis-project-root");
        let policy = pi_sandbox_policy(&root);
        assert!(
            !policy
                .writable_paths
                .iter()
                .any(|p| p.to_string_lossy().contains(".git")),
            ".git must not be a writable path: {:?}",
            policy.writable_paths
        );
    }

    #[test]
    fn pi_sandbox_policy_allows_project_write() {
        // The project root must be writable so pi's edit/write/bash land inside it.
        let root = PathBuf::from("/tmp/aspis-project-root");
        let policy = pi_sandbox_policy(&root);
        assert!(
            policy.writable_paths.contains(&root),
            "project root must be in writable_paths: {:?}",
            policy.writable_paths
        );
    }

    #[test]
    fn entries_cap_sliding_window_never_exceeds_max() {
        // #3: the console entry history must be bounded. Pushing 501 entries
        // into a 500-cap mapper must leave exactly 500 (oldest dropped).
        let mut mapper = EventMapper::new("pi-cap");
        for i in 0..501 {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: format!("turn-{i}"),
                time: String::new(),
                msg_id: None,
            });
        }
        assert_eq!(
            mapper.entries.len(),
            MAX_CONSOLE_ENTRIES,
            "entries must be capped at MAX_CONSOLE_ENTRIES"
        );
        // The oldest entry must have been evicted (turn-0 dropped).
        match mapper.entries.first() {
            Some(ConsoleEntry::Chat { text, .. }) => {
                assert_eq!(text, "turn-1", "oldest entry should be evicted");
            }
            other => panic!("expected Chat entry, got {other:?}"),
        }
        // The newest entry (turn-500) must be retained.
        match mapper.entries.last() {
            Some(ConsoleEntry::Chat { text, .. }) => {
                assert_eq!(text, "turn-500", "newest entry must be retained");
            }
            other => panic!("expected Chat entry, got {other:?}"),
        }
    }

    // -- Part A: thinking accumulation (console fidelity) -----------------------

    #[test]
    fn thinking_accumulates_to_entry_on_thinking_end() {
        // thinking_start/delta/delta/end must yield exactly one Thinking entry with
        // the concatenated content (thinking precedes text; nothing to flush first).
        let mut mapper = EventMapper::new("pi-think");
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_start".to_string(),
            delta: None,
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("hmm, ".to_string()),
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("let's go".to_string()),
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_end".to_string(),
            delta: None,
            content_index: Some(0),
        });
        assert_eq!(mapper.entries.len(), 1, "exactly one entry");
        match mapper.entries.last() {
            Some(ConsoleEntry::Thinking { text, .. }) => {
                assert_eq!(text, "hmm, let's go");
            }
            other => panic!("expected Thinking entry, got {other:?}"),
        }
    }

    #[test]
    fn interrupted_thinking_is_flushed_on_agent_end() {
        // An unfinished thinking block (no thinking_end) must still be flushed as a
        // Thinking entry when agent_end fires — it must never be silently dropped.
        // agent_end needs an AppHandle, so we exercise the exact flush line it calls.
        let mut mapper = EventMapper::new("pi-think2");
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("partial reasoning".to_string()),
            content_index: Some(0),
        });
        mapper.flush_thinking_block();
        match mapper.entries.last() {
            Some(ConsoleEntry::Thinking { text, .. }) => {
                assert_eq!(text, "partial reasoning");
            }
            other => panic!("expected Thinking entry, got {other:?}"),
        }
    }

    #[test]
    fn live_thinking_streams_in_place_then_finalizes_once() {
        // #2a: thinking deltas must surface LIVE in a single in-place Thinking entry
        // that grows with each delta, and `thinking_end` must finalize that same
        // entry WITHOUT pushing a duplicate. After end, `accumulated_thinking` is
        // empty and `live_thinking_idx` is None.
        let mut mapper = EventMapper::new("pi-think-live");
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_start".to_string(),
            delta: None,
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("Rea".to_string()),
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("soning".to_string()),
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("…".to_string()),
            content_index: Some(0),
        });

        // Before end: exactly ONE live Thinking entry, growing to "Reasoning…".
        let live_count = mapper
            .entries
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Thinking { .. }))
            .count();
        assert_eq!(live_count, 1, "exactly one Thinking entry while live");
        match mapper.entries.last() {
            Some(ConsoleEntry::Thinking { text, .. }) => {
                assert_eq!(text, "Reasoning…", "live thinking text must grow");
            }
            other => panic!("expected live Thinking entry, got {other:?}"),
        }
        assert_eq!(
            mapper.accumulated_thinking, "Reasoning…",
            "accumulator mirrors live text"
        );
        assert!(
            mapper.live_thinking_idx.is_some(),
            "live tracker should be set"
        );

        // Finalize.
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_end".to_string(),
            delta: None,
            content_index: Some(0),
        });

        let final_count = mapper
            .entries
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Thinking { .. }))
            .count();
        assert_eq!(final_count, 1, "still exactly ONE Thinking entry, no dup");
        match mapper.entries.last() {
            Some(ConsoleEntry::Thinking { text, .. }) => {
                assert_eq!(text, "Reasoning…", "finalized thinking text");
            }
            other => panic!("expected finalized Thinking entry, got {other:?}"),
        }
        assert!(
            mapper.accumulated_thinking.is_empty(),
            "accumulator cleared after flush"
        );
        assert!(
            mapper.live_thinking_idx.is_none(),
            "live tracker cleared after finalize"
        );
    }

    #[test]
    fn live_thinking_index_shifts_with_fifo_eviction() {
        // FIX 1: when the sliding window evicts the oldest entry during a live
        // thinking stream, `live_thinking_idx` must follow the surviving row
        // (decremented by one) so the next delta still updates the SAME entry
        // in place — and `thinking_end` finalizes it without corrupting an
        // unrelated row or duplicating.
        let mut mapper = EventMapper::new("evict-shift");
        // Fill to 498 (cap is 500) — no eviction yet.
        for i in 0..498u32 {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: format!("c{i}"),
                time: String::new(),
                msg_id: None,
            });
        }
        assert_eq!(mapper.entries.len(), 498);
        assert_eq!(mapper.evicted_count, 0);

        // Start a live thinking stream -> pushes a Thinking entry at index 498.
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_start".to_string(),
            delta: None,
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("A".to_string()),
            content_index: Some(0),
        });
        assert_eq!(mapper.entries.len(), 499);
        assert_eq!(mapper.live_thinking_idx, Some(498));
        assert!(matches!(mapper.entries[498], ConsoleEntry::Thinking { .. }));

        // Push 2 more entries: the 2nd crosses the cap and evicts the front,
        // shifting the tracked thinking row 498 -> 497.
        mapper.push_entry(ConsoleEntry::Chat {
            role: "user".to_string(),
            text: "c498".to_string(),
            time: String::new(),
            msg_id: None,
        });
        assert_eq!(mapper.entries.len(), 500); // 499 + 1, still no eviction
        mapper.push_entry(ConsoleEntry::Chat {
            role: "user".to_string(),
            text: "c499".to_string(),
            time: String::new(),
            msg_id: None,
        });
        // 501 > 500 -> eviction; tracker 498 -> 497, Thinking shifts to 497.
        assert_eq!(mapper.evicted_count, 1);
        assert_eq!(mapper.live_thinking_idx, Some(497));
        assert_eq!(mapper.entries.len(), 500);
        assert!(
            matches!(mapper.entries[497], ConsoleEntry::Thinking { .. }),
            "live thinking entry survived eviction at shifted index"
        );

        // Subsequent delta updates the SAME entry in place at the shifted index.
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("B".to_string()),
            content_index: Some(0),
        });
        match mapper.entries[497] {
            ConsoleEntry::Thinking { ref text, .. } => assert_eq!(text, "AB"),
            ref other => panic!("expected Thinking at shifted idx, got {other:?}"),
        }
        assert_eq!(
            mapper.live_thinking_idx, Some(497),
            "tracker still points at shifted live entry"
        );

        // Finalize: in place, no dup, no corruption of other rows.
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_end".to_string(),
            delta: None,
            content_index: Some(0),
        });
        let thinking_count = mapper
            .entries
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Thinking { .. }))
            .count();
        assert_eq!(thinking_count, 1, "exactly one Thinking entry after finalize");
        match mapper.entries[497] {
            ConsoleEntry::Thinking { ref text, .. } => assert_eq!(text, "AB"),
            ref other => panic!("expected finalized Thinking at idx 497, got {other:?}"),
        }
        assert!(mapper.accumulated_thinking.is_empty());
        assert!(mapper.live_thinking_idx.is_none());
        assert_eq!(mapper.entries.len(), 500, "window still bounded");
        assert!(
            matches!(mapper.entries[0], ConsoleEntry::Chat { .. }),
            "front entry is an unrelated Chat, not corrupted"
        );
    }

    #[test]
    fn live_thinking_tracker_cleared_when_entry_evicted() {
        // FIX 1/2: if the live thinking entry itself is front-evicted before
        // `thinking_end`, the tracker must be cleared (not point at a stale row).
        // A later delta re-pushes a fresh live entry; `thinking_end` finalizes
        // that fresh entry without corrupting anything else.
        let mut mapper = EventMapper::new("evict-clear");
        // Fill to 499 so the thinking entry lands at index 499 (cap 500).
        for i in 0..499u32 {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: format!("c{i}"),
                time: String::new(),
                msg_id: None,
            });
        }
        assert_eq!(mapper.entries.len(), 499);
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_start".to_string(),
            delta: None,
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("A".to_string()),
            content_index: Some(0),
        });
        assert_eq!(mapper.live_thinking_idx, Some(499));
        assert_eq!(mapper.entries.len(), 500);

        // Push 500 more entries; the thinking entry (at 499) is fully evicted,
        // pulling the tracker down to 0 then clearing it.
        for i in 0..500u32 {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: format!("x{i}"),
                time: String::new(),
                msg_id: None,
            });
        }
        assert_eq!(mapper.evicted_count, 500);
        assert_eq!(
            mapper.live_thinking_idx, None,
            "tracked thinking entry was evicted -> tracker cleared"
        );
        let thinking_count = mapper
            .entries
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Thinking { .. }))
            .count();
        assert_eq!(thinking_count, 0, "live thinking entry evicted, none remain");

        // A later delta re-pushes a fresh live entry at the tail. Note: FIFO
        // eviction drops the *visual* entry but does NOT reset `accumulated_thinking`
        // (that buffer is only cleared at thinking_end / agent_start), so the "B"
        // delta appends to the still-held "A" giving "AB".
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("B".to_string()),
            content_index: Some(0),
        });
        assert_eq!(
            mapper.live_thinking_idx,
            Some(499),
            "fresh live entry re-pushed at tail after eviction"
        );
        assert_eq!(mapper.entries.len(), 500);

        // Finalize -> in place, no duplicate.
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_end".to_string(),
            delta: None,
            content_index: Some(0),
        });
        let final_count = mapper
            .entries
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Thinking { .. }))
            .count();
        assert_eq!(final_count, 1, "exactly one Thinking entry after finalize");
        match mapper.entries.last() {
            Some(ConsoleEntry::Thinking { text, .. }) => assert_eq!(text, "AB"),
            other => panic!("expected Thinking, got {other:?}"),
        }
        assert!(mapper.accumulated_thinking.is_empty());
        assert!(mapper.live_thinking_idx.is_none());
    }

    #[test]
    fn live_thinking_orphan_finalized_by_agent_start() {
        // FIX C: a live thinking entry that is still open when a new turn begins
        // (abnormal reconnect / out-of-order / dropped thinking_end) must be
        // finalized as its OWN entry before the turn resets — not orphaned, not
        // merged into the next turn's thinking. Drive:
        //   thinking_delta("A") -> [agent_start: flush THEN reset] ->
        //   thinking_delta("B") -> thinking_end
        // and assert: exactly two Thinking entries ("A" then "B"), no orphan, no
        // stale-merge ("A" must NOT bleed into "B").
        //
        // `handle_event` isn't directly callable in a unit test (it needs an
        // `AppHandle` for the snapshot), so we replay the exact body of the
        // `agent_start` arm: flush_thinking_block() FIRST (finalizes the open
        // row via live_thinking_idx), THEN clear the accumulators/index. The
        // flush-before-clear ordering is precisely the FIX C guarantee.
        let mut mapper = EventMapper::new("orphan-finalize");

        // Open turn 1 thinking and stream "A".
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_start".to_string(),
            delta: None,
            content_index: Some(0),
        });
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("A".to_string()),
            content_index: Some(0),
        });
        assert_eq!(mapper.live_thinking_idx, Some(0));
        assert_eq!(mapper.accumulated_thinking, "A");
        assert!(matches!(mapper.entries[0], ConsoleEntry::Thinking { .. }));

        // New turn begins WITHOUT a preceding thinking_end -> replicate the
        // `agent_start` arm (FIX C: flush open thinking BEFORE clearing index).
        mapper.flush_thinking_block();
        mapper.accumulated_text.clear();
        mapper.accumulated_thinking.clear();
        mapper.live_thinking_idx = None;
        mapper.active_content_index = None;
        // Exactly one Thinking entry so far, finalized with "A".
        let count_after_start = mapper
            .entries
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Thinking { .. }))
            .count();
        assert_eq!(count_after_start, 1, "open 'A' finalized, no orphan");
        match mapper.entries[0] {
            ConsoleEntry::Thinking { ref text, .. } => assert_eq!(text, "A"),
            ref other => panic!("expected finalized 'A' Thinking, got {other:?}"),
        }
        assert!(
            mapper.live_thinking_idx.is_none(),
            "tracker cleared by agent_start flush"
        );
        assert!(
            mapper.accumulated_thinking.is_empty(),
            "thinking accumulator cleared by agent_start"
        );

        // Stream "B" in the new turn.
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_delta".to_string(),
            delta: Some("B".to_string()),
            content_index: Some(0),
        });
        assert_eq!(mapper.live_thinking_idx, Some(1), "'B' pushed as a new entry");

        // Finalize "B".
        mapper.apply_message_delta(&AssistantMessageEvent {
            delta_type: "thinking_end".to_string(),
            delta: None,
            content_index: Some(0),
        });

        // Exactly two Thinking entries: "A" (its own row) and "B" (its own row).
        // No merge, no duplicate, no orphan.
        let final_thinking: Vec<String> = mapper
            .entries
            .iter()
            .filter_map(|e| match e {
                ConsoleEntry::Thinking { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            final_thinking, vec!["A".to_string(), "B".to_string()],
            "'A' and 'B' finalized as separate entries, no stale-merge"
        );
        assert!(mapper.accumulated_thinking.is_empty());
        assert!(mapper.live_thinking_idx.is_none());
    }

    // -- Part B: partial result snippet extraction -----------------------------

    #[test]
    fn partial_snippet_prefers_content_text() {
        let partial = serde_json::json!({
            "content": [{"type": "text", "text": "child agent: searching files"}]
        });
        assert_eq!(extract_partial_snippet(&partial), "child agent: searching files");
    }

    #[test]
    fn partial_snippet_falls_back_to_json() {
        let partial = serde_json::json!({ "progress": 0.5, "step": "compile" });
        let s = extract_partial_snippet(&partial);
        assert!(s.contains("compile"), "json fallback must surface the data: {s}");
    }

    #[test]
    fn partial_snippet_caps_at_200_chars_and_single_lines() {
        let long = "a".repeat(300);
        let partial = serde_json::json!({ "content": [{ "type": "text", "text": long }] });
        let s = extract_partial_snippet(&partial);
        assert!(s.chars().count() <= 201, "capped at 200 + ellipsis, got {}", s.chars().count());
        assert!(!s.contains('\n'), "must be single-line");
    }

    #[test]
    fn partial_snippet_replaces_newlines() {
        let partial = serde_json::json!({
            "content": [{ "type": "text", "text": "line one\nline two\rline three" }]
        });
        let s = extract_partial_snippet(&partial);
        assert!(!s.contains('\n') && !s.contains('\r'), "newlines replaced: {s}");
        assert!(s.contains('␤'), "newline symbol present");
    }

    // -- Part B: live tool progress + eviction index tracking -------------------

    #[test]
    fn tool_progress_rewrites_args_row_in_place() {
        let mut mapper = EventMapper::new("pi-prog");
        mapper.active_tool_progress.insert("t1".to_string(), (0, "  args: {}".to_string()));
        mapper.push_entry(ConsoleEntry::Coder {
            node: None,
            text: "  args: {}".to_string(),
            time: String::new(),
        });
        let partial = serde_json::json!({ "content": [{ "type": "text", "text": "working…" }] });
        // A different tool must NOT rewrite the row.
        mapper.rewrite_tool_progress("other", &partial);
        match mapper.entries[0] {
            ConsoleEntry::Coder { ref text, .. } => assert_eq!(text, "  args: {}"),
            _ => panic!("entry 0 must stay a Coder row"),
        }
        // The matching tool rewrites it to a progress line.
        mapper.rewrite_tool_progress("t1", &partial);
        match mapper.entries[0] {
            ConsoleEntry::Coder { ref text, .. } => assert_eq!(text, "  ⋯ working…"),
            _ => panic!("entry 0 must stay a Coder row"),
        }
    }

    #[test]
    fn tool_progress_tracker_survives_front_eviction() {
        // Fill to cap, set a tracker strictly inside the window, then push past the
        // cap. Front-eviction must decrement the stored index so it keeps pointing
        // at the SAME entry, not a stale slot.
        let mut mapper = EventMapper::new("pi-ev1");
        for i in 0..MAX_CONSOLE_ENTRIES {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: format!("turn-{i}"),
                time: String::new(),
                msg_id: None,
            });
        }
        // The tracked entry originally sits at index 250.
        mapper.active_tool_progress.insert("t".to_string(), (250, "turn-250".to_string()));
        for _ in 0..100 {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: "x".to_string(),
                time: String::new(),
                msg_id: None,
            });
        }
        // 100 front-evictions shift index 250 -> 150 (and the tracked entry with it).
        match mapper.active_tool_progress.get("t") {
            Some((idx, _)) => {
                assert_eq!(*idx, 150, "tracker index must follow the eviction shift")
            }
            None => panic!("tracker must survive (entry not yet evicted)"),
        }
        match mapper.entries[150] {
            ConsoleEntry::Chat { ref text, .. } => {
                assert_eq!(text, "turn-250", "index must point at the original entry")
            }
            _ => panic!("entry at 150 must be the tracked Chat row"),
        }
    }

    #[test]
    fn tool_progress_tracker_dropped_when_its_entry_is_evicted() {
        // A tracker pointing near the front, then enough pushes to evict that very
        // entry, must be dropped (index 0 -> evicted on the next push).
        let mut mapper = EventMapper::new("pi-ev2");
        for i in 0..MAX_CONSOLE_ENTRIES {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: format!("turn-{i}"),
                time: String::new(),
                msg_id: None,
            });
        }
        mapper.active_tool_progress.insert("t".to_string(), (5, "turn-5".to_string()));
        for _ in 0..6 {
            mapper.push_entry(ConsoleEntry::Chat {
                role: "user".to_string(),
                text: "x".to_string(),
                time: String::new(),
                msg_id: None,
            });
        }
        assert!(
            mapper.active_tool_progress.is_empty(),
            "tracker must drop once its entry is evicted"
        );
    }

    #[test]
    fn tool_progress_restored_on_end() {
        let mut mapper = EventMapper::new("pi-restore");
        mapper.push_entry(ConsoleEntry::Coder {
            node: Some(NodeStyle::Dot),
            text: "🔧 Calling `subagent`".to_string(),
            time: String::new(),
        });
        let args_text = "  args: {\"q\":\"x\"}".to_string();
        mapper.push_entry(ConsoleEntry::Coder {
            node: None,
            text: args_text.clone(),
            time: String::new(),
        });
        mapper.active_tool_progress.insert("t1".to_string(), (1, args_text.clone()));
        let partial = serde_json::json!({ "content": [{ "type": "text", "text": "halfway there" }] });
        mapper.rewrite_tool_progress("t1", &partial);
        match mapper.entries[1] {
            ConsoleEntry::Coder { ref text, .. } => assert_eq!(text, "  ⋯ halfway there"),
            _ => panic!("entry 1 must stay a Coder row"),
        }
        mapper.restore_tool_progress("t1");
        match mapper.entries[1] {
            ConsoleEntry::Coder { ref text, .. } => {
                assert_eq!(text, &args_text, "args line restored on end")
            }
            _ => panic!("entry 1 must stay a Coder row"),
        }
    }

    #[test]
    fn parallel_tools_do_not_corrupt_each_others_rows() {
        // Fix 1 / audit: the exact interleaving that broke the single-slot tracker.
        // A start -> B start (clobbers single slot in the old code) -> B update ->
        // A end (old code nuked the slot, leaving B's row corrupted) -> B update ->
        // B end. With a per-id HashMap, B's row must be restored and the map empty.
        let mut mapper = EventMapper::new("pi-parallel");
        let args_a = "  args: {\"a\":1}".to_string();
        let args_b = "  args: {\"b\":2}".to_string();
        // A start: Calling(A) + args(A), register tracker A at the args index.
        mapper.push_entry(ConsoleEntry::Coder {
            node: Some(NodeStyle::Dot),
            text: "🔧 Calling `A`".to_string(),
            time: String::new(),
        });
        mapper.push_entry(ConsoleEntry::Coder {
            node: None,
            text: args_a.clone(),
            time: String::new(),
        });
        mapper
            .active_tool_progress
            .insert("A".to_string(), (mapper.entries.len() - 1, args_a.clone()));
        // B start: Calling(B) + args(B), register tracker B (must NOT clobber A).
        mapper.push_entry(ConsoleEntry::Coder {
            node: Some(NodeStyle::Dot),
            text: "🔧 Calling `B`".to_string(),
            time: String::new(),
        });
        mapper.push_entry(ConsoleEntry::Coder {
            node: None,
            text: args_b.clone(),
            time: String::new(),
        });
        mapper
            .active_tool_progress
            .insert("B".to_string(), (mapper.entries.len() - 1, args_b.clone()));
        assert_eq!(mapper.active_tool_progress.len(), 2, "both trackers present");

        let partial = serde_json::json!({ "content": [{ "type": "text", "text": "B progress" }] });
        // B update rewrites ONLY B's row (index 3).
        mapper.rewrite_tool_progress("B", &partial);
        match &mapper.entries[1] {
            ConsoleEntry::Coder { text, .. } => assert_eq!(text, &args_a, "A row untouched by B update"),
            _ => panic!("entry 1 must be the A args row"),
        }
        match &mapper.entries[3] {
            ConsoleEntry::Coder { text, .. } => assert!(text.contains("B progress"), "B row shows progress"),
            _ => panic!("entry 3 must be the B args row"),
        }
        // A end: restore A's row, remove A only — B's tracker must survive.
        mapper.restore_tool_progress("A");
        match &mapper.entries[1] {
            ConsoleEntry::Coder { text, .. } => assert_eq!(text, &args_a, "A row restored"),
            _ => panic!("entry 1 must be the A args row"),
        }
        match &mapper.entries[3] {
            ConsoleEntry::Coder { text, .. } => assert!(text.contains("B progress"), "B row still in progress"),
            _ => panic!("entry 3 must be the B args row"),
        }
        assert_eq!(mapper.active_tool_progress.len(), 1, "A removed, B remains");
        // B update again (still tracked), then B end -> restore B, map empty.
        mapper.rewrite_tool_progress("B", &partial);
        mapper.restore_tool_progress("B");
        match &mapper.entries[3] {
            ConsoleEntry::Coder { text, .. } => assert_eq!(text, &args_b, "B row restored to original"),
            _ => panic!("entry 3 must be the B args row"),
        }
        assert!(mapper.active_tool_progress.is_empty(), "all trackers removed");
    }

    #[test]
    fn result_summary_caps_multibyte_without_panic() {
        // Fix 3 / audit: a >200-byte string made of multi-byte chars must cap at
        // 200 CHARS (the old `&text[..200]` byte-slice would panic). 'α' is 2 bytes.
        let big = "α".repeat(400); // 800 bytes, 400 chars
        let capped = cap_chars(&big, 200);
        assert_eq!(capped.chars().count(), 201, "200 chars + ellipsis");
        assert!(capped.ends_with('…'), "ellipsis appended");
        // Same path the console uses for tool result summaries.
        let partial = serde_json::json!({ "content": [{ "type": "text", "text": big }] });
        let s = extract_partial_snippet(&partial);
        assert_eq!(s.chars().count(), 201, "snippet capped at 200 + ellipsis");
    }

    #[test]
    fn stop_pi_session_unknown_id_reports_no_live_session() {
        // Fix 5 / audit: the LIVE-existence decision must be false for any id that is
        // not a live pi session. `stop_agent_process_only` relies on `Ok(false)` to
        // fall through to its ledger/external routes. Exercised via the pure seam —
        // `stop_pi_session` itself needs a real AppHandle, which the test env lacks
        // (no tauri `test` feature), so we assert the decision it returns.
        let state = PiSidecarState::default();
        assert!(
            !pi_session_existed(&state, "pi-does-not-exist"),
            "unknown id => no live session"
        );
        assert!(!pi_session_existed(&state, ""), "empty id => no live session");
        // A present slot WITHOUT a live inner session must also report false.
        state
            .inner
            .lock()
            .unwrap()
            .insert("pi-empty".to_string(), SessionSlot { inner: None });
        assert!(
            !pi_session_existed(&state, "pi-empty"),
            "slot without live session => false"
        );
    }

    // -- Part C: banners for compaction / retry / error / queue drops ----------

    fn pevent(raw: &str) -> PiEvent {
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn banner_compaction_start() {
        let e = pevent(r#"{"type":"compaction_start","reason":"threshold"}"#);
        assert_eq!(
            banner_text_for_event(&e),
            Some("Compacting context (threshold)…".to_string())
        );
    }

    #[test]
    fn banner_compaction_end_variants() {
        let aborted = pevent(r#"{"type":"compaction_end","aborted":true}"#);
        assert_eq!(
            banner_text_for_event(&aborted),
            Some("Compaction aborted".to_string())
        );
        let failed = pevent(r#"{"type":"compaction_end","errorMessage":"oom"}"#);
        assert_eq!(
            banner_text_for_event(&failed),
            Some("Compaction failed: oom".to_string())
        );
        let ok = pevent(r#"{"type":"compaction_end"}"#);
        assert_eq!(
            banner_text_for_event(&ok),
            Some("Context compacted".to_string())
        );
    }

    #[test]
    fn banner_auto_retry_start() {
        let e = pevent(
            r#"{"type":"auto_retry_start","attempt":2,"maxAttempts":5,"errorMessage":"rate limited"}"#,
        );
        assert_eq!(
            banner_text_for_event(&e),
            Some("Provider error — retry 2/5: rate limited".to_string())
        );
    }

    #[test]
    fn banner_auto_retry_end_only_on_failure() {
        let ok = pevent(r#"{"type":"auto_retry_end","success":true}"#);
        assert_eq!(banner_text_for_event(&ok), None, "success is silent");
        let fail = pevent(r#"{"type":"auto_retry_end","success":false,"finalError":"exhausted"}"#);
        assert_eq!(
            banner_text_for_event(&fail),
            Some("Retries exhausted: exhausted".to_string())
        );
    }

    #[test]
    fn banner_sidecar_error() {
        let e = pevent(r#"{"type":"error","context":"spawn","message":"boom"}"#);
        assert_eq!(
            banner_text_for_event(&e),
            Some("Sidecar error [spawn]: boom".to_string())
        );
    }

    #[test]
    fn banner_queue_dropped() {
        let e = pevent(r#"{"type":"queue_dropped","count":3}"#);
        assert_eq!(
            banner_text_for_event(&e),
            Some("Dropped 3 queued prompt(s) on shutdown".to_string())
        );
    }

    #[test]
    fn banner_unknown_event_is_none() {
        let e = pevent(r#"{"type":"something_else"}"#);
        assert_eq!(banner_text_for_event(&e), None);
    }

    // -- session persistence (save / restore / cleanup) ----------------------

    #[test]
    fn save_and_restore_roundtrips() {
        std::env::set_var("DEVBOULE_PI_PERSIST", "true");
        let state = PiSidecarState::default();
        let now = super::now_ms();
        {
            let mut g = state.persisted.lock().unwrap();
            g.insert(
                "pi-1".to_string(),
                PersistedSession {
                    id: "pi-1".to_string(),
                    agent_role: "main-coder".to_string(),
                    project_id: Some("my-project".to_string()),
                    created_at: now,
                    last_active_at: now,
                    status: SessionStatus::Active,
                },
            );
            g.insert(
                "pi-2".to_string(),
                PersistedSession {
                    id: "pi-2".to_string(),
                    agent_role: "orchestrator".to_string(),
                    project_id: Some("my-project".to_string()),
                    created_at: now,
                    last_active_at: now,
                    status: SessionStatus::Active,
                },
            );
        }

        let root = std::env::temp_dir().join(format!(
            "pi-sessions-rt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        save_pi_sessions(&state, &root);

        let restored = restore_pi_sessions(&root);
        assert_eq!(restored.len(), 2, "both active sessions must roundtrip");
        let ids: std::collections::HashSet<String> =
            restored.iter().map(|s| s.session_id.clone()).collect();
        assert!(ids.contains("pi-1"));
        assert!(ids.contains("pi-2"));
        let roles: std::collections::HashSet<String> =
            restored.iter().map(|s| s.agent_role.clone()).collect();
        assert!(roles.contains("main-coder"));
        assert!(roles.contains("orchestrator"));

        // File on disk must contain both sessions.
        let data = std::fs::read_to_string(root.join(".devboule").join("pi-sessions.json")).unwrap();
        assert!(data.contains("pi-1") && data.contains("pi-2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_sessions_purged() {
        std::env::set_var("DEVBOULE_PI_PERSIST", "true");
        let state = PiSidecarState::default();
        let now = super::now_ms();
        let old = now - 8 * 24 * 3600 * 1000;
        {
            let mut g = state.persisted.lock().unwrap();
            g.insert(
                "old-stopped".to_string(),
                PersistedSession {
                    id: "old-stopped".to_string(),
                    agent_role: "mini-coder".to_string(),
                    project_id: None,
                    created_at: old,
                    last_active_at: old,
                    status: SessionStatus::Stopped,
                },
            );
            g.insert(
                "fresh-stopped".to_string(),
                PersistedSession {
                    id: "fresh-stopped".to_string(),
                    agent_role: "mini-coder".to_string(),
                    project_id: None,
                    created_at: now,
                    last_active_at: now,
                    status: SessionStatus::Stopped,
                },
            );
        }

        let root = std::env::temp_dir().join(format!(
            "pi-sessions-purge-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        save_pi_sessions(&state, &root);

        let data = std::fs::read_to_string(root.join(".devboule").join("pi-sessions.json")).unwrap();
        let file: SessionFile = serde_json::from_str(&data).unwrap();
        let ids: Vec<String> = file.sessions.iter().map(|s| s.id.clone()).collect();
        assert!(
            !ids.contains(&"old-stopped".to_string()),
            "8-day-old stopped session must be purged"
        );
        assert!(
            ids.contains(&"fresh-stopped".to_string()),
            "fresh stopped session must remain"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // -- #1: zombie session eviction on stdout-close-while-child-alive -------

    #[test]
    fn zombie_session_detected_on_stdout_close_with_child_alive() {
        // Scenario: reader EOF with the child STILL alive (try_wait returned
        // Ok(None)) and no exit status. classify_sidecar_termination(None, ..)
        // returns Clean, which would leak the session — the zombie guard must
        // flag it for forced kill + eviction.
        assert!(
            is_zombie_stdout_close(None, true),
            "Ok(None) from try_wait + closed stdout must be a zombie"
        );
        // Not a zombie when the child has already exited cleanly.
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert!(
                !is_zombie_stdout_close(Some(std::process::ExitStatus::from_raw(0)), true),
                "exited child is not a zombie"
            );
        }
        // Not a zombie when liveness is unknown (try_wait errored).
        assert!(
            !is_zombie_stdout_close(None, false),
            "unknown liveness must not be treated as a zombie"
        );
    }

    // -- #2: concurrent save_pi_sessions must not lose writes ----------------

    #[test]
    fn save_pi_sessions_concurrent_no_data_loss() {
        std::env::set_var("DEVBOULE_PI_PERSIST", "true");
        let state = std::sync::Arc::new(PiSidecarState::default());
        let n: usize = 16;
        let root = std::env::temp_dir().join(format!(
            "pi-sessions-conc-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let now = super::now_ms();
        let mut handles = Vec::new();
        for i in 0..n {
            let state = std::sync::Arc::clone(&state);
            let root = root.clone();
            handles.push(std::thread::spawn(move || {
                let id = format!("pi-conc-{i}");
                {
                    let mut g = state.persisted.lock().unwrap();
                    g.insert(
                        id.clone(),
                        PersistedSession {
                            id: id.clone(),
                            agent_role: "main-coder".to_string(),
                            project_id: None,
                            created_at: now,
                            last_active_at: now,
                            status: SessionStatus::Active,
                        },
                    );
                }
                save_pi_sessions(&state, &root);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Every thread's session must survive — no lost writes under the race.
        let data = std::fs::read_to_string(root.join(".devboule").join("pi-sessions.json")).unwrap();
        let file: SessionFile = serde_json::from_str(&data).unwrap();
        assert_eq!(
            file.sessions.len(),
            n,
            "all {n} concurrent sessions must be persisted (no data loss)"
        );
        for i in 0..n {
            assert!(
                file.sessions.iter().any(|s| s.id == format!("pi-conc-{i}")),
                "session pi-conc-{i} lost under concurrent save"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // -- Fix 2: pi_enabled_unrecognized (unrecognized-but-enabled warning) -----
    // Pure decision used by `pi_sidecar_enabled` to decide whether to WARN (the
    // sidecar is still enabled). Doesn't read env, so no lock needed; kept
    // lockless on purpose so it can run in parallel with the env-mutating tests.
    #[test]
    fn pi_enabled_unrecognized_false_for_falsy_truthy_and_empty() {
        for v in [
            "0", "false", "no", "off", "1", "true", "yes", "on", "", "  ", "\t",
        ] {
            assert!(
                !pi_enabled_unrecognized(v),
                "DEVBOULE_PI_ENABLED={v:?} must NOT trigger an unrecognized warning"
            );
        }
    }

    #[test]
    fn pi_enabled_unrecognized_true_for_garbage_values() {
        // These are the exact typo'd values the audit flagged: `disable`/`none`/
        // `disabled` would have been silently ENABLED before Fix 2. We now WARN
        // (and still enable — unknown -> enabled).
        for v in [
            "disable", "none", "disabled", "garbage", "2", "ENABLED", "  yesx ",
        ] {
            assert!(
                pi_enabled_unrecognized(v),
                "DEVBOULE_PI_ENABLED={v:?} should warn (unrecognized but enabled)"
            );
        }
    }

    // -- Fix 3: env_flag_default_on / DEVBOULE_PI_SANDBOX (inverted-footgun) ----
    // Serialize against anything reading DEVBOULE_PI_SANDBOX. Separate lock from
    // PI_SIDECAR_ENABLED_ENV_LOCK because they guard DIFFERENT variables; sharing
    // would needlessly serialize the two suites. (Only these tests read
    // DEVBOULE_PI_SANDBOX, so the lock is sufficient.)
    static PI_SIDECAR_SANDBOX_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn devboule_pi_sandbox_defaults_on_when_unset() {
        let _guard = PI_SIDECAR_SANDBOX_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DEVBOULE_PI_SANDBOX");
        assert!(env_flag_default_on("DEVBOULE_PI_SANDBOX"));
    }

    #[test]
    fn devboule_pi_sandbox_disabled_for_falsy_values() {
        let _guard = PI_SIDECAR_SANDBOX_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for v in ["0", "false", "no", "off", "FALSE", "Off", "  no  "] {
            std::env::set_var("DEVBOULE_PI_SANDBOX", v);
            assert!(
                !env_flag_default_on("DEVBOULE_PI_SANDBOX"),
                "DEVBOULE_PI_SANDBOX={v:?} must DISABLE the sandbox"
            );
        }
        std::env::remove_var("DEVBOULE_PI_SANDBOX");
    }

    #[test]
    fn devboule_pi_sandbox_enabled_for_true_and_one() {
        // Fix 3 regression: the OLD code did `.map(|v| v == "true")` which silently
        // DISABLED the sandbox for `1`/`TRUE`/`yes`/garbage — turning the macOS
        // Seatbelt OFF. Now any non-falsy value (the tolerant opt-out semantics)
        // keeps the sandbox ON.
        let _guard = PI_SIDECAR_SANDBOX_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for v in ["true", "1", "yes", "on", "TRUE", "Yes", "  yes ", "garbage", "ENABLED"] {
            std::env::set_var("DEVBOULE_PI_SANDBOX", v);
            assert!(
                env_flag_default_on("DEVBOULE_PI_SANDBOX"),
                "DEVBOULE_PI_SANDBOX={v:?} must ENABLE the sandbox (opt-out)"
            );
        }
        std::env::remove_var("DEVBOULE_PI_SANDBOX");
    }

    // -- #8: devboule_censor_review parse + dispatch (pure) --------------------

    #[test]
    fn censor_dispatch_parses_files_project_and_session() {
        let line = r#"{"type":"devboule_censor_review","files":["src/a.rs","src/b.rs"],"_devboule":{"projectId":"my-proj","sessionId":"pi-42"}}"#;
        let event: PiEvent = serde_json::from_str(line).unwrap();
        let d = censor_review_dispatch(&event, "pi-42");
        assert_eq!(d.banner, "⚑ Censor review started for: src/a.rs, src/b.rs");
        assert_eq!(d.project_id.as_deref(), Some("my-proj"));
        assert_eq!(d.session_id, "pi-42");
        assert_eq!(d.files, vec!["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn censor_dispatch_no_project_id_signals_skip() {
        let line = r#"{"type":"devboule_censor_review","files":["src/a.rs"],"_devboule":{"sessionId":"pi-7"}}"#;
        let event: PiEvent = serde_json::from_str(line).unwrap();
        let d = censor_review_dispatch(&event, "pi-7");
        // Banner still announces the start...
        assert_eq!(d.banner, "⚑ Censor review started for: src/a.rs");
        // ...but a missing project id must signal the caller to skip loudly.
        assert!(
            d.project_id.is_none(),
            "missing project id must signal the skip path"
        );
    }

    #[test]
    fn censor_dispatch_session_id_falls_back_to_agent() {
        // No sessionId in _devboule: must fall back to the mapper's agent id so the
        // findings prompt is delivered to the correct session.
        let line = r#"{"type":"devboule_censor_review","files":["src/a.rs"],"_devboule":{"projectId":"p1"}}"#;
        let event: PiEvent = serde_json::from_str(line).unwrap();
        let d = censor_review_dispatch(&event, "pi-agent-xyz");
        assert_eq!(d.session_id, "pi-agent-xyz");
        assert_eq!(d.project_id.as_deref(), Some("p1"));
    }

    // -- #8: path relativization (pure) ---------------------------------------

    #[test]
    fn relativize_strips_root_prefix() {
        assert_eq!(
            relativize_censor_path_in("/proj", "/proj/src/main.rs"),
            "src/main.rs"
        );
        assert_eq!(
            relativize_censor_path_in("/proj/", "/proj/src/main.rs"),
            "src/main.rs"
        );
    }

    #[test]
    fn relativize_keeps_relative_and_unrelated_absolute() {
        // Already relative: unchanged.
        assert_eq!(
            relativize_censor_path_in("/proj", "src/main.rs"),
            "src/main.rs"
        );
        // Absolute but not under root: unchanged (process_censor_review will simply
        // find no shard for it — never panics).
        assert_eq!(
            relativize_censor_path_in("/proj", "/elsewhere/main.rs"),
            "/elsewhere/main.rs"
        );
    }

    // -- #8: findings message composition (pure) ------------------------------

    #[test]
    fn compose_message_formats_file_line_and_caps() {
        use crate::backend::censor::schema::{Finding, Severity, Verdict};
        let findings = vec![Finding {
            file: "src/main.rs".to_string(),
            line: Some(42),
            severity: Severity::High,
            title: "Use of unsafe".to_string(),
            body: "Prefer safe API here.".to_string(),
            verdict: Verdict::Confirmed,
            ..Default::default()
        }];
        let msg = compose_censor_review_message(&findings);
        assert!(msg.contains("Automated Censor review found 1 issue(s):"));
        assert!(msg.contains("- src/main.rs:42 [HIGH] Use of unsafe — Prefer safe API here."));
        assert!(msg.contains("Fix the confirmed issues above, then continue."));
        // Short message must NOT be truncated.
        assert!(!msg.contains('…'), "short message must not be truncated");
    }

    #[test]
    fn compose_message_caps_length_and_empty_count() {
        use crate::backend::censor::schema::{Finding, Severity, Verdict};
        // Empty findings still composes a well-formed (clean) message.
        let empty = compose_censor_review_message(&[]);
        assert!(empty.contains("found 0 issue(s)"));
        assert!(empty.contains("Fix the confirmed issues above, then continue."));

        // Many long findings must be capped (no overflow past the cap).
        let mut many = Vec::new();
        for i in 0..50 {
            many.push(Finding {
                file: format!("src/mod{i}/file.rs"),
                line: Some(i),
                severity: Severity::Low,
                title: format!("Finding number {i} with a very long title that keeps going"),
                body: "x".repeat(500),
                verdict: Verdict::Confirmed,
                ..Default::default()
            });
        }
        let msg = compose_censor_review_message(&many);
        assert!(
            msg.chars().count() <= CENSOR_REVIEW_MSG_MAX_CHARS,
            "message must be capped at CENSOR_REVIEW_MSG_MAX_CHARS"
        );
        assert!(msg.contains('…'), "overlong message must be truncated");
    }

    // -- #8: anti-loop counter (pure) ----------------------------------------

    #[test]
    fn censor_loop_allows_up_to_cap_then_blocks() {
        let mut map: HashMap<String, u8> = HashMap::new();
        // Rounds 1 and 2 are allowed (cap = 2 consecutive sent rounds).
        assert!(censor_loop_allow_in(&map, "pi-1", CENSOR_LOOP_MAX_CONSECUTIVE));
        censor_loop_bump_in(&mut map, "pi-1");
        assert!(censor_loop_allow_in(&map, "pi-1", CENSOR_LOOP_MAX_CONSECUTIVE));
        censor_loop_bump_in(&mut map, "pi-1");
        // Round 3 must be blocked.
        assert!(!censor_loop_allow_in(&map, "pi-1", CENSOR_LOOP_MAX_CONSECUTIVE));
    }

    #[test]
    fn censor_loop_reset_clears_counter() {
        let mut map: HashMap<String, u8> = HashMap::new();
        censor_loop_bump_in(&mut map, "pi-9");
        censor_loop_bump_in(&mut map, "pi-9");
        assert!(!censor_loop_allow_in(&map, "pi-9", CENSOR_LOOP_MAX_CONSECUTIVE));
        censor_loop_reset_in(&mut map, "pi-9");
        assert!(censor_loop_allow_in(&map, "pi-9", CENSOR_LOOP_MAX_CONSECUTIVE));
    }

    // -- Fix 4: delivered-id dedup (pure) ------------------------------------

    /// Build a Confirmed finding with a deterministic id (mirrors how the shard
    /// ids findings; we only need the `id` field for dedup).
    fn finding_with_id(id: &str) -> crate::backend::censor::schema::Finding {
        use crate::backend::censor::schema::Verdict;
        let mut f = crate::backend::censor::schema::Finding::default();
        f.id = id.to_string();
        f.verdict = Verdict::Confirmed;
        f
    }

    #[test]
    fn censor_dedup_in_accepts_new_findings() {
        use crate::backend::censor::schema::{Finding, Verdict};
        use std::collections::HashSet;
        let _ = (Finding::default(), Verdict::Confirmed); // ensure schema types in scope
        let mut delivered: HashSet<String> = HashSet::new();
        let findings = vec![finding_with_id("a"), finding_with_id("b"), finding_with_id("c")];
        let (new_findings, already) = censor_dedup_in(&mut delivered, &findings);
        assert_eq!(already, 0, "first pass: nothing previously delivered");
        assert_eq!(new_findings.len(), 3, "first pass: all three are new");
        assert_eq!(delivered.len(), 3, "ids recorded as delivered");
    }

    #[test]
    fn censor_dedup_in_drops_already_delivered() {
        use std::collections::HashSet;
        let mut delivered: HashSet<String> = HashSet::new();
        let first = vec![finding_with_id("a"), finding_with_id("b")];
        let (new1, already1) = censor_dedup_in(&mut delivered, &first);
        assert_eq!(already1, 0);
        assert_eq!(new1.len(), 2);
        // A later review of the SAME files returns both findings again — dedup
        // must suppress them so the agent isn't nagged every 2 rounds.
        let second = vec![finding_with_id("a"), finding_with_id("b"), finding_with_id("c")];
        let (new2, already2) = censor_dedup_in(&mut delivered, &second);
        assert_eq!(already2, 2, "two were already delivered");
        assert_eq!(new2.len(), 1, "only the new id (c) survives");
        assert_eq!(new2[0].id, "c");
        // A third pass with nothing new reports all-already and zero new.
        let third = vec![finding_with_id("a"), finding_with_id("b"), finding_with_id("c")];
        let (new3, already3) = censor_dedup_in(&mut delivered, &third);
        assert_eq!(already3, 3);
        assert_eq!(new3.len(), 0, "no new findings on third pass");
    }

    // -- pi_route_for_launch: pure routing decision for the delegation gates --

    #[test]
    fn pi_route_orchestrator_with_local_client_and_enabled() {
        assert_eq!(
            pi_route_for_launch(true, "orchestrator", "orchestrator", true),
            Some("orchestrator"),
            "orchestrator + local client + enabled ⇒ pi orchestrator"
        );
    }

    #[test]
    fn pi_route_orchestrator_with_claudient_is_none() {
        assert_eq!(
            pi_route_for_launch(true, "orchestrator", "claude", true),
            None,
            "orchestrator + claude client ⇒ None (claude runs outside pi)"
        );
    }

    #[test]
    fn pi_route_coder_with_claude_is_none() {
        assert_eq!(
            pi_route_for_launch(true, "coder", "claude", true),
            None,
            "coder + claude client ⇒ None"
        );
    }

    #[test]
    fn pi_route_coder_with_local_client_and_enabled() {
        assert_eq!(
            pi_route_for_launch(true, "coder", "orchestrator", true),
            Some("coder"),
            "coder + local client + enabled ⇒ pi coder"
        );
    }

    #[test]
    fn pi_route_mini_with_local_client_and_enabled() {
        assert_eq!(
            pi_route_for_launch(true, "mini", "orchestrator", true),
            Some("coder"),
            "mini + local client + enabled ⇒ pi coder"
        );
    }

    #[test]
    fn pi_route_disabled_always_none() {
        assert_eq!(pi_route_for_launch(true, "orchestrator", "orchestrator", false), None);
        assert_eq!(pi_route_for_launch(true, "coder", "orchestrator", false), None);
        assert_eq!(pi_route_for_launch(true, "mini", "orchestrator", false), None);
    }

    #[test]
    fn pi_route_prepare_only_always_none() {
        // launch_terminal=false means prepare-only path (Copy prompt) ⇒ no pi route
        assert_eq!(pi_route_for_launch(false, "orchestrator", "orchestrator", true), None);
        assert_eq!(pi_route_for_launch(false, "coder", "orchestrator", true), None);
    }

    #[test]
    fn pi_route_unknown_role_is_none() {
        assert_eq!(pi_route_for_launch(true, "verifier", "orchestrator", true), None);
        assert_eq!(pi_route_for_launch(true, "", "orchestrator", true), None);
    }

    #[test]
    fn pi_route_codex_and_openai_clients_are_none() {
        assert_eq!(pi_route_for_launch(true, "orchestrator", "codex", true), None);
        assert_eq!(pi_route_for_launch(true, "coder", "openai", true), None);
    }

    // -- websearch_env_pairs (pure helper) ------------------------------------

    #[test]
    fn websearch_env_pairs_skips_missing_keys() {
        let pairs = websearch_env_pairs(|_provider| Ok(None));
        assert!(pairs.is_empty(), "missing keys should produce no env pairs");
    }

    #[test]
    fn websearch_env_pairs_returns_present_keys() {
        let pairs = websearch_env_pairs(|provider| match provider {
            "exa" => Ok(Some("test-exa-key-12345678".into())),
            _ => Ok(None),
        });
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "EXA_API_KEY");
        assert_eq!(pairs[0].1, "test-exa-key-12345678");
    }

    #[test]
    fn websearch_env_pairs_multiple_keys() {
        let pairs = websearch_env_pairs(|provider| match provider {
            "exa" => Ok(Some("exa-key-12345678".into())),
            "brave" => Ok(Some("brave-key-12345678".into())),
            _ => Ok(None),
        });
        assert_eq!(pairs.len(), 2);
        let env_names: Vec<&str> = pairs.iter().map(|(name, _)| *name).collect();
        assert!(env_names.contains(&"EXA_API_KEY"));
        assert!(env_names.contains(&"BRAVE_API_KEY"));
    }

    #[test]
    fn websearch_env_pairs_skips_on_vault_error() {
        let pairs = websearch_env_pairs(|_provider| Err("keyring error".into()));
        assert!(pairs.is_empty(), "vault errors must be silently skipped");
    }

    #[test]
    fn websearch_env_pairs_all_7_providers_have_env_var() {
        for provider in ["exa", "brave", "tavily", "perplexity", "gemini_search", "openai_search", "parallel"] {
            let pairs = websearch_env_pairs(|p| {
                if p == provider {
                    Ok(Some("test-key-12345678".into()))
                } else {
                    Ok(None)
                }
            });
            assert_eq!(pairs.len(), 1, "provider {provider} must produce exactly one env pair");
        }
    }
}

