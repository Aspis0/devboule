//! User-configured MCP servers (Phase A): the config layer + Tauri commands that let
//! a user declare their OWN MCP servers and have them injected into the MAIN coder
//! launch configs (claude `.mcp.json` / codex `-c mcp_servers.*`). See
//! `docs/design-user-mcp-servers-2026-06.md`.
//!
//! HARD invariant (design §6): the MINI coder NEVER receives user MCP servers. This
//! module is wired ONLY into the MAIN-coder launch path (`projects.rs`). Nothing in
//! `mini_coder_executor.rs` references it.
//!
//! Two scopes (design §2.1):
//! - GLOBAL  — `<app-data>/user-mcp-servers.json` (every project, resolved via the same
//!   Tauri `app_data_dir()` the rest of the backend uses for global state).
//! - PROJECT — `<project_root>/.devboule/mcp-servers.json` (git-versionable, per-repo).
//!
//! Merge (design §2.1, §5.1): `global ∪ project`, PROJECT WINS on a name collision, then
//! filter to `enabled == true`. The Oracle (`aspis-management`) is NOT in this list — the
//! launch builders add it separately, always first.
//!
//! Reads FAIL OPEN: a missing/oversized/malformed file ⇒ empty (never crash a launch); a
//! single invalid entry is skipped with a warning, the rest are kept.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use super::design::{atomic_write, design_write_guard};
use super::state::BackendState;

/// Max bytes of an MCP-servers config file. These files are a handful of small server
/// records, not a corpus — cap reads so a runaway/hostile file can never fully allocate
/// (a file over the cap is treated as absent ⇒ fail-open empty). Sized generously for a
/// realistic list of servers with env maps.
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// The only transport supported in v1 (design §2.2 / §9): `stdio` (child process). Any
/// other value is rejected at add time and skipped on read with a warning.
const STDIO_TRANSPORT: &str = "stdio";

/// Reserved name PREFIXES a user server may not start with (case-insensitive). Blocks a
/// user server from masquerading as Oracle/Devboule infrastructure or hijacking dispatch
/// routing (design §5.3). `aspis` covers the `aspis-management` Oracle server key itself.
const RESERVED_NAME_PREFIXES: &[&str] = &["oracle", "devboule", "aspis"];

/// The exact Oracle MCP tool names (the authoritative surface declared in
/// `oracle/server/aspis_mcp.py` `TOOLS`). A user server may not take any of these names,
/// so it can never shadow an Oracle tool in dispatch (design §5.3). Kept in sync with the
/// Python `TOOLS` list; the `oracle_*` / `project_*` entries are also covered by the
/// reserved prefixes, but listing every name makes the guard explicit and self-documenting.
const ORACLE_TOOL_NAMES: &[&str] = &[
    "agent_rules",
    "agent_state",
    "agent_register",
    "agent_heartbeat",
    "spawn_mini_coder",
    "steer_mini_coder",
    "mini_coder_result",
    "visual_check",
    "request_git_push",
    "plan_submit",
    "plan_status",
    "ask_user",
    "project_list",
    "project_get",
    "project_next_task",
    "project_claim_task",
    "project_update_status",
    "project_append_note",
    "project_create_followup",
    "project_create_plan_tasks",
    "provider_credentials_status",
    "cloudflare_list_workers",
    "cloudflare_rotate_worker_secret",
    "scaleway_list_resources",
    "scaleway_resource_action",
    "oracle_ask",
    "oracle_context",
    "project_structure",
    "censor_findings",
    "censor_dispose",
];

/// Max characters of a user server `name`. A name becomes a key in the claude `.mcp.json`
/// map and a token in the codex `-c mcp_servers.<name>.*` flags — keep it short and sane.
const MAX_NAME_LEN: usize = 64;

// ---------------------------------------------------------------------------
// On-the-wire shapes (camelCase over IPC + on disk, matching `.mcp.json`)
// ---------------------------------------------------------------------------

/// One user-declared MCP server (design §2.2). `name` is the routing key; on disk it is
/// the MAP KEY (`{"mcpServers": {"<name>": {...}}}`, the de-facto `.mcp.json` shape), so
/// the serialized RECORD ([`UserMcpServerRecord`]) carries every field EXCEPT `name`. This
/// struct is the flattened in-memory / over-IPC view the commands and launch builders use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMcpServer {
    /// Unique within a scope. Used as the map key on disk and the routing key in dispatch.
    pub name: String,
    /// Transport — `"stdio"` only in v1 (any other value rejected at add / skipped on read).
    pub transport: String,
    /// The child-process command to launch (e.g. `python`, `node`, an absolute path).
    pub command: String,
    /// Arguments passed to `command`. Defaults to empty.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables for the child. `BTreeMap` so on-disk key order is
    /// deterministic (stable git diffs). Defaults to empty.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Soft-disable without deleting. Defaults to true.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Default for [`UserMcpServer::enabled`] / [`UserMcpServerRecord::enabled`]: a present
/// record with no `enabled` key is ENABLED.
fn default_enabled() -> bool {
    true
}

/// The per-server RECORD as stored under the `mcpServers` map (everything but the name,
/// which is the map key). Splitting this from [`UserMcpServer`] keeps the on-disk shape
/// exactly `{"mcpServers": {"<name>": {transport, command, args, env, enabled}}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMcpServerRecord {
    transport: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

/// The full config file shape: `{"mcpServers": {"<name>": {...}}}`. `BTreeMap` so the
/// on-disk key order is deterministic (byte-stable round-trips, clean git diffs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct UserMcpConfig {
    mcp_servers: BTreeMap<String, UserMcpServerRecord>,
}

impl UserMcpServer {
    /// Split this flattened server into `(name, record)` for serialization under the
    /// `mcpServers` map.
    fn into_keyed(self) -> (String, UserMcpServerRecord) {
        (
            self.name,
            UserMcpServerRecord {
                transport: self.transport,
                command: self.command,
                args: self.args,
                env: self.env,
                enabled: self.enabled,
            },
        )
    }
}

/// Re-join a `(name, record)` map entry into the flattened [`UserMcpServer`] view.
fn server_from_keyed(name: String, record: UserMcpServerRecord) -> UserMcpServer {
    UserMcpServer {
        name,
        transport: record.transport,
        command: record.command,
        args: record.args,
        env: record.env,
        enabled: record.enabled,
    }
}

// ---------------------------------------------------------------------------
// Scope + path resolution
// ---------------------------------------------------------------------------

/// Which config file a command targets. Mirrors the project-skill commands' scope arg
/// shape; serialized lowercase over IPC (`"global"` | `"project"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpScope {
    /// `<app-data>/user-mcp-servers.json` — every project.
    Global,
    /// `<project_root>/.devboule/mcp-servers.json` — this repo only.
    Project,
}

/// The global config file path: `<app-data>/user-mcp-servers.json`. Resolves the app-data
/// dir via the SAME Tauri `app_data_dir()` the rest of the backend uses for global state
/// (`roles.rs`, `agents.rs`, etc.) — never a hardcoded home path. Creates the app-data
/// directory if absent so the first write succeeds.
fn global_config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "App data directory is unavailable.".to_string())?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create the app data directory: {e}"))?;
    Ok(dir.join("user-mcp-servers.json"))
}

/// The project config file path: `<project_root>/.devboule/mcp-servers.json`, resolved
/// INSIDE the project root with `..`-traversal rejected (design A.1 path-safety). The
/// project root must already exist (canonicalize collapses `.`/`..`/symlinks so the
/// under-root assertion is meaningful). The `.devboule` directory is created on the WRITE
/// path only — see [`ensure_project_config_dir`]. Returns the absolute, contained path.
fn project_config_path(project_root: &str) -> Result<PathBuf, String> {
    let canonical = canonical_project_root(project_root)?;
    let target = canonical.join(".devboule").join("mcp-servers.json");
    // Defense in depth: the relative path we join is a fixed literal with no traversal,
    // but assert the join stays under the canonical root anyway (catches a future change
    // and is the explicit guard the acceptance criteria require).
    if !target.starts_with(&canonical) {
        return Err("project MCP config path escapes the project root".to_string());
    }
    Ok(target)
}

/// Canonicalize + validate the project root: it must be a non-empty, existing directory.
/// Canonicalizing collapses any `.`/`..`/symlink so a caller-supplied `..` cannot escape:
/// the resolved path is the real directory, and every file path is then built UNDER it.
fn canonical_project_root(project_root: &str) -> Result<PathBuf, String> {
    if project_root.trim().is_empty() {
        return Err("project root path must not be empty".to_string());
    }
    // Reject a raw `..` component up front with a clear, stable error (the acceptance
    // criterion). Canonicalize would also neutralize it, but an explicit reject gives the
    // caller a precise message instead of a generic "unreadable".
    let raw = PathBuf::from(project_root);
    if raw
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("project root path must not contain '..'".to_string());
    }
    let canonical = std::fs::canonicalize(&raw).map_err(|e| {
        // Detail (incl. the absolute path) to the process log only; the wire error is a
        // stable short label so no FS layout leaks to the renderer.
        eprintln!(
            "[user-mcp] project root unreadable: {} ({e})",
            raw.display()
        );
        "project root does not exist or is unreadable".to_string()
    })?;
    if !canonical.is_dir() {
        return Err("project root is not a directory".to_string());
    }
    Ok(canonical)
}

/// Create `<project_root>/.devboule/` and return the contained `mcp-servers.json` path
/// for a WRITE. Same containment as [`project_config_path`].
fn ensure_project_config_dir(project_root: &str) -> Result<PathBuf, String> {
    let path = project_config_path(project_root)?;
    let dir = path
        .parent()
        .ok_or_else(|| "project MCP config path has no parent".to_string())?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create .devboule folder: {e}"))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Read (fail-open) + write (atomic)
// ---------------------------------------------------------------------------

/// Bounded, fail-open read of an MCP-servers config file at `path`. Missing / oversized /
/// non-regular / unreadable / malformed ⇒ `UserMcpConfig::default()` (empty). A single
/// invalid ENTRY (bad transport, empty command, reserved name) is skipped with a warning
/// and the rest are kept — so one bad hand-edit never blanks the whole list or crashes a
/// launch. NEVER returns an error: the launch path must always proceed.
fn read_config_file(path: &Path) -> UserMcpConfig {
    // Regular-file gate: a FIFO/device at the path would BLOCK File::open forever (the byte
    // cap bounds the read, not the open). metadata follows the path and does not block.
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            eprintln!(
                "[user-mcp] config path is not a regular file, ignoring: {}",
                path.display()
            );
            return UserMcpConfig::default();
        }
        // Missing is the normal case (no servers configured) — silent, not a warning.
        Err(_) => return UserMcpConfig::default(),
    }
    let mut handle = match std::fs::File::open(path) {
        Ok(f) => f.take(MAX_CONFIG_BYTES + 1),
        Err(e) => {
            eprintln!("[user-mcp] config unreadable, ignoring: {} ({e})", path.display());
            return UserMcpConfig::default();
        }
    };
    let mut buf = Vec::new();
    if let Err(e) = handle.read_to_end(&mut buf) {
        eprintln!("[user-mcp] config read failed, ignoring: {} ({e})", path.display());
        return UserMcpConfig::default();
    }
    if buf.len() as u64 > MAX_CONFIG_BYTES {
        eprintln!(
            "[user-mcp] config too large (> {MAX_CONFIG_BYTES} bytes), ignoring: {}",
            path.display()
        );
        return UserMcpConfig::default();
    }
    let mut config: UserMcpConfig = match serde_json::from_slice(&buf) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[user-mcp] config is not valid JSON, treating as empty: {} ({e})",
                path.display()
            );
            return UserMcpConfig::default();
        }
    };
    // Drop individually-invalid entries with a per-entry warning, keep the valid rest. This
    // is the "an individual invalid entry → skip with a warning" fail-open rule. We validate
    // the SAME way an add would (transport + name guard + command), so a hand-edited file
    // cannot smuggle a server past the rules an in-app add would have enforced.
    config.mcp_servers.retain(|name, record| {
        match validate_entry(name, record) {
            Ok(()) => true,
            Err(reason) => {
                eprintln!(
                    "[user-mcp] skipping invalid server '{name}' in {}: {reason}",
                    path.display()
                );
                false
            }
        }
    });
    config
}

/// Serialize + atomically write `config` to `path`. Reuses the shared `atomic_write`
/// (temp + atomic rename) so there is ONE write implementation. Pretty JSON for a
/// hand-readable, git-friendly file.
fn write_config_file(path: &Path, config: &UserMcpConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("could not serialize MCP servers config: {e}"))?;
    atomic_write(path, &json, "mcp-servers.json")
}

// ---------------------------------------------------------------------------
// Validation / guards
// ---------------------------------------------------------------------------

/// Validate a server NAME (design §5.3 name guard): non-empty, within length, no reserved
/// prefix (`oracle`/`devboule`/`aspis`, case-insensitive), and not an exact Oracle tool
/// name. Enforced at ADD time and again on READ (so a hand-edited reserved name is skipped,
/// never injected). A reserved name can never shadow the Oracle in dispatch.
fn validate_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("server name must not be empty".to_string());
    }
    if trimmed != name {
        return Err("server name must not have leading or trailing whitespace".to_string());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "server name too long ({} chars > {MAX_NAME_LEN} max)",
            name.len()
        ));
    }
    // CONFIG-INJECTION GUARD: the name becomes a JSON object key in the claude `.mcp.json`
    // AND a dotted TOML key in the codex `-c mcp_servers.<name>.command` flag. Restrict it
    // to a safe charset (ASCII alphanumeric, `-`, `_`) so it cannot carry a `.`/`=`/quote/
    // space that would split the codex dotted key or otherwise corrupt the generated config.
    // This is stricter than dispatch needs but matches the codebase's path-safety discipline.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "server name '{name}' may only contain ASCII letters, digits, '-' and '_'"
        ));
    }
    let lower = name.to_ascii_lowercase();
    for prefix in RESERVED_NAME_PREFIXES {
        if lower.starts_with(prefix) {
            return Err(format!(
                "server name '{name}' is reserved (must not start with '{prefix}')"
            ));
        }
    }
    // Exact Oracle tool names (those not already caught by a reserved prefix, e.g.
    // `spawn_mini_coder`, `plan_submit`, the `project_*`/`censor_*` tools).
    if ORACLE_TOOL_NAMES.iter().any(|t| t.eq_ignore_ascii_case(name)) {
        return Err(format!(
            "server name '{name}' collides with a reserved Oracle tool name"
        ));
    }
    Ok(())
}

/// Validate the ENV map of a server (CONFIG-INJECTION GUARD). Each env value is later
/// TOML-escaped (codex) / JSON-escaped (claude), but the env KEY is interpolated RAW into
/// the codex dotted key path `mcp_servers.<name>.env.<key>=...`. A key carrying a `.`
/// would create wrong TOML nesting; `=`/`\n`/`[`/`]`/quotes/whitespace could split the
/// dotted key or inject/misparse the generated codex config (the §9 shared-repo threat: a
/// malicious `.devboule/mcp-servers.json` committed to a repo). So restrict every env key
/// to the STANDARD env-var charset `[A-Za-z0-9_]`, non-empty. Enforced at ADD time and
/// again on READ (a hand-edited bad key is skipped, never injected).
///
/// F3 (backend guard): also reject env VALUES that contain control characters (including
/// `\r`, `\n`, `\x01`, etc.). The frontend strips trailing `\r` from pasted values, but
/// the hand-edit path bypasses the frontend entirely. A control char in a value is never
/// a legitimate environment variable and mirrors the existing `validate_args` guard.
fn validate_env(env: &BTreeMap<String, String>) -> Result<(), String> {
    for (key, value) in env {
        if key.is_empty() {
            return Err("environment variable name must not be empty".to_string());
        }
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!(
                "environment variable name '{key}' may only contain ASCII letters, digits and '_'"
            ));
        }
        // Mirror validate_args: control chars (including \r and \n) are never
        // legitimate in an env value and indicate a hand-edited hostile payload.
        if value.chars().any(|c| c.is_control()) {
            return Err(format!(
                "environment variable '{key}' value must not contain control characters or newlines"
            ));
        }
    }
    Ok(())
}

/// Validate the ARGS of a server (defense in depth). Args are TOML-escaped (codex) /
/// JSON-escaped (claude) before they reach a launch line, so a metacharacter cannot break
/// out — but a control char / newline in an arg is never a legitimate launch argument and
/// is exactly the kind of payload a hostile shared-repo config would carry, so reject it.
fn validate_args(args: &[String]) -> Result<(), String> {
    for arg in args {
        if arg.chars().any(|c| c.is_control()) {
            return Err("server arg must not contain control characters or newlines".to_string());
        }
    }
    Ok(())
}

/// Validate a server's transport: `stdio` only in v1. Any other value is rejected with a
/// clear, actionable error (design §2.2).
fn validate_transport(transport: &str) -> Result<(), String> {
    if transport == STDIO_TRANSPORT {
        Ok(())
    } else {
        Err(format!(
            "unsupported transport '{transport}' (only '{STDIO_TRANSPORT}' is supported in v1)"
        ))
    }
}

/// Validate a full server entry as stored on disk (name + record). Used by the fail-open
/// reader to drop a single bad entry, and (via [`validate_server`]) by the add command.
fn validate_entry(name: &str, record: &UserMcpServerRecord) -> Result<(), String> {
    validate_name(name)?;
    validate_transport(&record.transport)?;
    if record.command.trim().is_empty() {
        return Err("server command must not be empty".to_string());
    }
    validate_args(&record.args)?;
    validate_env(&record.env)?;
    Ok(())
}

/// Validate a flattened [`UserMcpServer`] at ADD time. Same rules as [`validate_entry`].
fn validate_server(server: &UserMcpServer) -> Result<(), String> {
    validate_name(&server.name)?;
    validate_transport(&server.transport)?;
    if server.command.trim().is_empty() {
        return Err("server command must not be empty".to_string());
    }
    validate_args(&server.args)?;
    validate_env(&server.env)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Merge — the launch-injection entry point
// ---------------------------------------------------------------------------

/// The merged, enabled set of user MCP servers for a launch (design §2.1, §5.1):
/// `global ∪ project`, PROJECT WINS on a name collision, then filter to `enabled == true`.
/// Sorted by name for deterministic launch-config output (byte-stable across runs).
///
/// FAIL-OPEN end to end: every read returns empty on any failure, so a missing/broken
/// config never blocks a coder launch. The Oracle (`aspis-management`) is NOT included —
/// the launch builders add it separately, always first.
///
/// `project_root` is the project directory; an empty/unreadable root contributes no project
/// servers (global still applies). This is the ONE function the MAIN-coder launch path calls.
pub(crate) fn merged_servers(app: &tauri::AppHandle, project_root: &Path) -> Vec<UserMcpServer> {
    // Global first, project overlaid on top so project keys WIN on collision.
    let mut merged: BTreeMap<String, UserMcpServerRecord> = BTreeMap::new();
    if let Ok(global_path) = global_config_path(app) {
        merged.extend(read_config_file(&global_path).mcp_servers);
    }
    // Project root may not be canonicalizable (e.g. a transient path) — fail open to "no
    // project servers" rather than erroring the whole launch.
    if let Some(root_str) = project_root.to_str() {
        if let Ok(project_path) = project_config_path(root_str) {
            // `extend` overwrites existing keys ⇒ project wins on collision.
            merged.extend(read_config_file(&project_path).mcp_servers);
        }
    }
    merged
        .into_iter()
        .map(|(name, record)| server_from_keyed(name, record))
        .filter(|s| s.enabled)
        .collect()
}

// ---------------------------------------------------------------------------
// Command impls (no Tauri scaffolding ⇒ unit-testable) + Tauri command wrappers
// ---------------------------------------------------------------------------
// Same split as project_skill.rs: the `#[tauri::command]` wrappers do only the
// Tauri-coupled concerns (`ensure_unlocked`, the write guard, resolving the global path
// from the AppHandle) and delegate to a path-based impl the tests drive directly.

/// Resolve the config file path for a scope. Global needs the AppHandle (app-data dir);
/// project needs `project_root`. Returns a clear error if `project_root` is missing for a
/// project-scoped call.
fn resolve_path(
    app: &tauri::AppHandle,
    scope: McpScope,
    project_root: Option<&str>,
) -> Result<PathBuf, String> {
    match scope {
        McpScope::Global => global_config_path(app),
        McpScope::Project => {
            let root = project_root
                .ok_or_else(|| "project_root is required for project scope".to_string())?;
            project_config_path(root)
        }
    }
}

/// List the servers configured in `scope` (the flattened view, sorted by name). Returns
/// the on-disk list AS-READ (fail-open: invalid entries already dropped by the reader).
/// `enabled` is preserved so the UI can render the toggle state. Does NOT merge scopes —
/// the panel manages one scope at a time.
fn list_impl(path: &Path) -> Vec<UserMcpServer> {
    read_config_file(path)
        .mcp_servers
        .into_iter()
        .map(|(name, record)| server_from_keyed(name, record))
        .collect()
}

/// Add a server to `scope`. WRITE HELPER: it TRUSTS its input is already validated — the
/// `user_mcp_add` command runs `validate_server` BEFORE creating any `.devboule` directory
/// (so a rejected add leaves no on-disk trace), making a second check here pure duplication.
/// A read-modify-write that PRESERVES every other entry. Returns an error only if a server
/// with the same name already exists in this scope (use remove-then-add or set_enabled to
/// change one) — that is a STATE check (needs the on-disk file), not input validation.
fn add_impl(path: &Path, server: UserMcpServer) -> Result<(), String> {
    let mut config = read_config_file(path);
    if config.mcp_servers.contains_key(&server.name) {
        return Err(format!(
            "a server named '{}' already exists in this scope",
            server.name
        ));
    }
    let (name, record) = server.into_keyed();
    config.mcp_servers.insert(name, record);
    write_config_file(path, &config)
}

/// Remove a server by name from `scope`. A no-op (Ok) if the name is absent — removing an
/// already-gone server is idempotent, not an error. Read-modify-write preserving the rest.
fn remove_impl(path: &Path, name: &str) -> Result<(), String> {
    let mut config = read_config_file(path);
    if config.mcp_servers.remove(name).is_none() {
        return Ok(());
    }
    write_config_file(path, &config)
}

/// Toggle a server's `enabled` flag in `scope` (read-modify-write preserving the rest).
/// Errors if the named server is absent (you cannot toggle what is not declared).
fn set_enabled_impl(path: &Path, name: &str, enabled: bool) -> Result<(), String> {
    let mut config = read_config_file(path);
    match config.mcp_servers.get_mut(name) {
        Some(record) => record.enabled = enabled,
        None => return Err(format!("no server named '{name}' in this scope")),
    }
    write_config_file(path, &config)
}

/// List the user MCP servers configured in `scope` (one scope, no merge). For project
/// scope, `project_root` is required.
#[tauri::command]
pub fn user_mcp_list(
    state: State<'_, BackendState>,
    app: tauri::AppHandle,
    scope: McpScope,
    project_root: Option<String>,
) -> Result<Vec<UserMcpServer>, String> {
    state.ensure_unlocked()?;
    let path = resolve_path(&app, scope, project_root.as_deref())?;
    Ok(list_impl(&path))
}

/// Add a user MCP server to `scope`. Enforces the name guard, transport, and command
/// checks at add time (design §5.3). For project scope, `project_root` is required and the
/// `.devboule` folder is created.
#[tauri::command]
pub fn user_mcp_add(
    state: State<'_, BackendState>,
    app: tauri::AppHandle,
    scope: McpScope,
    project_root: Option<String>,
    server: UserMcpServer,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    // Validate BEFORE creating any directory so a rejected add leaves no `.devboule` trace.
    validate_server(&server)?;
    let path = match scope {
        McpScope::Global => global_config_path(&app)?,
        McpScope::Project => {
            let root = project_root
                .as_deref()
                .ok_or_else(|| "project_root is required for project scope".to_string())?;
            ensure_project_config_dir(root)?
        }
    };
    add_impl(&path, server)
}

/// Remove a user MCP server from `scope` by name (idempotent). For project scope,
/// `project_root` is required.
#[tauri::command]
pub fn user_mcp_remove(
    state: State<'_, BackendState>,
    app: tauri::AppHandle,
    scope: McpScope,
    project_root: Option<String>,
    name: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let path = resolve_path(&app, scope, project_root.as_deref())?;
    remove_impl(&path, &name)
}

/// Enable/disable a user MCP server in `scope` without deleting it. For project scope,
/// `project_root` is required.
#[tauri::command]
pub fn user_mcp_set_enabled(
    state: State<'_, BackendState>,
    app: tauri::AppHandle,
    scope: McpScope,
    project_root: Option<String>,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let path = resolve_path(&app, scope, project_root.as_deref())?;
    set_enabled_impl(&path, &name, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, unique temp dir (created so canonicalize succeeds). Caller removes it.
    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aspis-usermcp-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn srv(name: &str, command: &str, enabled: bool) -> UserMcpServer {
        UserMcpServer {
            name: name.to_string(),
            transport: "stdio".to_string(),
            command: command.to_string(),
            args: vec!["-m".to_string(), "mod".to_string()],
            env: BTreeMap::new(),
            enabled,
        }
    }

    /// Mirror the `user_mcp_add` COMMAND end to end: validate the input, THEN write. The
    /// command validates before any directory/file is touched and `add_impl` now trusts its
    /// input (FIX 4 dedupe), so the validation tests must exercise this command-shaped path
    /// to assert a bad server is rejected. Path-based so no AppHandle is needed.
    fn add_validated(path: &Path, server: UserMcpServer) -> Result<(), String> {
        validate_server(&server)?;
        add_impl(path, server)
    }

    /// Merge two config files (global + project) the SAME way `merged_servers` does, but
    /// against explicit paths so the test needs no AppHandle. Mirrors the real merge:
    /// global first, project overlaid (project wins), then enabled filter + sort.
    fn merge_files(global: &Path, project: &Path) -> Vec<UserMcpServer> {
        let mut merged: BTreeMap<String, UserMcpServerRecord> = BTreeMap::new();
        merged.extend(read_config_file(global).mcp_servers);
        merged.extend(read_config_file(project).mcp_servers);
        merged
            .into_iter()
            .map(|(n, r)| server_from_keyed(n, r))
            .filter(|s| s.enabled)
            .collect()
    }

    #[test]
    fn round_trip_write_then_read_is_byte_stable() {
        let dir = fresh_dir("roundtrip");
        let path = dir.join("user-mcp-servers.json");
        let mut config = UserMcpConfig::default();
        let (n, r) = srv("my-db", "python", true).into_keyed();
        config.mcp_servers.insert(n, r);
        write_config_file(&path, &config).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        // Read back, write again — the bytes must be identical (deterministic BTreeMap order).
        let read_back = read_config_file(&path);
        write_config_file(&path, &read_back).unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "write→read→write must be byte-stable");
        // And the round-tripped server matches field-for-field.
        let listed = list_impl(&path);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], srv("my-db", "python", true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_project_wins_on_name_collision() {
        let dir = fresh_dir("collision");
        let global = dir.join("global.json");
        let project = dir.join("project.json");
        add_impl(&global, srv("shared", "global-cmd", true)).unwrap();
        add_impl(&project, srv("shared", "project-cmd", true)).unwrap();
        let merged = merge_files(&global, &project);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "shared");
        assert_eq!(merged[0].command, "project-cmd", "project entry must win");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_excludes_disabled_entries() {
        let dir = fresh_dir("disabled");
        let global = dir.join("global.json");
        let project = dir.join("project.json");
        add_impl(&global, srv("on", "a", true)).unwrap();
        add_impl(&global, srv("off", "b", false)).unwrap();
        let merged = merge_files(&global, &project);
        let names: Vec<&str> = merged.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["on"], "disabled entries excluded from merged output");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_unions_global_only_and_project_only() {
        let dir = fresh_dir("union");
        let global = dir.join("global.json");
        let project = dir.join("project.json");
        add_impl(&global, srv("g-only", "a", true)).unwrap();
        add_impl(&project, srv("p-only", "b", true)).unwrap();
        let merged = merge_files(&global, &project);
        let names: Vec<&str> = merged.iter().map(|s| s.name.as_str()).collect();
        // Sorted by name (BTreeMap): "g-only" < "p-only".
        assert_eq!(names, vec!["g-only", "p-only"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_collision_loser_disabled_does_not_resurrect_global() {
        // PROJECT wins on collision REGARDLESS of enabled: a project entry that DISABLES a
        // name the global enables must remove it from the merged set (not fall back to global).
        let dir = fresh_dir("collision-disable");
        let global = dir.join("global.json");
        let project = dir.join("project.json");
        add_impl(&global, srv("dup", "g", true)).unwrap();
        add_impl(&project, srv("dup", "p", false)).unwrap();
        let merged = merge_files(&global, &project);
        assert!(merged.is_empty(), "project's disabled entry wins and removes the name");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_fails_open_on_missing_and_malformed_and_oversized() {
        let dir = fresh_dir("failopen");
        // Missing ⇒ empty.
        assert!(list_impl(&dir.join("missing.json")).is_empty());
        // Malformed JSON ⇒ empty (no crash).
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ this is not json").unwrap();
        assert!(list_impl(&bad).is_empty());
        // Oversized ⇒ empty.
        let big = dir.join("big.json");
        let mut content = String::from("{\"mcpServers\":{\"x\":{\"transport\":\"stdio\",\"command\":\"c\",\"_pad\":\"");
        content.push_str(&"z".repeat((MAX_CONFIG_BYTES as usize) + 1024));
        content.push_str("\"}}}");
        std::fs::write(&big, &content).unwrap();
        assert!(list_impl(&big).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_skips_a_single_invalid_entry_and_keeps_the_rest() {
        let dir = fresh_dir("partial");
        let path = dir.join("c.json");
        // Hand-write a file with one good server and several invalid ones (bad transport,
        // reserved name, empty command). Only the good one survives.
        let raw = r#"{
            "mcpServers": {
                "good": { "transport": "stdio", "command": "python" },
                "bad-transport": { "transport": "http", "command": "python" },
                "oracle-evil": { "transport": "stdio", "command": "python" },
                "empty-cmd": { "transport": "stdio", "command": "" }
            }
        }"#;
        std::fs::write(&path, raw).unwrap();
        let listed = list_impl(&path);
        let names: Vec<&str> = listed.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["good"], "only the valid entry survives");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_reserved_names() {
        let dir = fresh_dir("reserved");
        let path = dir.join("c.json");
        for bad in ["oracle", "Oracle", "devboule", "aspis-management", "oracle_ask"] {
            let err = add_validated(&path, srv(bad, "python", true)).unwrap_err();
            assert!(
                err.contains("reserved") || err.contains("Oracle tool"),
                "name '{bad}' should be rejected, got: {err}"
            );
        }
        // An exact Oracle tool name with no reserved prefix is also rejected.
        for tool in ["spawn_mini_coder", "plan_submit", "project_get", "censor_dispose"] {
            let err = add_validated(&path, srv(tool, "python", true)).unwrap_err();
            assert!(err.contains("Oracle tool"), "tool name '{tool}' should be rejected, got: {err}");
        }
        // Nothing was written by any rejected add.
        assert!(list_impl(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_unsafe_name_chars() {
        let dir = fresh_dir("namechars");
        let path = dir.join("c.json");
        // A `.` would split the codex dotted key; `=`/space/quote would corrupt tokens.
        for bad in ["my.db", "my db", "a=b", "weird\"name", "tab\tname"] {
            let err = add_validated(&path, srv(bad, "python", true)).unwrap_err();
            assert!(
                err.contains("ASCII letters") || err.contains("whitespace"),
                "name '{bad}' should be rejected, got: {err}"
            );
        }
        // The conventional shape (letters, digits, '-', '_') is accepted.
        add_validated(&path, srv("my-db_2", "python", true)).unwrap();
        assert_eq!(list_impl(&path).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_non_stdio_transport() {
        let dir = fresh_dir("transport");
        let path = dir.join("c.json");
        let mut server = srv("my-http", "python", true);
        server.transport = "http".to_string();
        let err = add_validated(&path, server).unwrap_err();
        assert!(err.contains("transport"), "unexpected error: {err}");
        assert!(list_impl(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_duplicate_and_empty_command() {
        let dir = fresh_dir("dup-empty");
        let path = dir.join("c.json");
        add_validated(&path, srv("my-db", "python", true)).unwrap();
        // Duplicate is a STATE check enforced by add_impl itself (needs the on-disk file).
        let dup = add_impl(&path, srv("my-db", "node", true)).unwrap_err();
        assert!(dup.contains("already exists"), "unexpected: {dup}");
        // Empty command is INPUT validation — now enforced only by validate_server (the
        // command path), so exercise it via the command-shaped add_validated helper.
        let empty = add_validated(&path, srv("blank", "", true)).unwrap_err();
        assert!(empty.contains("command"), "unexpected: {empty}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_is_idempotent_and_preserves_others() {
        let dir = fresh_dir("remove");
        let path = dir.join("c.json");
        add_impl(&path, srv("a", "x", true)).unwrap();
        add_impl(&path, srv("b", "y", true)).unwrap();
        remove_impl(&path, "a").unwrap();
        remove_impl(&path, "a").unwrap(); // idempotent — no error on second remove
        let names: Vec<String> = list_impl(&path).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_enabled_toggles_and_errors_on_absent() {
        let dir = fresh_dir("toggle");
        let path = dir.join("c.json");
        add_impl(&path, srv("a", "x", true)).unwrap();
        set_enabled_impl(&path, "a", false).unwrap();
        assert!(!list_impl(&path)[0].enabled);
        set_enabled_impl(&path, "a", true).unwrap();
        assert!(list_impl(&path)[0].enabled);
        let err = set_enabled_impl(&path, "ghost", false).unwrap_err();
        assert!(err.contains("no server named"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_unsafe_env_keys_and_accepts_normal_key() {
        let dir = fresh_dir("envkeys");
        let path = dir.join("c.json");
        // Each of these env KEYS would corrupt the codex dotted key path
        // `mcp_servers.<name>.env.<key>=...` (a `.` nests wrong; `=`/space/newline split
        // or inject), and an empty key is meaningless. All must be rejected at ADD time.
        for bad_key in ["FOO.BAR", "FOO=X", "FOO BAR", "FOO\nBAR", ""] {
            let mut server = srv("envtest", "python", true);
            server.env = BTreeMap::new();
            server.env.insert(bad_key.to_string(), "v".to_string());
            let err = add_validated(&path, server).unwrap_err();
            assert!(
                err.contains("environment variable name"),
                "env key {bad_key:?} should be rejected, got: {err}"
            );
        }
        // A conventional env-var name is accepted and round-trips.
        let mut server = srv("envok", "python", true);
        server.env = BTreeMap::new();
        server.env.insert("MY_KEY".to_string(), "value".to_string());
        add_validated(&path, server).unwrap();
        let listed = list_impl(&path);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].env.get("MY_KEY").map(String::as_str), Some("value"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_skips_entry_with_unsafe_env_key() {
        // A hand-edited file smuggling a `.`-bearing env key (wrong TOML nesting) is dropped
        // by the fail-open reader, exactly like a reserved name — never injected into a launch.
        let dir = fresh_dir("envkey-read");
        let path = dir.join("c.json");
        let raw = r#"{
            "mcpServers": {
                "good": { "transport": "stdio", "command": "python", "env": { "MY_KEY": "ok" } },
                "evil-env": { "transport": "stdio", "command": "python", "env": { "FOO.BAR": "x" } }
            }
        }"#;
        std::fs::write(&path, raw).unwrap();
        let names: Vec<String> = list_impl(&path).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["good".to_string()], "the bad-env entry is skipped on read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F3 (backend): control chars in env VALUES must be rejected at ADD time and
    /// skipped on READ (the hand-edit / shared-repo bypass path).
    #[test]
    fn add_rejects_env_value_with_control_chars_and_read_skips_it() {
        let dir = fresh_dir("envval-ctrl");
        let path = dir.join("c.json");

        // \r in a value (Windows paste via hand-edit).
        let mut server = srv("test-cr", "python", true);
        server.env.insert("KEY".to_string(), "val\r".to_string());
        let err = add_validated(&path, server).unwrap_err();
        assert!(
            err.contains("control characters"),
            "\\r in env value should be rejected, got: {err}"
        );

        // \x01 (arbitrary control char / hostile payload).
        let mut server = srv("test-ctrl", "python", true);
        server.env.insert("KEY".to_string(), "val\x01".to_string());
        let err = add_validated(&path, server).unwrap_err();
        assert!(
            err.contains("control characters"),
            "\\x01 in env value should be rejected, got: {err}"
        );

        // \n in a value (newline injection).
        let mut server = srv("test-nl", "python", true);
        server.env.insert("KEY".to_string(), "val\ninjected".to_string());
        let err = add_validated(&path, server).unwrap_err();
        assert!(
            err.contains("control characters"),
            "\\n in env value should be rejected, got: {err}"
        );

        // Nothing was written by any rejected add.
        assert!(list_impl(&path).is_empty(), "no entry should have been stored");

        // A clean value is accepted.
        let mut server = srv("test-ok", "python", true);
        server.env.insert("KEY".to_string(), "clean-value".to_string());
        add_validated(&path, server).unwrap();
        let listed = list_impl(&path);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].env.get("KEY").map(String::as_str), Some("clean-value"));

        // READ skips a hand-edited entry whose env value contains \r (the bypass path).
        // Write a raw JSON with a \r in the env value alongside a clean server.
        let raw = "{\"mcpServers\":{\"clean\":{\"transport\":\"stdio\",\"command\":\"python\",\"env\":{\"K\":\"ok\"}},\"dirty\":{\"transport\":\"stdio\",\"command\":\"python\",\"env\":{\"K\":\"val\\r\"}}}}";
        std::fs::write(&path, raw).unwrap();
        let names: Vec<String> = list_impl(&path).into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec!["clean".to_string()],
            "the \\r-value entry must be skipped on read"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_arg_with_newline() {
        // Args are TOML/JSON-escaped downstream, but a newline in an arg is never legitimate
        // (defense in depth) — reject it at ADD time and on read.
        let dir = fresh_dir("argctrl");
        let path = dir.join("c.json");
        let mut server = srv("argtest", "python", true);
        server.args = vec!["--flag".to_string(), "line1\nline2".to_string()];
        let err = add_validated(&path, server).unwrap_err();
        assert!(err.contains("control characters"), "newline arg should be rejected, got: {err}");
        // And a file carrying such an arg is dropped on read.
        let raw = "{ \"mcpServers\": { \"x\": { \"transport\": \"stdio\", \"command\": \"python\", \"args\": [\"a\\nb\"] } } }";
        std::fs::write(&path, raw).unwrap();
        assert!(list_impl(&path).is_empty(), "the newline-arg entry is skipped on read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_config_path_rejects_dotdot_traversal() {
        // A project root carrying `..` is rejected up front (path safety). Use a real dir
        // so we know the error is from the `..` guard, not a missing-dir error.
        let base = fresh_dir("traversal");
        let with_dotdot = format!("{}/sub/../..", base.display());
        let err = project_config_path(&with_dotdot).unwrap_err();
        assert!(err.contains(".."), "expected a '..' rejection, got: {err}");
        // The plain (no-`..`) path resolves fine and stays under the root.
        let ok = project_config_path(base.to_str().unwrap()).unwrap();
        assert!(ok.starts_with(std::fs::canonicalize(&base).unwrap()));
        assert!(ok.ends_with("mcp-servers.json"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// True iff `name` is caught by a reserved name PREFIX (the prefix guard already blocks
    /// it, so the exact list need not also carry it). Mirrors `validate_name`'s prefix check.
    fn caught_by_reserved_prefix(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        RESERVED_NAME_PREFIXES.iter().any(|p| lower.starts_with(p))
    }

    /// Extract the Oracle tool names registered in `oracle/server/aspis_mcp.py`. The
    /// registration shape is a top-level `TOOLS = [ {"name": "...", ...}, ... ]` list literal
    /// whose closing `]` is a line starting at column 0. We isolate that block and match each
    /// `"name": "<value>"` — every tool dict's first key is `"name"`, and no nested parameter
    /// dict inside the block uses a `"name"` key, so this conservative parse yields exactly the
    /// tool names (verified against an AST parse). DOCUMENTED ASSUMPTION: if a future refactor
    /// nests a `"name"` key inside a tool's parameters, this would over-collect — which is the
    /// SAFE direction (it can only demand MORE names be reserved, never fewer), so the tripwire
    /// stays sound. The goal is a drift detector, not a perfect parser.
    fn oracle_tool_names_from_python(src: &str) -> Vec<String> {
        // Isolate the `TOOLS = [ ... ]` block: from the line starting with `TOOLS = [` to the
        // first subsequent line whose first char is `]` (the list close at column 0).
        let mut lines = src.lines();
        let mut block = String::new();
        let mut in_block = false;
        for line in lines.by_ref() {
            if !in_block {
                if line.starts_with("TOOLS = [") {
                    in_block = true;
                }
                continue;
            }
            if line.starts_with(']') {
                break;
            }
            block.push_str(line);
            block.push('\n');
        }
        assert!(in_block, "could not locate the `TOOLS = [` block in aspis_mcp.py");
        // Match `"name": "<value>"` (the tool key). Tiny hand-roll so the test needs no regex
        // crate: scan for the literal `"name"`, skip a `:` and optional spaces, then read the
        // next double-quoted string.
        let mut names = Vec::new();
        let bytes = block.as_bytes();
        let needle = b"\"name\"";
        let mut i = 0usize;
        while let Some(pos) = block[i..].find("\"name\"") {
            let mut j = i + pos + needle.len();
            // skip spaces, expect a colon, skip spaces.
            while j < bytes.len() && bytes[j] == b' ' { j += 1; }
            if j < bytes.len() && bytes[j] == b':' {
                j += 1;
                while j < bytes.len() && bytes[j] == b' ' { j += 1; }
                if j < bytes.len() && bytes[j] == b'"' {
                    j += 1;
                    let start = j;
                    while j < bytes.len() && bytes[j] != b'"' { j += 1; }
                    if j <= bytes.len() {
                        names.push(block[start..j].to_string());
                    }
                }
            }
            i = j.max(i + pos + 1);
        }
        names
    }

    #[test]
    fn oracle_tool_names_list_has_no_drift_from_python() {
        // DRIFT TRIPWIRE (design §5.3): if a future Oracle tool is added to aspis_mcp.py's
        // `TOOLS` without being added here (the Rust `ORACLE_TOOL_NAMES` list) AND without a
        // reserved prefix, its bare name (e.g. `visual_check`) would be free for a user server
        // to claim and shadow in dispatch. This test reads the authoritative Python source and
        // fails if any registered tool name is neither in the static list nor caught by a prefix.
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("CARGO_MANIFEST_DIR (src-tauri) must have a parent (repo root)");
        let py_path = repo_root.join("oracle").join("server").join("aspis_mcp.py");
        let src = std::fs::read_to_string(&py_path)
            .unwrap_or_else(|e| panic!("could not read {}: {e}", py_path.display()));
        let registered = oracle_tool_names_from_python(&src);
        assert!(
            !registered.is_empty(),
            "parsed ZERO tool names from {} — the parser or the file shape changed",
            py_path.display()
        );
        let mut missing: Vec<String> = Vec::new();
        for tool in &registered {
            let in_list = ORACLE_TOOL_NAMES.iter().any(|t| t.eq_ignore_ascii_case(tool));
            if !in_list && !caught_by_reserved_prefix(tool) {
                missing.push(tool.clone());
            }
        }
        assert!(
            missing.is_empty(),
            "Oracle tool(s) {missing:?} are registered in aspis_mcp.py but NOT in \
             ORACLE_TOOL_NAMES and NOT caught by a reserved prefix — a user server could \
             claim those names and shadow the Oracle. Add them to ORACLE_TOOL_NAMES."
        );
    }

    #[test]
    fn project_add_creates_devboule_and_round_trips() {
        let root = fresh_dir("proj-add");
        let path = ensure_project_config_dir(root.to_str().unwrap()).unwrap();
        assert!(path.starts_with(std::fs::canonicalize(&root).unwrap()));
        add_impl(&path, srv("proj-srv", "node", true)).unwrap();
        // The `.devboule/mcp-servers.json` file exists and lists the server.
        let listed = list_impl(&path);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "proj-srv");
        assert!(root.join(".devboule").join("mcp-servers.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
