//! Portable, runtime-resolved registration of the Aspis Oracle MCP server into a
//! collaborator's user-scope CLI agent configs, so a bare `claude` (and, when a
//! TOML dependency lands, `codex`) in any terminal already has the
//! `aspis-management` MCP server.
//!
//! Everything is resolved at runtime — interpreter, package root and projects
//! dir — so it works on any install/username/OS. There is a manual hardcoded
//! stopgap in the dev's own `~/.claude.json`; this is the real app-driven
//! version. The three commands are auth-gated (the unlocked session) because they
//! mutate the user's GLOBAL agent config.
//!
//! Privacy: the written config holds ONLY filesystem paths and offline flags. No
//! auth token is ever baked in — the AGENT token is read from the discovery file
//! at runtime by the MCP server. Nothing here logs or persists any secret.
//!
//! Cross-platform: the home directory and the venv interpreter layout are resolved
//! per-OS by reused helpers (`USERPROFILE` on Windows, `HOME` on Unix; the venv
//! python via `resolve_oracle_python`). The macOS path is UNVERIFIED on real
//! hardware — it follows the standard POSIX layout.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend::state::BackendState;
use crate::oracle::oracle_setup::{
    current_oracle_runtime_setup, oracle_venv_dir, resolve_oracle_python, venv_python,
};
use crate::oracle::python_oracle::find_oracle_package_root;

/// The MCP server key written into every supported client config. Stable so that
/// re-running `configure` overwrites in place (idempotent) and `unconfigure`
/// removes exactly this entry.
const MCP_KEY: &str = "aspis-management";
/// Filename of the user-scope Claude config under the home directory.
const CLAUDE_CONFIG_FILENAME: &str = ".claude.json";
/// Persisted backup written next to the Claude config before any mutation.
const CLAUDE_BACKUP_FILENAME: &str = ".claude.json.aspis-bak";

/// Status surfaced to the local "Configura CLI agents" UI. All paths are local
/// machine paths and safe to show the local operator; no secret is ever included.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliAgentsStatus {
    /// The `aspis-management` MCP entry is present in `~/.claude.json`.
    pub claude_configured: bool,
    /// Absolute path to the Claude config (whether or not it exists yet).
    pub claude_config_path: Option<String>,
    /// The `aspis-management` MCP entry is present in `~/.codex/config.toml`.
    pub codex_configured: bool,
    /// Human-readable note about Codex support state (e.g. deferred).
    pub codex_note: Option<String>,
    /// Resolved venv (or fallback) interpreter that the entry's `command` points at.
    pub interpreter: Option<String>,
    /// Resolved management/package root passed as `--root`.
    pub root: Option<String>,
    /// Resolved projects dir passed as `--projects-dir`.
    pub projects_dir: Option<String>,
    /// The local Oracle retrieval runtime (venv + embedder) is installed. When
    /// false, the registered `command` points at a python that cannot import
    /// `mcp`, so the UI must warn the operator to install the runtime first.
    pub runtime_ready: bool,
    /// Non-fatal advisory for the operator. Carries the real reason when a status
    /// could not be fully resolved (e.g. the user home directory was not found,
    /// which would otherwise be indistinguishable from "not configured"), and a
    /// post-write reminder to reopen any open Claude session so it picks up the
    /// change. `None` when there is nothing to surface.
    pub warning: Option<String>,
}

impl CliAgentsStatus {
    fn empty() -> Self {
        CliAgentsStatus {
            claude_configured: false,
            claude_config_path: None,
            codex_configured: false,
            codex_note: Some(CODEX_DEFERRED_NOTE.to_string()),
            interpreter: None,
            root: None,
            projects_dir: None,
            runtime_ready: false,
            warning: None,
        }
    }
}

/// Reminder appended to a successful configure: `~/.claude.json` is read by live
/// `claude` processes at startup, so a session that was already open when we wrote
/// will not see the new MCP entry until it is restarted.
const REOPEN_CLAUDE_NOTE: &str =
    "If a Claude session was open during configuration, reopen it to pick up the change.";

/// Surfaced when the user home directory cannot be resolved, so the all-None
/// status is not mistaken for "not configured".
const NO_HOME_WARNING: &str = "Could not resolve the user home directory.";

/// Refusal returned by `configure` when the Oracle retrieval runtime is not usable
/// yet. Writing the MCP entry against a bare OS python (which cannot `import mcp`)
/// would register a permanently-red server in every Claude session, so we fail
/// closed instead. Path-free on purpose.
const RUNTIME_NOT_READY_REFUSAL: &str =
    "Oracle runtime is not installed yet. Install it from Oracle → Setup before configuring CLI agents.";

/// Codex registration needs a TOML dependency to merge into `config.toml` without
/// clobbering other servers/keys. None is in `Cargo.toml` yet, so Codex is
/// deferred this step rather than silently adding a dependency.
const CODEX_DEFERRED_NOTE: &str =
    "Codex registration not built (needs a toml dependency); only Claude is configured.";

/// Resolve the user's home directory cross-platform. `USERPROFILE` on Windows,
/// `HOME` on Unix/macOS (mirrors `python_oracle::add_default_user_roots` and
/// `vault`). `None` ⇒ neither is set, which is fatal for config resolution.
fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// The resolved inputs needed to build the MCP entry. Kept as a struct so the
/// status report and the writer share one resolution and a pure entry-builder
/// seam can be unit-tested without touching the real filesystem.
struct ResolvedPaths {
    interpreter: String,
    root: PathBuf,
    projects_dir: PathBuf,
    runtime_ready: bool,
    /// The resolved interpreter is the fully-installed venv python (not the bare
    /// OS `python`/`python3` fallback). Together with `runtime_ready` this is the
    /// fail-closed gate: only when BOTH hold can the written `command` actually
    /// `import mcp`/`lancedb`. See `runtime_usable_for_write`.
    interpreter_is_venv: bool,
}

/// PURE fail-closed gate (FIX 2): the MCP entry is safe to write only when the
/// Oracle runtime is fully installed (`ready`) AND the interpreter the entry will
/// point at is the venv python (not the OS fallback, which cannot `import mcp`).
/// Either condition false ⇒ writing would register a permanently-red server.
fn runtime_usable_for_write(runtime_ready: bool, interpreter_is_venv: bool) -> bool {
    runtime_ready && interpreter_is_venv
}

/// Resolve interpreter + package root + projects dir at runtime. `None` when the
/// bundled `oracle/` package root or the projects dir cannot be located — in that
/// case there is nothing meaningful to register.
fn resolve_paths() -> Option<ResolvedPaths> {
    // PACKAGE root: the `oracle/` source, used as `--root` and PYTHONPATH for the
    // MCP entry. DATA root: where the venv (the interpreter) lives — separate in
    // release (read-only bundle vs. writable app-data).
    let root = find_oracle_package_root(None)?;
    let data_root = crate::oracle::python_oracle::oracle_data_root()?;
    let projects_dir = crate::backend::oracle_service::resolve_projects_dir_handle_free()?;
    // The venv interpreter when the runtime is installed; otherwise the OS default
    // (which lacks `mcp`/`lancedb`). `runtime_ready` captures whether the resolved
    // interpreter can actually import the deps so the UI can warn.
    let interpreter = resolve_oracle_python();
    let runtime_ready = current_oracle_runtime_setup().ready;
    // FIX 2: `resolve_oracle_python` returns the venv python ONLY when the venv is
    // fully installed; otherwise it falls back to a bare OS `python`/`python3` (or
    // a `PYTHON` override) that cannot import the MCP deps. Compare against the
    // canonical venv python path (under the DATA root) so the writer can refuse the
    // OS fallback.
    let venv_py = venv_python(&oracle_venv_dir(&data_root));
    let interpreter_is_venv = Path::new(&interpreter) == venv_py;
    Some(ResolvedPaths {
        interpreter,
        root,
        projects_dir,
        runtime_ready,
        interpreter_is_venv,
    })
}

/// Build the exact MCP entry value (command/args/env) for the resolved inputs.
///
/// Mirrors `projects::mcp_client_config_json` + the proven manual stopgap, with
/// the critical difference that `command` is the RESOLVED VENV python (an absolute
/// interpreter path), NOT the bare `"python"` the app-launch path relies on an
/// app-set PATH for. No auth token is included — the AGENT token comes from the
/// discovery file at runtime. Pure: no I/O, deterministic, unit-testable.
fn build_mcp_entry(interpreter: &str, root: &Path, projects_dir: &Path) -> Value {
    serde_json::json!({
        "command": interpreter,
        "args": [
            "-m",
            "oracle.server.aspis_mcp",
            "--root",
            root.to_string_lossy(),
            "--projects-dir",
            projects_dir.to_string_lossy(),
        ],
        "env": {
            "PYTHONPATH": root.to_string_lossy(),
            "PYTHONIOENCODING": "utf-8",
            "HF_HUB_OFFLINE": "1",
            "TRANSFORMERS_OFFLINE": "1",
            "ORACLE_REQUIRE_REAL_EMBEDDER": "1",
        },
    })
}

/// PURE merge: set `mcpServers["aspis-management"] = entry` on the Claude config,
/// `setdefault`-ing the `mcpServers` object, preserving every other key exactly.
/// Idempotent (running twice yields the same object).
///
/// Returns Err WITHOUT mutating when the config is not a JSON object or when an
/// existing `mcpServers` value is present but is not an object — refusing to
/// clobber a user file we do not understand.
fn upsert_claude_mcp_entry(config: &mut Value, entry: &Value) -> Result<(), String> {
    let Some(map) = config.as_object_mut() else {
        return Err("The Claude config is not a JSON object; refusing to modify it.".into());
    };
    let servers = map
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(servers) = servers.as_object_mut() else {
        return Err(
            "The Claude config 'mcpServers' is not an object; refusing to modify it.".into(),
        );
    };
    servers.insert(MCP_KEY.to_string(), entry.clone());
    Ok(())
}

/// PURE removal: drop `mcpServers["aspis-management"]` if present, leaving every
/// other key untouched. Idempotent; a missing entry/`mcpServers` is Ok. A
/// non-object config or non-object `mcpServers` is left untouched and reported Ok
/// (nothing of ours to remove — never clobber).
fn remove_claude_mcp_entry(config: &mut Value) {
    let Some(map) = config.as_object_mut() else {
        return;
    };
    if let Some(servers) = map.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove(MCP_KEY);
    }
}

/// True when the Claude config object currently carries our MCP entry.
fn claude_has_entry(config: &Value) -> bool {
    config
        .get("mcpServers")
        .and_then(|s| s.get(MCP_KEY))
        .is_some()
}

/// Load `~/.claude.json` into a JSON `Value`. Missing file ⇒ a fresh empty object
/// (`{}`). A present file that is invalid JSON, or valid JSON that is NOT an
/// object, is a hard error — we must never clobber an unparseable user file.
fn load_claude_config(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(Value::Object(serde_json::Map::new()));
            }
            let value: Value = serde_json::from_str(trimmed).map_err(|_| {
                "The existing ~/.claude.json is not valid JSON; refusing to modify it.".to_string()
            })?;
            if !value.is_object() {
                return Err(
                    "The existing ~/.claude.json is not a JSON object; refusing to modify it."
                        .into(),
                );
            }
            Ok(value)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Value::Object(serde_json::Map::new()))
        }
        Err(e) => Err(format!("Could not read ~/.claude.json: {e}")),
    }
}

/// Persist a backup of an existing config, then atomically replace it with
/// `new_text`. Backup is best-effort-persisted to `<home>/.claude.json.aspis-bak`
/// BEFORE writing (only when the original exists). The write itself is temp+rename
/// via the shared `fs_replace` helper (which uses its own transient backup for the
/// rename's crash safety and removes it on success).
///
/// CONCURRENCY (FIX 6): callers hold `cli_agents_write_lock` across the surrounding
/// load→modify→write so our own invocations cannot interleave. The transient
/// rename backup uses a UNIQUE per-write name (FIX 4) so even a hypothetical
/// unlocked concurrent writer cannot collide on it. NOTE: a separately-running
/// `claude` process rewrites this same file on its own schedule; our whole-file
/// rewrite is last-writer-wins against it and CANNOT be cross-process locked (the
/// `.aspis-bak` copy is the recovery path, and the success status tells the
/// operator to reopen any open Claude session).
fn atomic_write_claude_config(
    home: &Path,
    config_path: &Path,
    new_text: &str,
) -> Result<(), String> {
    if config_path.exists() {
        let backup = home.join(CLAUDE_BACKUP_FILENAME);
        std::fs::copy(config_path, &backup)
            .map_err(|e| format!("Could not back up ~/.claude.json: {e}"))?;
    }
    let temp = write_temp_sibling(config_path, new_text)?;
    // FIX 4: a UNIQUE transient backup name (the temp file's own random stem) so
    // two writes can never collide on a shared `.aspis-tmp-bak`.
    let temp_name = temp
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let transient_backup = sibling_with_suffix(config_path, &format!(".{temp_name}.bak"));
    crate::backend::fs_replace::replace_file_with_backup(
        &temp,
        config_path,
        &transient_backup,
        "Claude CLI agent config",
    )
}

/// Create a uniquely-named temp file next to `target` and write `contents`. Placed
/// in the SAME directory as the target so the subsequent rename is same-volume
/// (atomic). Not owner-restricted: this file holds only paths + offline flags, no
/// secret (unlike the discovery file's AGENT token).
fn write_temp_sibling(target: &Path, contents: &str) -> Result<PathBuf, String> {
    let mut name_bytes = [0u8; 16];
    getrandom::fill(&mut name_bytes).map_err(|e| format!("Could not generate temp name: {e}"))?;
    let file_name = format!(".claude-config-{}.tmp", hex::encode(name_bytes));
    let dir = target.parent().map(Path::to_path_buf).ok_or_else(|| {
        "The Claude config path has no parent directory; cannot write it.".to_string()
    })?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create the config directory: {e}"))?;
    let path = dir.join(file_name);
    std::fs::write(&path, contents).map_err(|e| {
        let _ = std::fs::remove_file(&path);
        format!("Could not write temp config file: {e}")
    })?;
    Ok(path)
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Read the current configured state of the Claude config WITHOUT writing.
/// A missing file is simply "not configured". An unparseable file is reported as
/// not-configured (the writer will refuse it loudly; the status must not throw).
fn claude_configured_on_disk(config_path: &Path) -> bool {
    match load_claude_config(config_path) {
        Ok(value) => claude_has_entry(&value),
        Err(_) => false,
    }
}

/// Build the status report from resolved paths + the on-disk Claude state.
fn build_status(
    resolved: Option<&ResolvedPaths>,
    claude_config_path: Option<&Path>,
) -> CliAgentsStatus {
    let mut status = CliAgentsStatus::empty();
    status.claude_config_path = claude_config_path.map(|p| p.to_string_lossy().to_string());
    status.claude_configured = claude_config_path
        .map(claude_configured_on_disk)
        .unwrap_or(false);
    if let Some(resolved) = resolved {
        status.interpreter = Some(resolved.interpreter.clone());
        status.root = Some(resolved.root.to_string_lossy().to_string());
        status.projects_dir = Some(resolved.projects_dir.to_string_lossy().to_string());
        status.runtime_ready = resolved.runtime_ready;
    }
    status
}

// --- commands ---------------------------------------------------------------

/// Process-wide lock serializing every read-modify-write of `~/.claude.json` and
/// its `.aspis-bak` backup (FIX 4). Two rapid `configure`/`unconfigure` calls (or
/// one of each) would otherwise interleave the load → upsert → atomic-write window
/// and could lose an update or race the backup copy. The config functions run on
/// the blocking pool, so a plain `std::sync::Mutex` is correct here (mirrors
/// `projects::project_write_lock`). NOTE (FIX 6): this lock is in-process only —
/// it does NOT coordinate with a separately-running `claude` process, which can
/// still last-writer-win us; that is documented on the writer, not lockable.
fn cli_agents_write_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Resolve paths at runtime and idempotently register the `aspis-management` MCP
/// entry in `~/.claude.json` (Codex deferred — no toml dep). Auth-gated.
#[tauri::command]
pub async fn configure_cli_agents(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<CliAgentsStatus, String> {
    auth_state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(configure_cli_agents_blocking)
        .await
        .map_err(|e| format!("CLI agents configure task failed: {e}"))?
}

fn configure_cli_agents_blocking() -> Result<CliAgentsStatus, String> {
    let home = user_home().ok_or_else(|| {
        "Could not resolve the user home directory (USERPROFILE/HOME).".to_string()
    })?;
    let config_path = home.join(CLAUDE_CONFIG_FILENAME);
    let resolved = resolve_paths().ok_or_else(|| {
        "Could not resolve the Oracle package root or projects directory to register.".to_string()
    })?;
    configure_with_resolved(&home, &config_path, &resolved)
}

/// Testable write seam: given an already-resolved home/config-path/inputs, apply
/// the fail-closed gate (FIX 2) and, only when usable, serialize and write the
/// entry under the process lock (FIX 4), then report success with the reopen
/// reminder (FIX 6). Pure of `user_home`/`resolve_paths` so a test can inject a
/// not-ready or ready `ResolvedPaths` against a temp home without a real venv.
fn configure_with_resolved(
    home: &Path,
    config_path: &Path,
    resolved: &ResolvedPaths,
) -> Result<CliAgentsStatus, String> {
    // FIX 2 (fail-closed): refuse to write a permanently-red entry. A bare OS
    // python (the fallback when the venv is absent) cannot `import mcp`, so
    // registering it would break every Claude session. NOTE: true release
    // portability also depends on the separate writable-data-dir work (the venv
    // currently resolves under the read-only resource root in an installed
    // build); until that lands, this refusal is the correct containment.
    if !runtime_usable_for_write(resolved.runtime_ready, resolved.interpreter_is_venv) {
        return Err(RUNTIME_NOT_READY_REFUSAL.to_string());
    }

    let entry = build_mcp_entry(
        &resolved.interpreter,
        &resolved.root,
        &resolved.projects_dir,
    );

    // FIX 4: serialize the whole read-modify-write + backup. FIX 6: keep the
    // read→modify→write window as TIGHT as possible — load immediately before
    // writing so a concurrent `claude` update has the smallest chance of being
    // clobbered (last-writer-wins; cannot be cross-process locked).
    let _guard = cli_agents_write_lock()
        .lock()
        .map_err(|_| "CLI agents write lock is poisoned.".to_string())?;

    let mut config = load_claude_config(config_path)?;
    upsert_claude_mcp_entry(&mut config, &entry)?;
    let new_text = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Could not serialize the Claude config: {e}"))?;
    atomic_write_claude_config(home, config_path, &new_text)?;

    let mut status = build_status(Some(resolved), Some(config_path));
    status.warning = Some(REOPEN_CLAUDE_NOTE.to_string());
    Ok(status)
}

/// Report (without writing) whether each client is configured plus the resolved
/// interpreter/root/projects-dir and `runtimeReady`. Auth-gated.
#[tauri::command]
pub async fn cli_agents_status(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<CliAgentsStatus, String> {
    auth_state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(cli_agents_status_blocking)
        .await
        .map_err(|e| format!("CLI agents status task failed: {e}"))
}

fn cli_agents_status_blocking() -> CliAgentsStatus {
    let home = user_home();
    let config_path = home.as_ref().map(|h| h.join(CLAUDE_CONFIG_FILENAME));
    let resolved = resolve_paths();
    let mut status = build_status(resolved.as_ref(), config_path.as_deref());
    // FIX 5: an unresolved home yields an all-None status that is otherwise
    // indistinguishable from "not configured". Surface the real reason so the UI
    // does not silently show a healthy-looking empty state.
    if home.is_none() {
        status.warning = Some(NO_HOME_WARNING.to_string());
    }
    status
}

/// Remove the `aspis-management` entry from the Claude config (and Codex when
/// built). Idempotent: a missing entry/file is Ok. Auth-gated.
#[tauri::command]
pub async fn unconfigure_cli_agents(
    auth_state: tauri::State<'_, BackendState>,
) -> Result<CliAgentsStatus, String> {
    auth_state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(unconfigure_cli_agents_blocking)
        .await
        .map_err(|e| format!("CLI agents unconfigure task failed: {e}"))?
}

fn unconfigure_cli_agents_blocking() -> Result<CliAgentsStatus, String> {
    let home = user_home().ok_or_else(|| {
        "Could not resolve the user home directory (USERPROFILE/HOME).".to_string()
    })?;
    let config_path = home.join(CLAUDE_CONFIG_FILENAME);

    // FIX 4: serialize against `configure` (and other `unconfigure`s) so the
    // read-modify-write + backup cannot interleave.
    let mut wrote = false;
    {
        let _guard = cli_agents_write_lock()
            .lock()
            .map_err(|_| "CLI agents write lock is poisoned.".to_string())?;
        if config_path.exists() {
            // Only rewrite when our entry is actually present, so unconfigure is a
            // true no-op (no needless backup churn / mtime bump) when nothing of
            // ours is set. Read immediately before write (FIX 6, tight window).
            let mut config = load_claude_config(&config_path)?;
            if claude_has_entry(&config) {
                remove_claude_mcp_entry(&mut config);
                let new_text = serde_json::to_string_pretty(&config)
                    .map_err(|e| format!("Could not serialize the Claude config: {e}"))?;
                atomic_write_claude_config(&home, &config_path, &new_text)?;
                wrote = true;
            }
        }
    }

    let resolved = resolve_paths();
    let mut status = build_status(resolved.as_ref(), Some(&config_path));
    // FIX 6: an already-open Claude session keeps the now-removed MCP entry in
    // memory until restarted.
    if wrote {
        status.warning = Some(REOPEN_CLAUDE_NOTE.to_string());
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn upsert_adds_entry_preserving_other_keys_and_servers() {
        let mut config = json!({
            "numStartups": 7,
            "theme": "dark",
            "mcpServers": {
                "other-server": { "command": "node", "args": ["x.js"] }
            }
        });
        let entry = json!({ "command": "py", "args": [] });

        upsert_claude_mcp_entry(&mut config, &entry).unwrap();

        // Other top-level keys preserved.
        assert_eq!(config["numStartups"], json!(7));
        assert_eq!(config["theme"], json!("dark"));
        // Existing other server preserved.
        assert_eq!(
            config["mcpServers"]["other-server"],
            json!({ "command": "node", "args": ["x.js"] })
        );
        // Our entry added.
        assert_eq!(config["mcpServers"][MCP_KEY], entry);
    }

    #[test]
    fn upsert_is_idempotent() {
        let mut config = json!({ "mcpServers": {} });
        let entry = json!({ "command": "py", "args": ["-m", "oracle.server.aspis_mcp"] });

        upsert_claude_mcp_entry(&mut config, &entry).unwrap();
        let after_first = config.clone();
        upsert_claude_mcp_entry(&mut config, &entry).unwrap();

        assert_eq!(config, after_first);
    }

    #[test]
    fn upsert_creates_mcpservers_when_absent() {
        let mut config = json!({ "theme": "light" });
        let entry = json!({ "command": "py" });

        upsert_claude_mcp_entry(&mut config, &entry).unwrap();

        assert_eq!(config["mcpServers"][MCP_KEY], entry);
        assert_eq!(config["theme"], json!("light"));
    }

    #[test]
    fn upsert_refuses_non_object_config() {
        let mut config = json!([1, 2, 3]);
        let entry = json!({ "command": "py" });

        let err = upsert_claude_mcp_entry(&mut config, &entry).unwrap_err();

        assert!(err.contains("not a JSON object"), "unexpected: {err}");
        // Unchanged.
        assert_eq!(config, json!([1, 2, 3]));
    }

    #[test]
    fn upsert_refuses_non_object_mcpservers() {
        let mut config = json!({ "mcpServers": "oops" });
        let entry = json!({ "command": "py" });

        let err = upsert_claude_mcp_entry(&mut config, &entry).unwrap_err();

        assert!(
            err.contains("'mcpServers' is not an object"),
            "unexpected: {err}"
        );
        assert_eq!(config["mcpServers"], json!("oops"));
    }

    #[test]
    fn remove_drops_only_our_entry() {
        let mut config = json!({
            "topKey": 1,
            "mcpServers": {
                "other-server": { "command": "node" },
                MCP_KEY: { "command": "py" }
            }
        });

        remove_claude_mcp_entry(&mut config);

        assert!(config["mcpServers"].get(MCP_KEY).is_none());
        assert_eq!(
            config["mcpServers"]["other-server"],
            json!({ "command": "node" })
        );
        assert_eq!(config["topKey"], json!(1));
    }

    #[test]
    fn remove_is_idempotent_and_safe_when_absent() {
        let mut config = json!({ "mcpServers": { "other": {} } });
        let before = config.clone();

        remove_claude_mcp_entry(&mut config);

        assert_eq!(config, before);
    }

    #[test]
    fn remove_is_noop_on_non_object_config() {
        let mut config = json!("not an object");
        remove_claude_mcp_entry(&mut config);
        assert_eq!(config, json!("not an object"));
    }

    #[test]
    fn build_mcp_entry_has_exact_args_and_env_shape() {
        let root = PathBuf::from("/code/root");
        let projects = PathBuf::from("/data/projects");
        let entry = build_mcp_entry("/venv/bin/python3", &root, &projects);

        // command is the resolved interpreter, NOT bare "python".
        assert_eq!(entry["command"], json!("/venv/bin/python3"));

        let args = entry["args"].as_array().unwrap();
        assert_eq!(args[0], json!("-m"));
        assert_eq!(args[1], json!("oracle.server.aspis_mcp"));
        assert_eq!(args[2], json!("--root"));
        assert_eq!(args[3], json!("/code/root"));
        assert_eq!(args[4], json!("--projects-dir"));
        assert_eq!(args[5], json!("/data/projects"));

        let env = &entry["env"];
        assert_eq!(env["PYTHONPATH"], json!("/code/root"));
        assert_eq!(env["PYTHONIOENCODING"], json!("utf-8"));
        assert_eq!(env["HF_HUB_OFFLINE"], json!("1"));
        assert_eq!(env["TRANSFORMERS_OFFLINE"], json!("1"));
        assert_eq!(env["ORACLE_REQUIRE_REAL_EMBEDDER"], json!("1"));
        // No token of any kind is baked in.
        assert!(env.get("ORACLE_AGENT_AUTH_TOKEN").is_none());
        assert!(env.get("ORACLE_AUTH_TOKEN").is_none());
    }

    #[test]
    fn claude_has_entry_detects_presence() {
        let with = json!({ "mcpServers": { MCP_KEY: { "command": "py" } } });
        let without = json!({ "mcpServers": { "other": {} } });
        let empty = json!({});

        assert!(claude_has_entry(&with));
        assert!(!claude_has_entry(&without));
        assert!(!claude_has_entry(&empty));
    }

    #[test]
    fn load_claude_config_missing_file_is_empty_object() {
        let path = std::env::temp_dir().join(format!(
            "aspis-cli-agents-missing-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let value = load_claude_config(&path).unwrap();

        assert_eq!(value, json!({}));
    }

    #[test]
    fn load_claude_config_refuses_invalid_json() {
        let path =
            std::env::temp_dir().join(format!("aspis-cli-agents-bad-{}.json", std::process::id()));
        std::fs::write(&path, "{ this is : not json").unwrap();

        let err = load_claude_config(&path).unwrap_err();
        assert!(err.contains("not valid JSON"), "unexpected: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_claude_config_refuses_non_object_json() {
        let path = std::env::temp_dir().join(format!(
            "aspis-cli-agents-array-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, "[1,2,3]").unwrap();

        let err = load_claude_config(&path).unwrap_err();
        assert!(err.contains("not a JSON object"), "unexpected: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn home_config_path_building_is_pure() {
        // Cross-platform home/interpreter resolution seam: given a fake home + fake
        // root, building the config path and entry needs no real FS writes.
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\fake\home"
        } else {
            "/fake/home"
        });
        let config_path = home.join(CLAUDE_CONFIG_FILENAME);
        assert!(config_path.ends_with(".claude.json"));

        let entry = build_mcp_entry(
            "/fake/venv/python",
            &home.join("code"),
            &home.join("projects"),
        );
        assert_eq!(entry["command"], json!("/fake/venv/python"));
    }

    #[test]
    fn sibling_with_suffix_appends_to_filename() {
        let path = PathBuf::from(if cfg!(windows) {
            r"C:\home\.claude.json"
        } else {
            "/home/.claude.json"
        });
        let bak = sibling_with_suffix(&path, ".aspis-tmp-bak");
        assert!(bak
            .to_string_lossy()
            .ends_with(".claude.json.aspis-tmp-bak"));
    }

    #[test]
    fn full_merge_roundtrip_on_disk_preserves_user_data() {
        // Integration of load → upsert → atomic write → reload, against a temp
        // "home". Asserts other keys survive and re-running stays idempotent. Does
        // NOT touch the real ~/.claude.json.
        let home = std::env::temp_dir().join(format!(
            "aspis-cli-agents-home-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let config_path = home.join(CLAUDE_CONFIG_FILENAME);
        std::fs::write(
            &config_path,
            r#"{"numStartups":3,"mcpServers":{"keep":{"command":"node"}}}"#,
        )
        .unwrap();

        let entry = build_mcp_entry("/venv/python", &home.join("code"), &home.join("projects"));
        let mut config = load_claude_config(&config_path).unwrap();
        upsert_claude_mcp_entry(&mut config, &entry).unwrap();
        let text = serde_json::to_string_pretty(&config).unwrap();
        atomic_write_claude_config(&home, &config_path, &text).unwrap();

        let reloaded = load_claude_config(&config_path).unwrap();
        assert_eq!(reloaded["numStartups"], json!(3));
        assert_eq!(reloaded["mcpServers"]["keep"], json!({ "command": "node" }));
        assert_eq!(reloaded["mcpServers"][MCP_KEY], entry);
        assert!(claude_has_entry(&reloaded));
        // Backup was persisted.
        assert!(home.join(CLAUDE_BACKUP_FILENAME).exists());

        // Idempotent re-run.
        let mut config2 = load_claude_config(&config_path).unwrap();
        upsert_claude_mcp_entry(&mut config2, &entry).unwrap();
        assert_eq!(config2, reloaded);

        // Remove only our entry.
        remove_claude_mcp_entry(&mut config2);
        let text2 = serde_json::to_string_pretty(&config2).unwrap();
        atomic_write_claude_config(&home, &config_path, &text2).unwrap();
        let reloaded2 = load_claude_config(&config_path).unwrap();
        assert!(!claude_has_entry(&reloaded2));
        assert_eq!(
            reloaded2["mcpServers"]["keep"],
            json!({ "command": "node" })
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// FIX 1: with serde_json's `preserve_order` feature the JSON map is an
    /// insertion-ordered IndexMap, so a load → upsert → pretty-print round trip
    /// must keep the user's existing keys in their ORIGINAL relative order with
    /// our `mcpServers`/entry appended — not scrambled to hash order.
    #[test]
    fn upsert_preserves_user_key_order() {
        let original =
            r#"{"zeta":1,"alpha":2,"mcpServers":{"keep":{"command":"node"}},"middle":3}"#;
        let mut config = load_claude_config_from_text(original);
        let entry = json!({ "command": "py" });
        upsert_claude_mcp_entry(&mut config, &entry).unwrap();

        let top_keys: Vec<&str> = config
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        // Original relative order kept; no new top-level key (mcpServers existed).
        assert_eq!(top_keys, vec!["zeta", "alpha", "mcpServers", "middle"]);
        // Inside mcpServers the pre-existing server stays first, ours is appended.
        let server_keys: Vec<&str> = config["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(server_keys, vec!["keep", MCP_KEY]);

        // And the pretty-printed text emits them in that same order.
        let text = serde_json::to_string_pretty(&config).unwrap();
        let z = text.find("\"zeta\"").unwrap();
        let a = text.find("\"alpha\"").unwrap();
        let m = text.find("\"middle\"").unwrap();
        assert!(z < a && a < m, "keys reordered in output: {text}");
    }

    fn load_claude_config_from_text(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// FIX 2 (pure gate): the entry is writable ONLY when the runtime is ready AND
    /// the resolved interpreter is the venv python.
    #[test]
    fn runtime_gate_requires_ready_and_venv_interpreter() {
        assert!(runtime_usable_for_write(true, true));
        assert!(!runtime_usable_for_write(false, true));
        assert!(!runtime_usable_for_write(true, false));
        assert!(!runtime_usable_for_write(false, false));
    }

    fn resolved_fixture(
        interpreter: &str,
        runtime_ready: bool,
        interpreter_is_venv: bool,
    ) -> ResolvedPaths {
        ResolvedPaths {
            interpreter: interpreter.to_string(),
            root: PathBuf::from(if cfg!(windows) {
                r"C:\code\root"
            } else {
                "/code/root"
            }),
            projects_dir: PathBuf::from(if cfg!(windows) {
                r"C:\data\projects"
            } else {
                "/data/projects"
            }),
            runtime_ready,
            interpreter_is_venv,
        }
    }

    fn temp_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!(
            "aspis-cli-agents-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    /// FIX 2: when the runtime is not ready (or the interpreter is the OS
    /// fallback), `configure` must NOT write the file and must return the clear,
    /// path-free refusal.
    #[test]
    fn configure_refuses_and_does_not_write_when_runtime_not_ready() {
        let home = temp_home("notready");
        let config_path = home.join(CLAUDE_CONFIG_FILENAME);
        // OS fallback interpreter, runtime not installed.
        let resolved = resolved_fixture("python", false, false);

        let err = configure_with_resolved(&home, &config_path, &resolved).unwrap_err();
        assert_eq!(err, RUNTIME_NOT_READY_REFUSAL);
        assert!(
            !config_path.exists(),
            "config must not be written when not ready"
        );
        assert!(!home.join(CLAUDE_BACKUP_FILENAME).exists());

        let _ = std::fs::remove_dir_all(&home);
    }

    /// FIX 2 + FIX 6: with the runtime ready and the venv interpreter, `configure`
    /// writes the entry and the success status carries the reopen reminder.
    #[test]
    fn configure_writes_when_runtime_ready_and_warns_to_reopen() {
        let home = temp_home("ready");
        let config_path = home.join(CLAUDE_CONFIG_FILENAME);
        let venv_py = if cfg!(windows) {
            r"C:\venv\Scripts\python.exe"
        } else {
            "/venv/bin/python3"
        };
        let resolved = resolved_fixture(venv_py, true, true);

        let status = configure_with_resolved(&home, &config_path, &resolved).unwrap();

        assert!(config_path.exists(), "config should be written when ready");
        let reloaded = load_claude_config(&config_path).unwrap();
        assert!(claude_has_entry(&reloaded));
        assert_eq!(reloaded["mcpServers"][MCP_KEY]["command"], json!(venv_py));
        assert!(status.claude_configured);
        assert!(status.runtime_ready);
        assert_eq!(status.warning.as_deref(), Some(REOPEN_CLAUDE_NOTE));

        let _ = std::fs::remove_dir_all(&home);
    }

    /// FIX 4: the process write lock serializes concurrent `configure` calls so a
    /// read-modify-write race cannot corrupt or lose the entry. After two threads
    /// each configure against the same temp home, the file is valid JSON with our
    /// entry present exactly once and the user's other keys intact.
    #[test]
    fn write_lock_serializes_concurrent_configure() {
        let home = temp_home("race");
        let config_path = home.join(CLAUDE_CONFIG_FILENAME);
        std::fs::write(
            &config_path,
            r#"{"numStartups":5,"mcpServers":{"keep":{"command":"node"}}}"#,
        )
        .unwrap();
        let venv_py = if cfg!(windows) {
            r"C:\venv\Scripts\python.exe"
        } else {
            "/venv/bin/python3"
        };

        std::thread::scope(|scope| {
            for _ in 0..2 {
                let home = home.clone();
                let config_path = config_path.clone();
                scope.spawn(move || {
                    let resolved = resolved_fixture(venv_py, true, true);
                    configure_with_resolved(&home, &config_path, &resolved).unwrap();
                });
            }
        });

        // No corruption: still valid JSON, user key intact, our entry present once.
        let reloaded = load_claude_config(&config_path).unwrap();
        assert_eq!(reloaded["numStartups"], json!(5));
        assert_eq!(reloaded["mcpServers"]["keep"], json!({ "command": "node" }));
        assert!(claude_has_entry(&reloaded));
        let server_keys: Vec<&str> = reloaded["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(server_keys.iter().filter(|k| **k == MCP_KEY).count(), 1);

        let _ = std::fs::remove_dir_all(&home);
    }

    /// FIX 4: the transient rename backup uses a unique per-write name (derived
    /// from the random temp file name), never a shared fixed suffix, so two writes
    /// cannot collide on it.
    #[test]
    fn transient_backup_name_is_unique_per_write() {
        let home = temp_home("uniqbak");
        let config_path = home.join(CLAUDE_CONFIG_FILENAME);
        std::fs::write(&config_path, r#"{"a":1}"#).unwrap();

        atomic_write_claude_config(&home, &config_path, r#"{"a":2}"#).unwrap();
        // No transient backup may survive a successful write, and crucially there
        // is no fixed `.aspis-tmp-bak` artifact left to collide on.
        assert!(!config_path
            .with_file_name(".claude.json.aspis-tmp-bak")
            .exists());
        // The persisted user-facing backup is still the stable name.
        assert!(home.join(CLAUDE_BACKUP_FILENAME).exists());

        let _ = std::fs::remove_dir_all(&home);
    }

    /// FIX 5: the status carries the new `warning` field (serde camelCase) and it
    /// defaults to None on a normal status.
    #[test]
    fn status_warning_field_serializes_camel_case_and_defaults_none() {
        let status = CliAgentsStatus::empty();
        assert!(status.warning.is_none());
        let json_text = serde_json::to_string(&status).unwrap();
        assert!(
            json_text.contains("\"warning\""),
            "missing camelCase warning: {json_text}"
        );
        assert!(json_text.contains("\"runtimeReady\""));
    }
}
