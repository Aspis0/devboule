use super::model::{
    AuthState, AuxCredentialStatus, OracleIndexPreferences, OracleLlmSettings,
    OracleLlmSettingsStatus,
};
use super::state::BackendState;
use super::vault;
use tauri::{AppHandle, State};

#[tauri::command]
pub fn get_auth_state(state: State<'_, BackendState>) -> Result<AuthState, String> {
    state.auth_state()
}

#[tauri::command]
pub fn request_unlock(
    state: State<'_, BackendState>,
    reason: Option<String>,
) -> Result<AuthState, String> {
    let message = reason.unwrap_or_else(|| "Unlock Devboule".into());
    state.verify_unlock(&message)
}

#[tauri::command]
pub fn lock_app(state: State<'_, BackendState>) -> Result<AuthState, String> {
    state.lock("manual")
}

/// Refresh soft-lock idle TTL after genuine user activity (frontend-throttled).
/// Background pollers must NOT call this — only pointer/key handlers.
#[tauri::command]
pub fn touch_idle_activity(state: State<'_, BackendState>) -> Result<(), String> {
    state.touch_idle_activity()
}

// Censor CLOUD LLM API key — WRITE-ONLY from the UI: the key value is never returned.
// `get_*_status` reports present/absent, `save_*` SETs it, `delete_*` CLEARs it. The async
// Censor review reads it backend-internal (`vault::read_censor_cloud_key`) to authenticate
// the configured https endpoint — the one Censor path that egresses code off-device (opt-in).

#[tauri::command]
pub async fn get_censor_cloud_key_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::censor_cloud_key_status)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn save_censor_cloud_key(
    state: State<'_, BackendState>,
    key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::save_censor_cloud_key(&key))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn delete_censor_cloud_key(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::delete_censor_cloud_key)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

// Cloud main-coder API key for the local Devboule orchestrator's OPT-IN Cloud mode.
// WRITE-ONLY from the UI: the key value is never returned. `get_*_status` reports
// present/absent, `save_*` SETs it, `delete_*` CLEARs it. The orchestrator launch reads
// the key (backend-internal `vault::read_cloud_llm_key`) and sets `DEVBOULE_CLOUD_API_KEY`
// only when present AND the configured backend is `cloud`.

#[tauri::command]
pub async fn get_cloud_llm_key_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::cloud_llm_key_status)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn save_cloud_llm_key(
    state: State<'_, BackendState>,
    key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::save_cloud_llm_key(&key))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn delete_cloud_llm_key(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::delete_cloud_llm_key)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

// F46-close: Claude setup-token (shared). F47: keyring off the main thread.

#[tauri::command]
pub async fn get_claude_oauth_token_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::claude_oauth_token_status)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn save_claude_oauth_token(
    state: State<'_, BackendState>,
    token: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::save_claude_oauth_token(&token))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn delete_claude_oauth_token(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::delete_claude_oauth_token)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

// F51: in-app Login with Claude (PTY + vault). F47: never on the main thread.

#[tauri::command]
pub async fn claude_login_start(
    state: State<'_, BackendState>,
) -> Result<crate::backend::claude_login::ClaudeLoginResult, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(crate::backend::claude_login::run_claude_setup_token)
        .await
        .map_err(|e| format!("task join error: {e}"))
}

#[tauri::command]
pub async fn claude_login_cancel(
    state: State<'_, BackendState>,
) -> Result<crate::backend::claude_login::ClaudeLoginResult, String> {
    state.ensure_unlocked()?;
    // Cancel is a quick mutex/AtomicBool poke — still off main for consistency.
    tauri::async_runtime::spawn_blocking(crate::backend::claude_login::cancel_claude_login)
        .await
        .map_err(|e| format!("task join error: {e}"))
}

#[tauri::command]
pub async fn claude_login_state(
    state: State<'_, BackendState>,
) -> Result<crate::backend::claude_login::ClaudeLoginState, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(crate::backend::claude_login::login_state)
        .await
        .map_err(|e| format!("task join error: {e}"))
}

#[tauri::command]
pub async fn claude_login_submit_code(
    state: State<'_, BackendState>,
    code: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::backend::claude_login::submit_login_code(code)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

// F50: per-role Cloud LLM keys (fallback to shared). F47: keyring off the main thread.

#[tauri::command]
pub async fn get_cloud_llm_key_status_for_role(
    state: State<'_, BackendState>,
    role: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::cloud_llm_key_status_for_role(&role))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn save_cloud_llm_key_for_role(
    state: State<'_, BackendState>,
    role: String,
    key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::save_cloud_llm_key_for_role(&role, &key))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn delete_cloud_llm_key_for_role(
    state: State<'_, BackendState>,
    role: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::delete_cloud_llm_key_for_role(&role))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

// --- Web-search key commands (parameterized, 5 providers) ---------------------
//
// Three-parameterized set replaces the per-provider triples. The vault layer
// enforces the allowlist and reuses `provider:<id>` entries — Exa reuses the
// EXISTING `provider:exa` so no key is orphaned by this refactor.

#[tauri::command]
pub async fn websearch_key_status(
    state: State<'_, BackendState>,
    provider: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::websearch_key_status(&provider))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn websearch_save_key(
    state: State<'_, BackendState>,
    provider: String,
    key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::save_websearch_key(&provider, &key))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn websearch_delete_key(
    state: State<'_, BackendState>,
    provider: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || vault::delete_websearch_key(&provider))
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

// Web-search default-provider config (web-search.json) ----------------------

#[tauri::command]
pub fn websearch_get_config(
    state: State<'_, BackendState>,
    app: AppHandle,
) -> Result<crate::backend::pi_extensions::WebsearchConfig, String> {
    state.ensure_unlocked()?;
    crate::backend::pi_extensions::websearch_get_config(&app)
}

#[tauri::command]
pub fn websearch_set_config(
    state: State<'_, BackendState>,
    app: AppHandle,
    provider: String,
) -> Result<crate::backend::pi_extensions::WebsearchConfig, String> {
    state.ensure_unlocked()?;
    crate::backend::pi_extensions::websearch_set_config(&app, &provider)
}

#[tauri::command]
pub async fn get_oracle_llm_settings(
    state: State<'_, BackendState>,
) -> Result<OracleLlmSettingsStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::oracle_llm_settings_status)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub async fn save_oracle_llm_settings(
    state: State<'_, BackendState>,
    settings: OracleLlmSettings,
    api_key: Option<String>,
) -> Result<OracleLlmSettingsStatus, String> {
    state.ensure_unlocked()?;
    let status = tauri::async_runtime::spawn_blocking(move || {
        vault::save_oracle_llm_settings(&settings, api_key.as_deref())
    })
    .await
    .map_err(|e| format!("task join error: {e}"))??;
    // The resident Oracle server captures the LLM credentials at (re)spawn time
    // (rust_oracle::apply_llm_env_in_process, run inside ensure_rust_oracle_server)
    // and never re-reads the vault, so an already-running server would keep its
    // STALE key after a save. We do NOT tear it down synchronously here — that
    // would block this Tauri command and freeze the UI. Instead set a lightweight
    // "needs restart" flag and return immediately; the supervisor (oracle_service,
    // ~10s tick) observes it, stops the in-process server OFF the UI thread, and
    // the same tick's ensure respawns it with the fresh credentials. This command
    // returns within ~100ms regardless of server state.
    crate::backend::oracle_service::request_llm_restart();
    Ok(status)
}

#[tauri::command]
pub async fn delete_oracle_llm_api_key(
    state: State<'_, BackendState>,
) -> Result<OracleLlmSettingsStatus, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::delete_oracle_llm_api_key)
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

/// F31: async so keychain / file I/O never runs on the Tauri main thread
/// (sync keychain ACL prompts freeze the whole webview — pilot ping OK, eval/ipc dead).
#[tauri::command]
pub async fn get_oracle_index_preferences(
    state: State<'_, BackendState>,
) -> Result<OracleIndexPreferences, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(vault::read_oracle_index_preferences)
        .await
        .map_err(|e| format!("Oracle index preferences read task failed: {e}"))?
}

#[tauri::command]
pub async fn save_oracle_index_preferences(
    state: State<'_, BackendState>,
    preferences: OracleIndexPreferences,
) -> Result<OracleIndexPreferences, String> {
    state.ensure_unlocked()?;
    let saved = tokio::task::spawn_blocking(move || vault::save_oracle_index_preferences(&preferences))
        .await
        .map_err(|e| format!("Oracle index preferences save task failed: {e}"))??;
    // Re-arm the watcher one-shot so the supervisor's next tick picks up the
    // new index_mode (e.g. switching from "watch" to "commit" takes effect
    // immediately on the next ~10s tick rather than waiting for a restart).
    crate::backend::oracle_service::reset_watcher_armed();
    Ok(saved)
}
