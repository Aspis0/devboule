use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::thread;

use tauri::AppHandle;
use tauri::Manager;
use serde_json;

// Statics
static PIGEON_DATA_ROOT: OnceLock<PathBuf> = OnceLock::new();
static PIGEON_CHILD: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
static PIGEON_HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
static PIGEON_PORT: OnceLock<u16> = OnceLock::new();
static PIGEON_AUTH_TOKEN: OnceLock<String> = OnceLock::new();

// Windows no-window
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn apply_no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

pub fn set_pigeon_data_root(dir: &Path) {
    let _ = PIGEON_DATA_ROOT.set(dir.to_path_buf());
}

fn pigeon_data_root() -> Option<PathBuf> {
    PIGEON_DATA_ROOT.get().cloned().or_else(|| {
        #[cfg(debug_assertions)]
        return std::env::current_dir().ok().map(|c| c.join("pigeon-data"));
        #[cfg(not(debug_assertions))]
        None
    })
}

/// Pure: read the `pigeon.enabled` flag out of a parsed config.json value. Default false.
///
/// ALPHA HARD-DISABLE (2026-07-23): Pigeon does NOT ship in the public alpha. This is the single
/// choke point every gate funnels through (spawn, agent hints, mini-executor ingest, censor pool,
/// prompt routing, `pigeon_spawn_env`), so forcing `false` here disables the entire transport and,
/// crucially, makes `pigeon_spawn_env` return `None` — so the loopback `PIGEON_AUTH_TOKEN` is never
/// minted or surfaced over IPC. No config value (`pigeon.enabled=true`) can turn it back on. The
/// original flag read is kept below (dead under the early return) so re-enabling later is a one-line
/// revert. See also `set_pigeon_enabled`, which refuses to persist an enable.
pub fn pigeon_enabled_from_value(v: &serde_json::Value) -> bool {
    // Public-alpha kill switch — remove this line to restore the config-driven flag.
    if !pigeon_alpha_enable_override() {
        return false;
    }
    v.get("pigeon")
        .and_then(|p| p.get("enabled"))
        .and_then(|e| e.as_bool())
        .unwrap_or(false)
}

/// Escape hatch for internal builds only: Pigeon stays hard-off unless `DEVBOULE_ALPHA_PIGEON=1`
/// is set in the process environment. There is no config.json / UI path to flip this — the public
/// alpha ships without the env var, so the transport is unreachable.
fn pigeon_alpha_enable_override() -> bool {
    std::env::var("DEVBOULE_ALPHA_PIGEON")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Pure alpha-gate for the config setter. When `alpha_override` is false (public alpha),
/// enabling is rejected. Disabling (`enabled=false`) is always allowed so a stale `true`
/// from an older config can be cleaned up. Unit-tested without touching process env.
pub fn pigeon_enable_write_allowed(enabled: bool, alpha_override: bool) -> Result<(), String> {
    if enabled && !alpha_override {
        return Err("Pigeon is disabled in this build and cannot be enabled.".to_string());
    }
    Ok(())
}

/// Read config.json and return whether Pigeon is enabled (default false on any error).
pub fn read_pigeon_enabled(app: &AppHandle) -> bool {
    let Some(path) = crate::backend::projects::locate_config_path(app) else { return false; };
    let Ok(content) = std::fs::read_to_string(&path) else { return false; };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else { return false; };
    pigeon_enabled_from_value(&value)
}

static PIGEON_ENABLED_CACHED: OnceLock<bool> = OnceLock::new();

/// Cached `pigeon.enabled` for the HOT executor path (read on every ~1500ms mini-executor tick and
/// on every terminal reap). MAX-RECALL fix: `read_pigeon_enabled` does a `config.json` disk read +
/// JSON parse each call — on the default (disabled) path that was a per-tick disk read the OFF path
/// never had before (byte-identical regression). The flag is **restart-scoped** (toggling it applies
/// on next launch — see LabsView "applies on restart"), so caching the first read is correct. The
/// UI/command path keeps using the fresh `read_pigeon_enabled` so the toggle reflects disk state.
pub fn pigeon_enabled_cached(app: &AppHandle) -> bool {
    *PIGEON_ENABLED_CACHED.get_or_init(|| read_pigeon_enabled(app))
}

fn pigeon_port() -> u16 {
    *PIGEON_PORT.get_or_init(random_pigeon_port)
}

fn random_pigeon_port() -> u16 {
    let mut bytes = [0u8; 2];
    if let Ok(()) = getrandom::fill(&mut bytes) {
        20_000 + (u16::from_le_bytes(bytes) % 30_000)
    } else {
        20_000 + (std::process::id() as u16 % 30_000)
    }
}

fn pigeon_auth_token() -> &'static str {
    PIGEON_AUTH_TOKEN.get_or_init(|| {
        let mut b = [0u8; 24];
        let _ = getrandom::fill(&mut b);
        b.iter().map(|x| format!("{:02x}", x)).collect()
    })
}

/// Slice 3: the `(PIGEON_PORT, PIGEON_AUTH_TOKEN)` pair the aspis_mcp spawn must inject so
/// the Python MCP server can talk to THIS process's Pigeon dispatcher — but ONLY when
/// Pigeon is enabled. Returns `None` when disabled, so the spawn site can keep the launch
/// env byte-identical to before this slice. Both statics lazily initialise here on first
/// call to the SAME values the child supervisor was (or will be) launched with, so the
/// MCP-published port/token always match the running dispatcher.
pub fn pigeon_spawn_env(app: &AppHandle) -> Option<(String, String)> {
    if !read_pigeon_enabled(app) {
        return None;
    }
    Some((pigeon_port().to_string(), pigeon_auth_token().to_string()))
}

fn pigeon_http_client() -> &'static reqwest::blocking::Client {
    PIGEON_HTTP_CLIENT.get_or_init(|| reqwest::blocking::Client::builder().build().unwrap())
}

/// Build a Pigeon HTTP client (Slice 3a) from the loopback port + auth token, ONLY when the Pigeon
/// child supervisor is actually spawned. The `PIGEON_*` statics stay private to this module; this is
/// the single production entry the mini-dispatch executor will use.
///
/// PRECONDITION: returns `None` unless `start_if_enabled` has spawned the child (i.e. `PIGEON_CHILD`
/// is `Some` and currently holds a live process). It does NOT re-probe `/health` — readiness is the
/// caller's concern (use `probe_ready` semantics, or just let the first request fail with a
/// connection error while the service is still booting). Note both `pigeon_port()` and
/// `pigeon_auth_token()` lazily initialise the statics on first call, so the URL/token always match
/// whatever the child was (or will be) launched with.
#[allow(dead_code)]
pub fn pigeon_client_from_running() -> Option<crate::backend::pigeon_client::PigeonClient> {
    // Gate on the child actually being present, so callers can't talk to a dispatcher that was
    // never started in this process.
    let running = PIGEON_CHILD
        .get()
        .map(|slot| {
            slot.lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
        })
        .unwrap_or(false);
    if !running {
        return None;
    }
    let base_url = format!("http://127.0.0.1:{}", pigeon_port());
    crate::backend::pigeon_client::PigeonClient::new(base_url, pigeon_auth_token()).ok()
}

fn pigeon_package_root(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let pigeon_dir = resource_dir.join("pigeon");
        if pigeon_dir.is_dir() {
            return Some(resource_dir);
        }
    }
    #[cfg(debug_assertions)]
    {
        std::env::current_dir().ok()
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn build_pigeon_command(app: &AppHandle) -> Result<Command, String> {
    let python = crate::oracle::oracle_setup::resolve_oracle_runtime_python()
        .ok_or_else(|| "Pigeon: Python runtime (oracle venv) not installed; skipping".to_string())?;
    let package_root = pigeon_package_root(app).ok_or_else(|| "Pigeon: package root not found".to_string())?;
    let data_root = pigeon_data_root().ok_or_else(|| "Pigeon: data root not set".to_string())?;
    let sqlite_path = data_root.join("mailbox.sqlite");
    let mut cmd = Command::new(python);
    cmd.args(["-m", "pigeon.dispatcher"])
        .current_dir(&package_root)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONPATH", &package_root)
        .env("PIGEON_PORT", pigeon_port().to_string())
        .env("PIGEON_AUTH_TOKEN", pigeon_auth_token())
        .env("PIGEON_DIR", &data_root)
        .env("PIGEON_SQLITE_PATH", &sqlite_path);
    apply_no_window(&mut cmd);
    Ok(cmd)
}

fn probe_ready() -> bool {
    let port = pigeon_port();
    let token = pigeon_auth_token();
    let client = pigeon_http_client();
    let url = format!("http://127.0.0.1:{port}/health");
    match client.get(&url)
        .timeout(Duration::from_secs(2))
        .header("x-pigeon-auth-token", token)
        .send() {
        Ok(resp) => {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>() {
                    return body.get("service").and_then(|s| s.as_str()) == Some("pigeon");
                }
            }
            false
        }
        Err(_) => false,
    }
}

pub fn start_if_enabled(app: &AppHandle) {
    if !read_pigeon_enabled(app) {
        eprintln!("Pigeon disabled (config pigeon.enabled=false); not starting.");
        return;
    }

    let slot = PIGEON_CHILD.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        eprintln!("Pigeon already running; not starting again.");
        return;
    }

    let mut cmd = match build_pigeon_command(app) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Pigeon: failed to build command: {e}");
            return;
        }
    };

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Pigeon: failed to spawn child: {e}");
            return;
        }
    };

    *guard = Some(child);

    thread::spawn(move || {
        let start = Instant::now();
        let timeout = Duration::from_secs(20);
        loop {
            if probe_ready() {
                eprintln!("Pigeon service is ready.");
                return;
            }
            if start.elapsed() > timeout {
                eprintln!("Pigeon: did not become ready within 20s.");
                return;
            }
            thread::sleep(Duration::from_millis(250));
        }
    });
}

pub fn on_app_exit() {
    if let Some(mutex) = PIGEON_CHILD.get() {
        let mut guard = mutex.lock().unwrap();
        if let Some(mut child) = guard.take() {
            // GRACEFUL STOP (Slice 2): send SIGTERM first so uvicorn runs the FastAPI lifespan
            // shutdown — which checkpoints the SQLite WAL (`wal_checkpoint(TRUNCATE)`) and closes
            // the connection cleanly — BEFORE we force-kill. Windows has no SIGTERM, so that build
            // falls straight through to kill() (TerminateProcess); the WAL is still crash-safe via
            // recovery, this just bounds its growth on a clean exit.
            #[cfg(unix)]
            {
                // SAFETY: kill() with a live pid and SIGTERM is async-signal-safe and well-defined.
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
                let graceful_deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => return, // exited cleanly after the WAL checkpoint
                        Ok(None) => {
                            if Instant::now() > graceful_deadline {
                                break; // escalate to SIGKILL below
                            }
                            thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => return,
                    }
                }
            }
            // Escalate (or the Windows path): force-kill, then reap up to 5s.
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if Instant::now() > deadline {
                            eprintln!("Pigeon: child did not exit within 5s, detaching.");
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

#[tauri::command]
pub fn get_pigeon_enabled(app: tauri::AppHandle) -> bool {
    read_pigeon_enabled(&app)
}

#[tauri::command]
pub fn set_pigeon_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::backend::state::BackendState>,
    enabled: bool,
) -> Result<bool, String> {
    state.ensure_unlocked()?;

    // ALPHA HARD-DISABLE (2026-07-23): Pigeon does not ship in the public alpha. Refuse any request
    // to persist `pigeon.enabled=true` so the UI/config path cannot arm a transport that
    // `pigeon_enabled_from_value` will ignore anyway. Disabling (writing `false`) is still allowed so
    // a stale `true` from an older config can be cleaned up. Internal builds bypass via the env override.
    pigeon_enable_write_allowed(enabled, pigeon_alpha_enable_override())?;

    let _lock = crate::backend::projects::config_write_lock()
        .lock()
        .map_err(|e| format!("config write lock poisoned: {e}"))?;

    let path = crate::backend::projects::locate_config_path(&app)
        .ok_or_else(|| "config.json not found".to_string())?;

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) if !path.exists() => "{}".to_string(),
        Err(e) => return Err(format!("Could not read config.json: {e}")),
    };
    let mut value: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
    if !value.is_object() { value = serde_json::json!({}); }
    let obj = value.as_object_mut().unwrap();
    let pigeon = obj.entry("pigeon").or_insert_with(|| serde_json::json!({}));
    if !pigeon.is_object() { *pigeon = serde_json::json!({}); }
    pigeon.as_object_mut().unwrap().insert("enabled".to_string(), serde_json::json!(enabled));
    let serialized = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;

    let timestamp = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let suffix = format!("{}-{}", std::process::id(), timestamp);
    let temp = path.with_extension(format!("json.{suffix}.tmp"));
    let backup = path.with_extension(format!("json.{suffix}.bak"));

    std::fs::write(&temp, serialized).map_err(|e| e.to_string())?;
    crate::backend::fs_replace::replace_file_with_backup(&temp, &path, &backup, "config.json")?;

    Ok(enabled)
}

#[cfg(test)]
mod tests {
    use super::{pigeon_enable_write_allowed, pigeon_enabled_from_value};
    use serde_json::json;
    #[test]
    fn default_false_when_absent() {
        assert!(!pigeon_enabled_from_value(&json!({})));
        assert!(!pigeon_enabled_from_value(&json!({"pigeon": {}})));
        assert!(!pigeon_enabled_from_value(&json!({"other": true})));
    }
    #[test]
    fn alpha_hard_disable_ignores_explicit_true() {
        // ALPHA HARD-DISABLE: even an explicit `enabled: true` reads as false while the
        // DEVBOULE_ALPHA_PIGEON override is unset (the default for the public alpha build).
        // Runtime gate coerces any stored true → false (defense in depth for old config.json).
        assert!(!pigeon_enabled_from_value(&json!({"pigeon": {"enabled": true}})));
        assert!(!pigeon_enabled_from_value(&json!({"pigeon": {"enabled": false}})));
    }
    #[test]
    fn setter_rejects_true_without_override() {
        // Public alpha (override=false): enabling must be rejected so IPC cannot arm the transport.
        let err = pigeon_enable_write_allowed(true, false).expect_err("must reject enable");
        assert!(
            err.contains("disabled in this build"),
            "unexpected error message: {err}"
        );
        // Disabling is always allowed (cleanup of stale true).
        assert!(pigeon_enable_write_allowed(false, false).is_ok());
    }
    #[test]
    fn setter_allows_true_with_internal_override() {
        // Internal builds with DEVBOULE_ALPHA_PIGEON=1 may enable.
        assert!(pigeon_enable_write_allowed(true, true).is_ok());
        assert!(pigeon_enable_write_allowed(false, true).is_ok());
    }
    #[test]
    fn non_bool_is_false() {
        assert!(!pigeon_enabled_from_value(&json!({"pigeon": {"enabled": "yes"}})));
        assert!(!pigeon_enabled_from_value(&json!({"pigeon": {"enabled": 1}})));
    }

    #[test]
    fn client_is_none_when_no_child_spawned() {
        // Slice 3 INGEST GATE (flag-OFF half): with no Pigeon child supervised in this
        // process, `pigeon_client_from_running()` returns None — so the executor's seam-B
        // drain (which builds the client via this fn) constructs NO client and makes NO
        // poll. Combined with the `read_pigeon_enabled` gate (default false, proven above),
        // the disabled path performs zero Pigeon work.
        assert!(super::pigeon_client_from_running().is_none());
    }
}
