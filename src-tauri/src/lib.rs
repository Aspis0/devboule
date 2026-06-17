mod backend;
mod oracle;
mod polis;

use serde::Serialize;
use std::fs;
use tauri::command;
use tauri::Manager;

use backend::agent_pty::AgentPtySessions;
use backend::censor::commands::CensorState;
use backend::design_generate::DesignGenState;
use backend::mini_coder_executor::MiniCoderState;
use backend::state::BackendState;
use polis::PolisState;

#[cfg(target_os = "windows")]
pub fn run_auth_helper_from_args() -> Option<i32> {
    backend::auth::run_helper_from_args(std::env::args())
}

#[cfg(not(target_os = "windows"))]
pub fn run_auth_helper_from_args() -> Option<i32> {
    None
}

/// Headless STRUCTURE bridge dispatch (Phase 11.2). When the process is invoked as
/// `aspis-management structure --root <path>` this prints the deterministic
/// `StructureGraph` as JSON to stdout and returns `Some(exit_code)` (0 ok, non-zero on a
/// bad/missing root); a normal launch returns `None` so `main` proceeds to the GUI.
///
/// This REUSES the existing `backend::structure` builder (no tree-sitter duplication) and
/// is the binary the shared, read-only `project_structure` MCP tool shells out to (the
/// Python tool resolves this binary via the `ASPIS_APP_BIN` env wired at every MCP launch
/// site). All-args overload so `main` can pass `std::env::args()` verbatim.
pub fn run_structure_cli_from_args() -> Option<i32> {
    backend::structure::run_structure_cli(std::env::args())
}

const ALLOWED_EXTERNAL_HOSTS: &[&str] = &[
    "aspis-bio.com",
    "console.nebius.ai",
    "console.scaleway.com",
    "dash.cloudflare.com",
    "developers.cloudflare.com",
    "docs.aspis-bio.com",
    "github.com",
    "manager.infomaniak.com",
    "www.scaleway.com",
];

#[derive(Serialize)]
struct ConfigPayload {
    raw: serde_json::Value,
}

fn resolve_config_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    if let Ok(dir) = app.path().resource_dir() {
        let path = dir.join("config.json");
        if path.exists() {
            return Ok(path);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let parent_path = cwd.join("../config.json");
        if parent_path.exists() {
            return Ok(parent_path);
        }
        let cwd_path = cwd.join("config.json");
        if cwd_path.exists() {
            return Ok(cwd_path);
        }
        // Nothing found anywhere: bootstrap a minimal default at the CWD so a fresh
        // checkout (config.json is per-machine and untracked) gets a working app with
        // zero manual setup. NEVER create in the resource dir (read-only in a packaged
        // build); CWD only. If the CWD is not writable, fall through to the original
        // not-found error so callers still fail cleanly.
        if let Ok(path) = bootstrap_default_config(&cwd) {
            return Ok(path);
        }
    }
    Err("config.json not found in resource dir, parent of CWD, or CWD".into())
}

/// Create a minimal default `config.json` (`{}`) in `dir` and return its path.
///
/// `{}` is the smallest content every config reader accepts: the design registry,
/// custom-agent-clients, mini-coder/censor RMW writers and the roles trust anchor all
/// treat a missing key as "unset" and only require the top-level value to be a JSON
/// object; agents.rs uses mere existence of the file as an app-root marker. This matches
/// the `"{}"` fixtures already used in the agents.rs management-root tests.
///
/// The write is atomic (temp + rename, reusing `replace_file_with_backup` exactly as
/// `design.rs` does); since the target does not exist no backup is ever taken. Returns
/// `Err` (never panics) when `dir` is not writable so the caller can fall back.
fn bootstrap_default_config(dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use backend::fs_replace::replace_file_with_backup;
    use backend::projects::config_write_lock;
    use chrono::Utc;

    let path = dir.join("config.json");
    // Serialize against every other config.json writer (Settings RMW savers, design
    // registry) so the bootstrap write and a concurrent save can never race over the
    // same temp+rename idiom. Neither caller (`setup()` nor `resolve_config_path`) holds
    // this lock, so acquiring it here cannot recurse/deadlock (std Mutex is non-reentrant).
    let _config_guard = config_write_lock()
        .lock()
        .map_err(|_| "Config write lock is poisoned.".to_string())?;
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let temp_path = path.with_extension(format!("json.{suffix}.tmp"));
    let backup_path = path.with_extension(format!("json.{suffix}.bak"));
    fs::write(&temp_path, "{}\n")
        .map_err(|e| format!("Could not write a default config.json in {}: {e}", dir.display()))?;
    replace_file_with_backup(&temp_path, &path, &backup_path, "config.json")?;
    Ok(path)
}

fn require_config_auth(state: &BackendState) -> Result<(), String> {
    state.ensure_unlocked()
}

fn validate_external_url(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "External link is invalid.".to_string())?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "External link host is missing.".to_string())?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !ALLOWED_EXTERNAL_HOSTS.contains(&host)
    {
        return Err("External link is not in the allowlist.".into());
    }
    Ok(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_command_gate_blocks_when_locked() {
        let state = BackendState::new();
        assert!(require_config_auth(&state).is_err());
    }

    #[test]
    fn external_url_gate_allows_known_https_hosts_only() {
        assert!(validate_external_url("https://dash.cloudflare.com").is_ok());
        assert!(validate_external_url("http://dash.cloudflare.com").is_err());
        assert!(validate_external_url("https://evil.example").is_err());
        assert!(validate_external_url("https://user:pass@dash.cloudflare.com").is_err());
    }

    fn bootstrap_tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aspis-config-bootstrap-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bootstrap_creates_a_valid_default_config_when_missing() {
        let dir = bootstrap_tmp_dir("missing");
        let path = bootstrap_default_config(&dir).expect("bootstrap should create the config");
        assert_eq!(path, dir.join("config.json"));
        assert!(path.exists(), "the default config.json must exist on disk");
        let content = fs::read_to_string(&path).unwrap();
        // Must parse as a JSON object (every reader requires `value.is_object()`).
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(value.is_object(), "default config must be a JSON object");
        assert!(
            value.as_object().unwrap().is_empty(),
            "default config must be the empty object {{}}"
        );
        // No temp/backup artifacts left behind by the atomic write.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "config.json")
            .collect();
        assert!(leftovers.is_empty(), "no temp/backup files: {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_helper_is_idempotent_for_byte_content() {
        // The locate function only calls bootstrap when no config exists anywhere, but
        // verify the helper itself writes byte-identical content so an accidental second
        // call on an already-default file is a no-op in practice. A second call DOES
        // overwrite (the helper takes no backup of the empty `{}` it would re-create),
        // so the second run exercises the temp+rename idiom over an existing target —
        // assert it leaves NO *.tmp / *.bak artifacts behind.
        let dir = bootstrap_tmp_dir("existing");
        let first = bootstrap_default_config(&dir).unwrap();
        let first_bytes = fs::read(&first).unwrap();
        let second = bootstrap_default_config(&dir).unwrap();
        let second_bytes = fs::read(&second).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first_bytes, second_bytes,
            "re-bootstrapping the default must be byte-identical"
        );
        // No transient temp/backup artifacts left behind by the second (overwriting) run.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "config.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "no *.tmp / *.bak artifacts after re-bootstrap: {leftovers:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bootstrap_errors_cleanly_when_dir_is_not_writable() {
        // Seam test: a non-existent parent directory makes the temp write fail, exercising
        // the same Err path an unwritable dir would (Windows read-only dirs don't reliably
        // block file creation, so we drive the logic through a missing-dir seam instead).
        let missing = bootstrap_tmp_dir("unwritable").join("does-not-exist");
        let res = bootstrap_default_config(&missing);
        assert!(res.is_err(), "bootstrap into a missing dir must Err, not panic");
        let _ = fs::remove_dir_all(missing.parent().unwrap());
    }
}

#[command]
fn get_config(
    app: tauri::AppHandle,
    backend_state: tauri::State<'_, BackendState>,
) -> Result<ConfigPayload, String> {
    require_config_auth(&backend_state)?;
    let config_path = resolve_config_path(&app)?;
    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {e}"))?;
    Ok(ConfigPayload { raw: value })
}

#[command]
fn open_external_url(
    url: String,
    backend_state: tauri::State<'_, BackendState>,
) -> Result<(), String> {
    backend_state.ensure_unlocked()?;
    let safe_url = validate_external_url(&url)?;
    open::that(safe_url).map_err(|_| "Failed to open external link.".to_string())
}

/// Resolve the projects directory for the resident-Oracle discovery file. Mirrors
/// `backend::projects::projects_dir`: an explicit `ASPIS_PROJECTS_DIR` wins, then
/// a `config.json`/`projects`-bearing cwd or its parent, then `<app_data>/projects`.
/// Returns `None` only if none resolve (publishing is then disabled; the operator
/// `/ask` path is unaffected).
fn resolve_projects_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    const PROJECTS_DIR: &str = "projects";
    // The handle-free portion (env override, then a config-bearing cwd/parent) is
    // shared with the Oracle supervisor so it can re-resolve a late-available
    // projects dir; only the app-data fallback below needs the `AppHandle`.
    if let Some(dir) = backend::oracle_service::resolve_projects_dir_handle_free() {
        return Some(dir);
    }
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join(PROJECTS_DIR))
}

pub fn run() {
    let backend_state = BackendState::new();
    let polis_state = PolisState::new();
    let agent_pty_sessions = AgentPtySessions::new();
    let censor_state = CensorState::new();
    let mini_coder_state = MiniCoderState::new();
    let design_gen_state = DesignGenState::new();

    tauri::Builder::default()
        // OS folder/file picker for the Polis "Open folder" action (folder-agnostic
        // map). Only the `dialog:allow-open` permission is granted in the
        // capabilities file — no save/message/ask surface is exposed.
        .plugin(tauri_plugin_dialog::init())
        // OS notifications for "an agent needs you" (Phase 5). Only the
        // `notification:default` permission is granted in the capabilities file;
        // the frontend requests OS permission on first use and degrades silently
        // to the in-app Header pill when denied/unsupported.
        .plugin(tauri_plugin_notification::init())
        .manage(backend_state)
        .manage(polis_state)
        .manage(agent_pty_sessions)
        .manage(censor_state)
        .manage(mini_coder_state)
        .manage(backend::mini_activity::MiniActivityStore::default())
        .manage(backend::mini_activity::ActivityTailRegistry::default())
        .manage(design_gen_state)
        .setup(|app| {
            // Record the bundled, read-only `oracle/` location so release builds
            // run Oracle Python only from there, never from a user "drop" dir.
            if let Ok(dir) = app.path().resource_dir() {
                oracle::python_oracle::set_bundled_oracle_root(&dir);
                // Record the same resource dir for the Censor's bundled, OFFLINE
                // semgrep ruleset (`resources/censor/semgrep-rules.yml`), so the
                // runner resolves an absolute local `--config` path instead of the
                // network registry. Dev/test builds use a crate-relative fallback.
                backend::censor::runners::semgrep::set_censor_resource_dir(&dir);
            }
            // RELEASE: data root = app_data_dir (writable). The bundled package
            // root above is read-only, so the `oracle-data/venv` runtime must be
            // installed/resolved under a writable app-data dir instead. In DEV we
            // intentionally do NOT record it, so `oracle_data_root()` resolves to
            // the source repo (which owns the real installed venv) via the
            // candidate search. The directory itself is created lazily by the
            // installer (`run_oracle_runtime_bootstrap` makes `oracle-data/`).
            //
            // SECURITY (FIX 1 — release RCE): recording this is MANDATORY in a
            // release build. `oracle_data_root()` fails CLOSED when no root is
            // recorded (returns `None`, never the candidate search), so if
            // `app_data_dir()` failed and we silently skipped recording, the Oracle
            // runtime would simply be unavailable rather than fall back to a
            // user-writable "drop" dir where `install_oracle_runtime` could `pip
            // install` + build a trusted venv = RCE. We therefore treat the failure
            // as FATAL so the data root is always recorded when the app runs.
            #[cfg(not(debug_assertions))]
            {
                let dir = app
                    .path()
                    .app_data_dir()
                    .expect("release: app_data_dir required for Oracle data root");
                oracle::python_oracle::set_oracle_data_root(&dir);
            }
            // Bootstrap the per-machine `config.json` EAGERLY, before any projects-dir
            // resolution below. On a fresh checkout config.json is untracked and absent;
            // `resolve_projects_dir` / `resolve_management_root` treat a config-bearing
            // cwd (or its parent) as the management root. If we waited for the first
            // `get_config` to lazily bootstrap it, this `setup()` would resolve the
            // projects dir to `<app_data>/projects` and `oracle_service::init` would
            // publish `.oracle-server.json` there, while agents (resolving later, after
            // the lazy bootstrap created cwd/config.json) would look in `cwd/projects` —
            // so the Oracle would appear OFFLINE for the entire first session and only
            // self-heal on restart. Resolving it here first makes both resolutions agree.
            //
            // We call `resolve_config_path` (not `bootstrap_default_config` directly) so
            // an EXISTING config is found and returned untouched — only a genuine
            // "nothing found anywhere" triggers the lazy bootstrap at the CWD. This is
            // exactly the same search+bootstrap `get_config` would run, just pulled
            // forward. Best-effort: if it Errs (e.g. unwritable CWD) we proceed exactly
            // as before; the lazy path in `resolve_config_path` remains the safety net.
            let _ = resolve_config_path(app.handle());
            // Record the projects dir for the resident-Oracle discovery file. The
            // resolution mirrors `backend::projects::projects_dir` (env override,
            // then a config-bearing cwd/parent, then the app data dir) so the
            // supervisor publishes `.oracle-server.json` exactly where the MCP
            // thin-clients (ASPIS_PROJECTS_DIR / --projects-dir) look for it.
            if let Some(projects_dir) = resolve_projects_dir(app.handle()) {
                backend::oracle_service::init(projects_dir);
            }
            // Install the mini-coder executor: the singleton backend thread that
            // drains `miniCoderDirectives` from `.aspis-agents.json` and spawns the
            // one-shot mini PTY for each. It is the ONLY agent->app action bridge
            // (the MCP file bridge is one-way and the frontend poll only runs on
            // Projects). Backend-resident + idle-cheap (early-exits when the queue is
            // empty); it is reaped on RunEvent::Exit below. Reading the agent state
            // is itself unlock-gated at the command layer, and a mini spawn needs a
            // live (registered) parent coder, so the loop is effectively inert until
            // the app is in use.
            if let Some(state) = app.try_state::<MiniCoderState>() {
                state.install(app.handle().clone());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            open_external_url,
            backend::commands::audit_provider_connection,
            backend::commands::audit_saved_provider_connection,
            backend::cli_agents::configure_cli_agents,
            backend::cli_agents::cli_agents_status,
            backend::cli_agents::unconfigure_cli_agents,
            backend::commands::cloudflare_smoke_dry_run,
            backend::commands::delete_cloudflare_agent_token_profile,
            backend::devices::approve_device_invite,
            backend::commands::delete_provider_token,
            backend::commands::delete_provider_scope,
            backend::commands::delete_oracle_llm_api_key,
            backend::commands::delete_scaleway_object_access_key,
            backend::commands::delete_scaleway_object_secret_key,
            backend::commands::get_exa_key_status,
            backend::commands::save_exa_key,
            backend::commands::delete_exa_key,
            backend::workspace::decrypt_workspace_bootstrap_package,
            backend::workspace::download_workspace_bootstrap_package,
            backend::roles::get_local_role,
            backend::roles::issue_role_grant,
            backend::roles::verify_and_adopt_role_grant,
            backend::roles::bake_trust_anchor,
            backend::roles::set_debug_role,
            backend::commands::get_auth_state,
            backend::devices::ensure_local_device_identity,
            backend::github::check_github_repo_access,
            backend::github::delete_github_token,
            backend::github::get_github_connection_status,
            backend::github::import_github_token_from_cli,
            backend::github::save_github_token,
            backend::agents::get_agent_live_state,
            backend::agents::focus_agent_terminal,
            backend::agents::stop_agent,
            backend::agent_pty::agent_pty_snapshot,
            backend::agent_pty::agent_pty_write,
            backend::agent_pty::agent_pty_resize,
            backend::agent_pty::agent_pty_kill,
            backend::agent_pty::agent_pty_list,
            backend::mini_coder_executor::mini_coder_kill,
            backend::mini_coder_executor::mini_coder_steer,
            backend::mini_activity::mini_activity_snapshot,
            backend::token_usage::get_agent_token_usage,
            backend::commands::get_cloud_dashboard_snapshot,
            backend::commands::get_cloudflare_agent_token_profiles,
            backend::devices::get_devices_invites_snapshot,
            backend::commands::get_provider_scope_status,
            backend::commands::get_oracle_index_preferences,
            backend::commands::get_oracle_llm_settings,
            backend::commands::get_scaleway_object_access_key_status,
            backend::commands::get_scaleway_object_secret_key_status,
            backend::commands::get_secret_status,
            backend::workspace::get_workspace_hygiene_snapshot,
            backend::workspace::get_workspace_package_snapshot,
            backend::commands::lock_app,
            backend::projects::add_project_milestone,
            backend::projects::append_project_note,
            backend::projects::create_project,
            backend::projects::create_project_task,
            backend::projects::get_custom_agent_clients,
            backend::projects::get_design_llm_backend,
            backend::projects::get_local_coder_backend,
            backend::projects::get_mini_coder_backend,
            backend::projects::get_mini_write_behavior,
            backend::projects::set_mini_write_behavior,
            backend::projects::get_agentic_coverage_languages,
            backend::projects::get_project,
            backend::projects::launch_project_agent_terminal,
            backend::projects::list_projects,
            backend::saved_workflows::list_saved_workflows,
            backend::projects::move_project_task,
            backend::projects::project_git_commit,
            backend::projects::project_git_push,
            backend::projects::project_git_pull,
            backend::projects::project_git_clone,
            backend::projects::git_push_requests_list,
            backend::projects::approve_git_push_request,
            backend::projects::deny_git_push_request,
            backend::plan_approval::plan_approval_requests_list,
            backend::plan_approval::get_plan_markdown,
            backend::plan_approval::list_project_plans,
            backend::plan_approval::approve_plan_request,
            backend::plan_approval::deny_plan_request,
            backend::plan_approval::reply_to_agent,
            backend::projects::prepare_project_agent_prompt,
            backend::commands::perform_scaleway_resource_action,
            backend::commands::create_scaleway_block_volume,
            backend::commands::resize_scaleway_block_volume,
            backend::commands::create_scaleway_block_snapshot,
            backend::commands::delete_scaleway_block_storage,
            backend::commands::create_scaleway_filesystem,
            backend::commands::delete_scaleway_filesystem,
            backend::commands::create_scaleway_object_bucket,
            backend::commands::delete_scaleway_object_bucket,
            backend::commands::set_scaleway_object_bucket_lifecycle,
            backend::commands::create_scaleway_sql_database,
            backend::commands::delete_scaleway_sql_database,
            backend::commands::scaleway_instance_create_dry_run,
            backend::commands::create_scaleway_instance,
            backend::commands::create_scaleway_function,
            backend::commands::delete_scaleway_function,
            backend::commands::create_scaleway_container,
            backend::commands::delete_scaleway_container,
            backend::projects::refresh_project_live_status,
            backend::projects::remove_project_milestone,
            backend::projects::get_censor_local_ai,
            backend::projects::set_censor_local_ai,
            backend::projects::set_custom_agent_clients,
            backend::projects::set_design_llm_backend,
            backend::projects::set_local_coder_backend,
            backend::projects::set_mini_coder_backend,
            backend::devices::reset_local_device_identity,
            backend::devices::revoke_device_invite,
            backend::commands::request_unlock,
            backend::agent_notifications::read_agent_notification_state,
            backend::agent_notifications::write_agent_notification_state,
            backend::commands::fetch_cloudflare_worker_settings,
            backend::commands::fetch_cloudflare_billing,
            backend::commands::fetch_scaleway_billing,
            backend::commands::cloudflare_env_dry_run,
            backend::commands::cloudflare_set_worker_env,
            backend::commands::fetch_cloudflare_ai_gateway_settings,
            backend::commands::set_cloudflare_ai_gateway_settings,
            backend::commands::cloudflare_autorag_reindex,
            backend::commands::fetch_cloudflare_kv_keys,
            backend::commands::fetch_cloudflare_kv_value,
            backend::commands::set_cloudflare_kv_value,
            backend::commands::delete_cloudflare_kv_value,
            backend::commands::cloudflare_d1_query,
            backend::commands::fetch_cloudflare_r2_config,
            backend::commands::set_cloudflare_r2_lifecycle,
            backend::commands::set_cloudflare_r2_cors,
            backend::commands::rotate_cloudflare_worker_secret,
            backend::commands::save_provider_scope,
            backend::commands::save_provider_token,
            backend::commands::save_cloudflare_agent_token_profile,
            backend::commands::save_oracle_index_preferences,
            backend::commands::save_oracle_llm_settings,
            backend::commands::save_scaleway_object_access_key,
            backend::commands::save_scaleway_object_secret_key,
            backend::workspace::scan_workspace_hygiene,
            backend::workspace::create_workspace_bootstrap_package,
            backend::commands::sync_provider_inventory,
            backend::projects::update_project_metadata,
            oracle::commands::get_oracle_coverage,
            oracle::commands::get_oracle_doctor,
            oracle::commands::get_oracle_duplicates,
            oracle::commands::get_oracle_index_status,
            oracle::commands::get_oracle_indexed_files,
            oracle::commands::get_oracle_node,
            oracle::commands::get_oracle_runtime,
            oracle::commands::get_oracle_runtime_setup,
            oracle::commands::install_oracle_runtime,
            oracle::commands::get_oracle_similar,
            oracle::commands::get_oracle_snapshot,
            oracle::commands::ask_oracle,
            oracle::commands::localize_card_suspects,
            oracle::commands::start_oracle_index_job,
            oracle::commands::start_oracle_index_watcher,
            oracle::commands::stop_oracle_index_watcher,
            oracle::commands::sync_oracle_text_chunks,
            polis::commands::generate_city_state,
            polis::commands::trigger_file_disaster,
            polis::commands::resolve_file_disaster,
            polis::commands::set_agent_location,
            polis::commands::update_agent_status,
            polis::commands::append_city_note,
            polis::commands::reset_city_to_new_era,
            polis::commands::polis_open_in_editor,
            polis::commands::spawn_scaleway_resource,
            polis::commands::stop_scaleway_resource,
            polis::commands::refresh_scaleway_status,
            polis::commands::polis_start_watch,
            polis::commands::polis_stop_watch,
            polis::commands::polis_refresh_agents,
            polis::commands::polis_get_scan_extensions,
            polis::commands::polis_set_scan_extensions,
            polis::commands::polis_reclassify_features,
            polis::commands::polis_get_dossier,
            polis::commands::polis_generate_dossier,
            polis::commands::polis_debug_log,
            backend::censor::commands::censor_start_watch,
            backend::censor::commands::censor_stop_watch,
            backend::censor::commands::censor_review_now,
            backend::censor::commands::censor_get_findings,
            backend::censor::commands::censor_dispose_finding,
            backend::censor::commands::censor_count_open,
            backend::censor::commands::censor_status,
            backend::censor::commands::censor_open_in_editor,
            backend::censor::commands::set_censor_trusted,
            backend::design::design_create_project,
            backend::design::design_load_project,
            backend::design::design_save_project,
            backend::design::design_write_manifest,
            backend::design::design_write_node,
            backend::design::design_oracle_context,
            backend::design::design_oracle_status,
            backend::design::design_read_design_md,
            backend::design::design_write_design_md,
            backend::design::design_write_tokens,
            backend::design::design_append_generation_log,
            backend::design::design_write_export,
            backend::project_skill::skills_list,
            backend::project_skill::skills_save,
            backend::project_skill::skills_set_enabled,
            backend::project_skill::skills_catalog,
            backend::project_skill::skills_install_from_catalog,
            backend::design_preview::design_preview_open,
            backend::design_preview::design_preview_capture,
            backend::design_preview::design_visual_critique,
            backend::design_preview::design_read_thumbnail,
            backend::design::design_registry_list,
            backend::design::design_registry_remember,
            backend::design::design_registry_rename,
            backend::design::design_registry_remove,
            backend::design_generate::design_generate,
            backend::design_generate::design_cancel_generation,
            backend::provider_detect::detect_providers,
            backend::hardware::detect_hardware,
            backend::api_fuzz::api_fuzz_run,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // APP-EXIT teardown for the resident Oracle server. The server, its
        // supervisor, and the discovery file are tied to the APP PROCESS lifecycle
        // (NOT the vault lock) so agents keep querying and any in-flight index keeps
        // running across a screen-lock. They are torn down only HERE, on app exit.
        // On Windows the child does NOT die with the parent, so this explicit
        // teardown is REQUIRED to avoid orphaning the server. `on_app_exit` is
        // idempotent and bounded (it stops the supervisor non-blocking, bounded-reaps
        // the killed child, and deletes the discovery file — no network I/O), so it
        // is safe here and from `BackendState::drop`. Run it once, on the first
        // exit-signalling event.
        .run(|app_handle, event| match event {
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
                backend::oracle_service::on_app_exit();
                // PAST-LESSON: kill+reap every app-hosted agent PTY child here. On
                // Windows the ConPTY child does NOT die with the parent, and a dev
                // Ctrl-C must not orphan agent shells. Idempotent and bounded (kill,
                // bounded wait/reap, bounded reader join), so it is safe to run on
                // both ExitRequested and Exit.
                backend::agent_pty::kill_all_on_exit(app_handle);
                // Reap the active Censor watcher (+ its worker) so quit / dev
                // Ctrl-C never orphans the watcher thread or an in-flight linter
                // subprocess. Non-blocking (detached reaper) + idempotent.
                backend::censor::commands::kill_all_on_exit(app_handle);
                // Signal + detached-reap the mini-coder executor loop so quit / dev
                // Ctrl-C never orphans the action-bridge thread. The mini PTY children
                // are reaped by agent_pty::kill_all_on_exit above (they live in the
                // same PTY map). Non-blocking + idempotent.
                backend::mini_coder_executor::kill_all_on_exit(app_handle);
                // Signal EVERY orchestrator activity-tail task to stop on quit. The
                // per-agent stop is normally driven by `mark_agent_session_closed`, but
                // the app-EXIT path does not funnel through there — without this the tail
                // tokio tasks would keep polling until the process actually tears down.
                // Idempotent + safe when no tails are registered.
                if let Some(registry) =
                    app_handle.try_state::<backend::mini_activity::ActivityTailRegistry>()
                {
                    registry.stop_all();
                }
            }
            _ => {}
        });
}
