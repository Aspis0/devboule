use super::model::{
    AuxCredentialStatus, OracleIndexPreferences, OracleLlmSettings, OracleLlmSettingsStatus,
};
use chrono::Utc;
use keyring::{Entry, Error as KeyringError};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const SERVICE: &str = "Devboule";

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn account_entry(account: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, account).map_err(|_| vault_error("open"))
}

fn github_token_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "provider:github").map_err(|_| vault_error("open"))
}

/// L2.4 — the Exa web-search API key for the local Devboule orchestrator. Stored
/// under the `provider:exa` account, following the same `provider:*` convention as
/// `provider:github`. The launch reads it (`read_exa_key`) and sets `EXA_API_KEY`
/// ONLY when present, so a missing key keeps the orchestrator's egress OFF.
fn exa_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "provider:exa").map_err(|_| vault_error("open"))
}

/// Censor CLOUD LLM API key. Stored under `provider:censor_cloud` (the same `provider:*`
/// convention as `provider:exa`). WRITE-ONLY from the UI: only present/absent is ever
/// surfaced via [`censor_cloud_key_status`]; the raw value is read backend-internal by the
/// async Censor review ([`read_censor_cloud_key`]) to send a `Bearer` header to the
/// configured https endpoint — the ONLY Censor path that egresses code off-device (opt-in).
fn censor_cloud_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "provider:censor_cloud").map_err(|_| vault_error("open"))
}

/// The CLOUD LLM bearer key for the LOCAL Devboule orchestrator's OPT-IN Cloud mode.
/// Stored under `provider:cloud_llm` (the same `provider:*` convention as `provider:exa`
/// and `provider:github`). The launch reads it (`read_cloud_llm_key`) and sets
/// `DEVBOULE_CLOUD_API_KEY` ONLY when present + the configured backend is `cloud`, so a
/// missing key keeps the orchestrator on its safe Mock path (the binary refuses to send an
/// unauthenticated request off-machine).
///
/// F50: per-role keys live at `provider:cloud_llm:<role>` and fall back to this shared
/// entry when absent (see [`read_cloud_llm_key_for_role`]).
fn cloud_llm_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "provider:cloud_llm").map_err(|_| vault_error("open"))
}

/// F46-close: Claude CLI setup-token (`claude setup-token` → `CLAUDE_CODE_OAUTH_TOKEN`).
fn claude_oauth_token_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "provider:claude_oauth_token").map_err(|_| vault_error("open"))
}

/// Canonical role ids for per-role Cloud LLM keys (F50).
pub const CLOUD_LLM_ROLES: &[&str] = &["orchestrator", "main", "mini", "verifier", "coder"];

/// Map spawn-path role aliases onto the F50 vault set. Unknown strings fail loud.
pub fn canonicalize_cloud_llm_role(role: &str) -> Result<&'static str, String> {
    match role.trim() {
        "orchestrator" => Ok("orchestrator"),
        "main" | "main-coder" => Ok("main"),
        "mini" | "mini-coder" => Ok("mini"),
        "verifier" => Ok("verifier"),
        "coder" | "local" => Ok("coder"),
        other => Err(format!(
            "Unknown cloud LLM role {other:?}. Allowed: {}",
            CLOUD_LLM_ROLES.join(", ")
        )),
    }
}

fn cloud_llm_key_entry_for_role(role: &str) -> Result<Entry, String> {
    let role = canonicalize_cloud_llm_role(role)?;
    Entry::new(SERVICE, &format!("provider:cloud_llm:{role}")).map_err(|_| vault_error("open"))
}

fn device_private_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "device:local_private_key:v1").map_err(|_| vault_error("open"))
}

fn device_signing_private_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "device:local_signing_private_key:v1").map_err(|_| vault_error("open"))
}

fn oracle_llm_settings_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "oracle:llm_settings").map_err(|_| vault_error("open"))
}

fn legacy_oracle_llm_api_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "oracle:llm_api_key").map_err(|_| vault_error("open"))
}

/// Pure keyring-slot NAME for the dedicated Oracle LLM key. There is a single key
/// per provider (no fallback), so the scope hashes the provider alone. The
/// `:primary:` segment is retained verbatim so keys saved by older builds (which
/// wrote `oracle:llm_api_key:primary:{scope}`) still resolve without a migration.
/// Pure (no keyring access) so it is unit-testable.
fn oracle_llm_api_key_entry_name(settings: &OracleLlmSettings) -> String {
    let scope = oracle_llm_key_scope(settings);
    format!("oracle:llm_api_key:primary:{scope}")
}

fn oracle_llm_api_key_entry(settings: &OracleLlmSettings) -> Result<Entry, String> {
    Entry::new(SERVICE, &oracle_llm_api_key_entry_name(settings)).map_err(|_| vault_error("open"))
}

/// LEGACY base_url-scoped key entry for the given settings. Used only by the
/// migration path to read back / clean up keys written by older builds.
fn legacy_oracle_llm_api_key_entry_for_settings(
    settings: &OracleLlmSettings,
) -> Result<Entry, String> {
    let scope = legacy_oracle_llm_key_scope(settings);
    Entry::new(SERVICE, &format!("oracle:llm_api_key:{scope}")).map_err(|_| vault_error("open"))
}

/// Best-effort removal of the legacy base_url-scoped key entry for the given
/// settings. Used after a save/delete to clean orphaned duplicates left by
/// older builds. NEVER fails the caller: any error (NoEntry/open/delete) is
/// ignored. If the legacy scope equals the new provider-only scope (impossible
/// today, but defensive), it is skipped so we never delete the live key.
fn cleanup_legacy_oracle_llm_api_key(settings: &OracleLlmSettings) {
    if legacy_oracle_llm_key_scope(settings) == oracle_llm_key_scope(settings) {
        return;
    }
    if let Ok(entry) = legacy_oracle_llm_api_key_entry_for_settings(settings) {
        let _ = entry.delete_credential();
    }
}

fn oracle_index_preferences_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "oracle:index_preferences").map_err(|_| vault_error("open"))
}

fn vault_error(action: &str) -> String {
    format!("The system keyring could not {action} this provider token.")
}




fn llm_provider_credential_account(provider: &str) -> Option<&'static str> {
    // The generic `openai` provider has no shared provider token — the dedicated
    // Oracle key is the only source.  Keep the fn shape (callers rely on it) but
    // always return None.
    let _ = provider;
    None
}

pub fn read_llm_provider_token(provider: &str) -> Result<Option<String>, String> {
    let Some(account) = llm_provider_credential_account(provider) else {
        return Ok(None);
    };
    match account_entry(account)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}



pub fn save_github_token(token: &str) -> Result<(), String> {
    let cleaned = token.trim();
    if cleaned.len() < 20 || cleaned.chars().any(char::is_whitespace) {
        return Err("GitHub token is too short or contains whitespace.".into());
    }
    github_token_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))
}

pub fn delete_github_token() -> Result<(), String> {
    match github_token_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Err(vault_error("delete")),
    }
}

pub fn read_github_token() -> Result<Option<String>, String> {
    match github_token_entry()?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

// --- Exa web-search key (L2.4 orchestrator egress) ---------------------------
//
// Stored under `provider:exa` (the `provider:*` convention). WRITE-ONLY from the
// UI: `save`/`delete` mutate it and `exa_key_status` reports present/absent ONLY —
// the raw value is NEVER returned to the frontend. The launch reads it via
// `read_exa_key` (backend-internal) and exports `EXA_API_KEY` to the orchestrator
// child ONLY when present, so a missing key keeps that agent's egress off.

const EXA_KEY_ID: &str = "exa_api_key";
const EXA_KEY_LABEL: &str = "Exa web-search API key";

pub fn save_exa_key(key: &str) -> Result<AuxCredentialStatus, String> {
    let cleaned = key.trim();
    // Same minimum-length + no-whitespace guard the scaleway object keys use: a
    // too-short / whitespace-bearing value is a paste error, not a real key.
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id: EXA_KEY_ID.into(),
            label: EXA_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Exa API key is too short or contains whitespace.".into()),
        });
    }
    exa_key_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    exa_key_status()
}

pub fn delete_exa_key() -> Result<AuxCredentialStatus, String> {
    match exa_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    exa_key_status()
}

/// Backend-INTERNAL reader: returns the raw key (or `None`). Used ONLY by the
/// orchestrator launch to set `EXA_API_KEY`. NOT exposed as a command — the UI can
/// only ever see present/absent via [`exa_key_status`].
pub fn read_exa_key() -> Result<Option<String>, String> {
    match exa_key_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

/// Present/absent status ONLY — never the value.
pub fn exa_key_status() -> Result<AuxCredentialStatus, String> {
    match read_exa_key() {
        Ok(Some(_)) => Ok(AuxCredentialStatus {
            id: EXA_KEY_ID.into(),
            label: EXA_KEY_LABEL.into(),
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Ok(None) => Ok(AuxCredentialStatus {
            id: EXA_KEY_ID.into(),
            label: EXA_KEY_LABEL.into(),
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some("Optional. Without a key, web search falls back to the free rate-limited Exa MCP server (mcp.exa.ai).".into()),
        }),
        Err(e) => Ok(AuxCredentialStatus {
            id: EXA_KEY_ID.into(),
            label: EXA_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(e),
        }),
    }
}

// --- Censor CLOUD LLM key (opt-in remote Censor review) ----------------------
//
// Stored under `provider:censor_cloud`. WRITE-ONLY from the UI: `save`/`delete` mutate it
// and `censor_cloud_key_status` reports present/absent ONLY — the raw value is NEVER
// returned to the frontend. The async Censor review reads it via `read_censor_cloud_key`
// (backend-internal) to authenticate the configured https endpoint.

const CENSOR_CLOUD_KEY_ID: &str = "censor_cloud_api_key";
const CENSOR_CLOUD_KEY_LABEL: &str = "Censor cloud LLM API key";

pub fn save_censor_cloud_key(key: &str) -> Result<AuxCredentialStatus, String> {
    let cleaned = key.trim();
    // Same minimum-length + no-whitespace guard the exa/scaleway keys use: a too-short /
    // whitespace-bearing value is a paste error, not a real key.
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id: CENSOR_CLOUD_KEY_ID.into(),
            label: CENSOR_CLOUD_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Censor cloud API key is too short or contains whitespace.".into()),
        });
    }
    // Reject non-whitespace control chars (mirrors `save_cloud_llm_key`): they pass the
    // whitespace guard but make an invalid `Bearer` header value, so the review would fail at
    // the transport layer with no actionable reason. Catch it here at save time instead.
    if cleaned.chars().any(char::is_control) {
        return Ok(AuxCredentialStatus {
            id: CENSOR_CLOUD_KEY_ID.into(),
            label: CENSOR_CLOUD_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Censor cloud API key must not contain control characters.".into()),
        });
    }
    censor_cloud_key_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    censor_cloud_key_status()
}

pub fn delete_censor_cloud_key() -> Result<AuxCredentialStatus, String> {
    match censor_cloud_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    censor_cloud_key_status()
}

/// Backend-INTERNAL reader: returns the raw key (or `None`). Used ONLY by the async Censor
/// review to set the `Bearer` header. NOT exposed as a command — the UI can only ever see
/// present/absent via [`censor_cloud_key_status`].
pub fn read_censor_cloud_key() -> Result<Option<String>, String> {
    match censor_cloud_key_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

/// Present/absent status ONLY — never the value. Mirrors [`exa_key_status`].
pub fn censor_cloud_key_status() -> Result<AuxCredentialStatus, String> {
    match read_censor_cloud_key() {
        Ok(Some(_)) => Ok(AuxCredentialStatus {
            id: CENSOR_CLOUD_KEY_ID.into(),
            label: CENSOR_CLOUD_KEY_LABEL.into(),
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Ok(None) => Ok(AuxCredentialStatus {
            id: CENSOR_CLOUD_KEY_ID.into(),
            label: CENSOR_CLOUD_KEY_LABEL.into(),
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some("Required for the opt-in Censor cloud LLM review.".into()),
        }),
        Err(e) => Ok(AuxCredentialStatus {
            id: CENSOR_CLOUD_KEY_ID.into(),
            label: CENSOR_CLOUD_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(e),
        }),
    }
}

// --- Cloud LLM key (opt-in Cloud mode for the local main coder) --------------
//
// Stored under `provider:cloud_llm` (the `provider:*` convention). WRITE-ONLY from
// the UI: `save`/`delete` mutate it and `cloud_llm_key_status` reports present/absent
// ONLY — the raw value is NEVER returned to the frontend. The launch reads it via
// `read_cloud_llm_key` (backend-internal) and exports `DEVBOULE_CLOUD_API_KEY` to the
// orchestrator child ONLY when present AND the configured backend is `cloud`. This
// mirrors the Exa key block field-for-field.

const CLOUD_LLM_KEY_ID: &str = "cloud_llm_api_key";
const CLOUD_LLM_KEY_LABEL: &str = "Cloud main-coder API key";

pub fn save_cloud_llm_key(key: &str) -> Result<AuxCredentialStatus, String> {
    let cleaned = key.trim();
    // Same minimum-length + no-whitespace guard the Exa key uses: a too-short /
    // whitespace-bearing value is a paste error, not a real bearer key.
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id: CLOUD_LLM_KEY_ID.into(),
            label: CLOUD_LLM_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Cloud API key is too short or contains whitespace.".into()),
        });
    }
    // Reject other ASCII control characters (`\x01`, `\x7f`, …) the whitespace check above
    // misses. reqwest would later refuse such a header value gracefully (no leak, no panic),
    // but the user only sees confusing repeated "request failed" with no diagnostic — fail
    // LOUD at save time instead. A real bearer key is never control-bearing.
    if cleaned.chars().any(|c| c.is_control()) {
        return Ok(AuxCredentialStatus {
            id: CLOUD_LLM_KEY_ID.into(),
            label: CLOUD_LLM_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Cloud API key must not contain control characters.".into()),
        });
    }
    cloud_llm_key_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    cloud_llm_key_status()
}

pub fn delete_cloud_llm_key() -> Result<AuxCredentialStatus, String> {
    match cloud_llm_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    cloud_llm_key_status()
}

/// Backend-INTERNAL reader: returns the raw key (or `None`). Used ONLY by the
/// orchestrator launch to set `DEVBOULE_CLOUD_API_KEY`. NOT exposed as a command — the
/// UI can only ever see present/absent via [`cloud_llm_key_status`].
pub fn read_cloud_llm_key() -> Result<Option<String>, String> {
    match cloud_llm_key_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

/// Present/absent status ONLY — never the value. Mirrors [`exa_key_status`].
pub fn cloud_llm_key_status() -> Result<AuxCredentialStatus, String> {
    match read_cloud_llm_key() {
        Ok(Some(_)) => Ok(AuxCredentialStatus {
            id: CLOUD_LLM_KEY_ID.into(),
            label: CLOUD_LLM_KEY_LABEL.into(),
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Ok(None) => Ok(AuxCredentialStatus {
            id: CLOUD_LLM_KEY_ID.into(),
            label: CLOUD_LLM_KEY_LABEL.into(),
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some(
                "Required for the local main coder's opt-in Cloud mode (prompts leave the machine)."
                    .into(),
            ),
        }),
        Err(e) => Ok(AuxCredentialStatus {
            id: CLOUD_LLM_KEY_ID.into(),
            label: CLOUD_LLM_KEY_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(e),
        }),
    }
}

// --- F46-close: Claude setup-token (`CLAUDE_CODE_OAUTH_TOKEN`) --------------------
//
// Same shape as the shared Cloud LLM key: write-only from the UI, backend-internal
// read for spawn injection. Value is NEVER returned by status and NEVER logged.

const CLAUDE_OAUTH_TOKEN_ID: &str = "claude_oauth_token";
const CLAUDE_OAUTH_TOKEN_LABEL: &str = "Claude setup-token";

pub fn save_claude_oauth_token(token: &str) -> Result<AuxCredentialStatus, String> {
    let cleaned = token.trim();
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id: CLAUDE_OAUTH_TOKEN_ID.into(),
            label: CLAUDE_OAUTH_TOKEN_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Claude setup-token is too short or contains whitespace.".into()),
        });
    }
    if cleaned.chars().any(|c| c.is_control()) {
        return Ok(AuxCredentialStatus {
            id: CLAUDE_OAUTH_TOKEN_ID.into(),
            label: CLAUDE_OAUTH_TOKEN_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Claude setup-token must not contain control characters.".into()),
        });
    }
    claude_oauth_token_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    claude_oauth_token_status()
}

pub fn delete_claude_oauth_token() -> Result<AuxCredentialStatus, String> {
    match claude_oauth_token_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    claude_oauth_token_status()
}

/// Backend-INTERNAL reader for spawn injection (`CLAUDE_CODE_OAUTH_TOKEN`). Not a command.
pub fn read_claude_oauth_token() -> Result<Option<String>, String> {
    match claude_oauth_token_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

/// Present/absent status ONLY — never the value.
pub fn claude_oauth_token_status() -> Result<AuxCredentialStatus, String> {
    match read_claude_oauth_token() {
        Ok(Some(_)) => Ok(AuxCredentialStatus {
            id: CLAUDE_OAUTH_TOKEN_ID.into(),
            label: CLAUDE_OAUTH_TOKEN_LABEL.into(),
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Ok(None) => Ok(AuxCredentialStatus {
            id: CLAUDE_OAUTH_TOKEN_ID.into(),
            label: CLAUDE_OAUTH_TOKEN_LABEL.into(),
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some(
                "Optional. Generate with `claude setup-token` and save here so product Claude \
                 launches authenticate without the owner interactive /login."
                    .into(),
            ),
        }),
        Err(e) => Ok(AuxCredentialStatus {
            id: CLAUDE_OAUTH_TOKEN_ID.into(),
            label: CLAUDE_OAUTH_TOKEN_LABEL.into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(e),
        }),
    }
}

// --- F50: per-role Cloud LLM keys (fallback to shared `provider:cloud_llm`) --------

fn cloud_llm_role_key_id(role: &str) -> String {
    format!("cloud_llm_api_key:{role}")
}

fn cloud_llm_role_key_label(role: &str) -> String {
    format!("Cloud API key ({role})")
}

/// Validate a cloud LLM key paste the same way as [`save_cloud_llm_key`].
/// Returns `Err(status)` when rejected (caller returns that status without keyring I/O).
fn validate_cloud_llm_key_paste(
    cleaned: &str,
    id: String,
    label: String,
) -> Result<(), AuxCredentialStatus> {
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Err(AuxCredentialStatus {
            id,
            label,
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Cloud API key is too short or contains whitespace.".into()),
        });
    }
    if cleaned.chars().any(|c| c.is_control()) {
        return Err(AuxCredentialStatus {
            id,
            label,
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Cloud API key must not contain control characters.".into()),
        });
    }
    Ok(())
}

/// Save a per-role Cloud LLM key (`provider:cloud_llm:<role>`). Same validation as the
/// shared key. Status NEVER returns the raw value.
pub fn save_cloud_llm_key_for_role(role: &str, key: &str) -> Result<AuxCredentialStatus, String> {
    let role = canonicalize_cloud_llm_role(role)?;
    let id = cloud_llm_role_key_id(role);
    let label = cloud_llm_role_key_label(role);
    let cleaned = key.trim();
    if let Err(status) = validate_cloud_llm_key_paste(cleaned, id, label) {
        return Ok(status);
    }
    cloud_llm_key_entry_for_role(role)?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    cloud_llm_key_status_for_role(role)
}

/// Delete the per-role Cloud LLM key (does not touch the shared fallback).
pub fn delete_cloud_llm_key_for_role(role: &str) -> Result<AuxCredentialStatus, String> {
    let role = canonicalize_cloud_llm_role(role)?;
    match cloud_llm_key_entry_for_role(role)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    cloud_llm_key_status_for_role(role)
}

/// Backend-INTERNAL reader: role entry first, else shared [`read_cloud_llm_key`].
/// Unknown roles error (no silent fallback for typos).
pub fn read_cloud_llm_key_for_role(role: &str) -> Result<Option<String>, String> {
    let role = canonicalize_cloud_llm_role(role)?;
    match cloud_llm_key_entry_for_role(role)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => read_cloud_llm_key(),
        Err(_) => Err(vault_error("read")),
    }
}

/// Present/absent for the **role-specific** slot only (not the shared fallback).
/// Never returns the raw value. `configured: true` only when the role entry itself is set.
pub fn cloud_llm_key_status_for_role(role: &str) -> Result<AuxCredentialStatus, String> {
    let role = canonicalize_cloud_llm_role(role)?;
    let id = cloud_llm_role_key_id(role);
    let label = cloud_llm_role_key_label(role);
    match cloud_llm_key_entry_for_role(role)?.get_password() {
        Ok(_) => Ok(AuxCredentialStatus {
            id,
            label,
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Err(KeyringError::NoEntry) => Ok(AuxCredentialStatus {
            id,
            label,
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some(
                "No per-role key — launches fall back to the shared Cloud API key.".into(),
            ),
        }),
        Err(_) => Ok(AuxCredentialStatus {
            id,
            label,
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(vault_error("read")),
        }),
    }
}

// --- Parameterized web-search API keys (5 providers) -----------------------------
//
// Stored under `provider:<websearch_id>` (the same `provider:*` convention as
// `provider:exa`). The parameterized set mirrors the per-provider blocks above
// (save → validate → set_password → status) but with a SINGLE allowlist + entry
// function. The EXISTING `provider:exa` entry is reused for Exa — no duplicate.

/// Strict allowlist of websearch provider ids the extension accepts for vault
/// keys. `gemini_search` maps to the env var `GEMINI_API_KEY` (the "gemini"
/// id is the DEFAULT PROVIDER value for the config file — a separate concern).
const WEBSEARCH_PROVIDER_ALLOWLIST: &[&str] = &[
    "exa",
    "brave",
    "tavily",
    "perplexity",
    "gemini_search",
    "openai_search",
    "parallel",
];

/// Label shown in the vault UI for a given websearch provider.
fn websearch_provider_label(provider: &str) -> &'static str {
    match provider {
        "exa" => "Exa",
        "brave" => "Brave",
        "tavily" => "Tavily",
        "perplexity" => "Perplexity",
        "gemini_search" => "Gemini",
        "openai_search" => "OpenAI",
        "parallel" => "Parallel",
        _ => "Unknown",
    }
}

/// Vault keyring account for the given websearch provider.
/// `"provider:exa"` is kept identical to the legacy `exa_key_entry()` so
/// existing keys are NOT orphaned by the refactor.
fn websearch_key_entry_name(provider: &str) -> String {
    format!("provider:{provider}")
}

fn websearch_keyring_entry(provider: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, &websearch_key_entry_name(provider))
        .map_err(|_| vault_error("open"))
}

/// Reject unknown provider ids before any keyring I/O. Pure — safe in tests.
fn validate_websearch_provider(provider: &str) -> Result<(), String> {
    if WEBSEARCH_PROVIDER_ALLOWLIST.contains(&provider) {
        Ok(())
    } else {
        Err(format!(
            "Unknown websearch provider: {provider:?}. \
             Allowed: {}.",
            WEBSEARCH_PROVIDER_ALLOWLIST.join(", ")
        ))
    }
}

/// Backend-INTERNAL reader: returns the raw key (or `None`). Used by the
/// sidecar spawn to set the matching env var. NOT exposed to the frontend.
pub fn read_websearch_key(provider: &str) -> Result<Option<String>, String> {
    validate_websearch_provider(provider)?;
    match websearch_keyring_entry(provider)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

/// Present/absent status ONLY — never the value.
pub fn websearch_key_status(provider: &str) -> Result<AuxCredentialStatus, String> {
    validate_websearch_provider(provider)?;
    let label = websearch_provider_label(provider);
    let id = format!("{provider}_api_key");
    match websearch_keyring_entry(provider)?.get_password() {
        Ok(_) => Ok(AuxCredentialStatus {
            id,
            label: format!("{label} web-search API key"),
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Err(KeyringError::NoEntry) => Ok(AuxCredentialStatus {
            id,
            label: format!("{label} web-search API key"),
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some(format!("{label} API key is not configured.")),
        }),
        Err(e) => Ok(AuxCredentialStatus {
            id,
            label: format!("{label} web-search API key"),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(format!("{label} API key vault error: {e}")),
        }),
    }
}

pub fn save_websearch_key(provider: &str, key: &str) -> Result<AuxCredentialStatus, String> {
    validate_websearch_provider(provider)?;
    let cleaned = key.trim();
    let label = websearch_provider_label(provider);
    let id = format!("{provider}_api_key");
    let status_label = format!("{label} web-search API key");
    // Same length / whitespace / control-char paste guard as cloud LLM keys
    // (`validate_cloud_llm_key_paste`): a key with \x01/\r is never legitimate.
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id,
            label: status_label,
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(format!("{label} API key is too short or contains whitespace.")),
        });
    }
    if cleaned.chars().any(|c| c.is_control()) {
        return Ok(AuxCredentialStatus {
            id,
            label: status_label,
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(format!(
                "{label} API key must not contain control characters."
            )),
        });
    }
    websearch_keyring_entry(provider)?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    websearch_key_status(provider)
}

pub fn delete_websearch_key(provider: &str) -> Result<AuxCredentialStatus, String> {
    validate_websearch_provider(provider)?;
    match websearch_keyring_entry(provider)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    websearch_key_status(provider)
}

pub fn save_device_private_key_hex(private_key_hex: &str) -> Result<(), String> {
    let cleaned = private_key_hex.trim();
    if cleaned.len() != 64 || !cleaned.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("Device private key must be 32 bytes encoded as hex.".into());
    }
    device_private_key_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))
}

pub fn read_device_private_key_hex() -> Result<Option<String>, String> {
    match device_private_key_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

pub fn delete_device_private_key() -> Result<(), String> {
    match device_private_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Err(vault_error("delete")),
    }
}

/// Ed25519 device signing key (separate from the X25519 key-exchange key).
/// Stored hex-encoded (32-byte seed) in its own isolated, versioned account.
pub fn save_device_signing_private_key_hex(private_key_hex: &str) -> Result<(), String> {
    let cleaned = private_key_hex.trim();
    if cleaned.len() != 64 || !cleaned.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("Device signing private key must be 32 bytes encoded as hex.".into());
    }
    device_signing_private_key_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))
}

pub fn read_device_signing_private_key_hex() -> Result<Option<String>, String> {
    match device_signing_private_key_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

pub fn delete_device_signing_private_key() -> Result<(), String> {
    match device_signing_private_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Err(vault_error("delete")),
    }
}




/// Remote-first default: Oracle answers are API-only (remote providers).
///
/// The default is a generic OpenAI-compatible endpoint the user configures
/// (base_url + model + key).  DeepSeek, OpenRouter, Claude-via-OpenRouter,
/// etc. are just examples of providers reachable through the single `openai`
/// provider — NOT separate provider values.  Users without a key get
/// extractive (retrieval-only) answers.
/// NOTE: the local *embedder* (Qwen3-Embedding-0.6B) is unaffected and remains
/// mandatory for retrieval.
pub fn default_oracle_llm_settings() -> OracleLlmSettings {
    OracleLlmSettings {
        provider: "openai".into(),
        model: "gpt-4o-mini".into(),
        base_url: None,
        remote_enabled: true,
    }
}

pub fn default_oracle_index_preferences() -> OracleIndexPreferences {
    let root = default_oracle_index_root().map(|path| path.to_string_lossy().to_string());
    OracleIndexPreferences {
        auto_watch_on_unlock: true,
        index_root: root.clone(),
        index_roots: root.into_iter().collect(),
        index_mode: None,
    }
}

pub fn save_oracle_index_preferences(
    preferences: &OracleIndexPreferences,
) -> Result<OracleIndexPreferences, String> {
    let cleaned = sanitize_oracle_index_preferences(preferences)?;
    let raw = serde_json::to_string(&cleaned)
        .map_err(|_| "Oracle index preferences could not be serialized.".to_string())?;
    // F31: debug/DEV unlock never writes the ad-hoc-signed keychain ACL path
    // (every relink invalidates "Always Allow"). File store survives rebuilds.
    if super::state::dev_unlock_enabled() {
        write_oracle_index_preferences_dev_file(&raw)?;
        return Ok(cleaned);
    }
    oracle_index_preferences_entry()?
        .set_password(&raw)
        .map_err(|_| vault_error("save"))?;
    read_oracle_index_preferences()
}

/// TEST SEAM: unit tests must NEVER reach the OS keyring through this read. On a
/// dev machine the "Devboule" keychain item EXISTS but the per-build test
/// binary is not in the item's ACL, so `get_password()` blocks forever on an
/// authorization prompt no headless test can answer — two resolver tests
/// (`http_command_root_resolver_is_the_workspace_resolver`,
/// `index_root_uses_the_same_shared_resolver_as_the_operator_path`) hung at 0%
/// CPU because of exactly this (2026-07-03 finding). In `cfg(test)` builds the
/// read returns the process-shared override (or the defaults), never the vault.
#[cfg(test)]
pub(crate) fn set_oracle_index_preferences_override_for_test(
    preferences: Option<OracleIndexPreferences>,
) {
    *test_oracle_index_preferences_override()
        .lock()
        .expect("oracle index preferences test override lock poisoned") = preferences;
}

#[cfg(test)]
fn test_oracle_index_preferences_override(
) -> &'static std::sync::Mutex<Option<OracleIndexPreferences>> {
    static OVERRIDE: std::sync::OnceLock<std::sync::Mutex<Option<OracleIndexPreferences>>> =
        std::sync::OnceLock::new();
    OVERRIDE.get_or_init(|| std::sync::Mutex::new(None))
}

/// cfg(test) twin of [`read_oracle_index_preferences`]: never touches the
/// keyring (see the seam doc above) — returns the override or the defaults.
#[cfg(test)]
pub fn read_oracle_index_preferences() -> Result<OracleIndexPreferences, String> {
    Ok(test_oracle_index_preferences_override()
        .lock()
        .expect("oracle index preferences test override lock poisoned")
        .clone()
        .unwrap_or_else(default_oracle_index_preferences))
}

/// Max wall-clock wait for a keychain round-trip (F31). Beyond this we return
/// defaults so the Tauri main thread / UI never freezes on an invisible ACL prompt
/// after an ad-hoc-signed debug relink.
const ORACLE_INDEX_PREFS_KEYCHAIN_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(2);

/// File-backed prefs used under debug DEV unlock (F31). Survives rebuilds because
/// it is not bound to the binary's codesign ACL.
fn oracle_index_preferences_dev_file_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DEVBOULE_ORACLE_INDEX_PREFS_PATH") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    if let Ok(dir) = std::env::var("ASPIS_PROJECTS_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join(".oracle-index-preferences.json"));
        }
    }
    // Prefer absolute checkout paths (CARGO_MANIFEST_DIR = src-tauri) so CWD
    // does not decide whether prefs load (audit: relative CWD trap).
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest.join("projects/.oracle-index-preferences.json"),
        manifest.join("../projects/.oracle-index-preferences.json"),
        PathBuf::from("src-tauri/projects/.oracle-index-preferences.json"),
        PathBuf::from("projects/.oracle-index-preferences.json"),
    ];
    for c in candidates {
        if let Some(parent) = c.parent() {
            if parent.exists() {
                return Some(c);
            }
        }
    }
    None
}

pub(crate) fn write_oracle_index_preferences_dev_file(raw: &str) -> Result<(), String> {
    let path = oracle_index_preferences_dev_file_path().ok_or_else(|| {
        "DEV unlock Oracle index prefs path unavailable (set DEVBOULE_ORACLE_INDEX_PREFS_PATH or ASPIS_PROJECTS_DIR)."
            .to_string()
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Could not create Oracle index prefs directory {}: {e}",
                parent.display()
            )
        })?;
    }
    std::fs::write(&path, raw).map_err(|e| {
        format!(
            "Could not write Oracle index prefs file {}: {e}",
            path.display()
        )
    })
}

/// F39: true when prefs look like a real operator choice (workspace path set),
/// not the empty/default "No indexed workspace folder" state.
pub(crate) fn oracle_index_prefs_have_workspace(prefs: &OracleIndexPreferences) -> bool {
    prefs.primary_index_root().is_some()
}

pub(crate) fn read_oracle_index_preferences_dev_file() -> Result<OracleIndexPreferences, String> {
    let Some(path) = oracle_index_preferences_dev_file_path() else {
        return Ok(default_oracle_index_preferences());
    };
    if path.is_file() {
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "Could not read Oracle index prefs file {}: {e}",
                path.display()
            )
        })?;
        let parsed: OracleIndexPreferences = serde_json::from_str(&raw)
            .map_err(|_| "Oracle index preferences are invalid.".to_string())?;
        return sanitize_oracle_index_preferences(&parsed);
    }
    // F39: one-shot migrate from keychain when the DEV file does not exist yet
    // (post-F31 upgrade). Bounded so a keychain ACL prompt cannot freeze DEV boot.
    migrate_oracle_index_prefs_keychain_to_dev_file(&path)
}

/// F39 pure decision: when the DEV prefs file is missing, should we copy
/// keychain prefs into it? Only if keychain has a real workspace root.
/// Unit-tested without OS keychain I/O.
pub(crate) fn f39_should_migrate_keychain_to_dev(
    dev_file_exists: bool,
    from_keychain: &OracleIndexPreferences,
) -> bool {
    !dev_file_exists && oracle_index_prefs_have_workspace(from_keychain)
}

/// F39 pure apply: if migrate decision is true, write `from_keychain` JSON to
/// `dev_path` and return those prefs; otherwise return defaults (caller may
/// still read an existing file separately).
pub(crate) fn f39_apply_migrate_keychain_to_dev_file(
    dev_path: &Path,
    from_keychain: &OracleIndexPreferences,
) -> Result<OracleIndexPreferences, String> {
    let exists = dev_path.is_file();
    if !f39_should_migrate_keychain_to_dev(exists, from_keychain) {
        return Ok(default_oracle_index_preferences());
    }
    let raw = serde_json::to_string_pretty(from_keychain)
        .map_err(|e| format!("Could not serialize Oracle index prefs for migrate: {e}"))?;
    if let Some(parent) = dev_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Could not create Oracle index prefs parent {}: {e}",
                parent.display()
            )
        })?;
    }
    std::fs::write(dev_path, &raw).map_err(|e| {
        format!(
            "Could not write migrated Oracle index prefs {}: {e}",
            dev_path.display()
        )
    })?;
    Ok(from_keychain.clone())
}

/// F39: if keychain still holds a real workspace preference, copy it into the
/// DEV file store and return it. Best-effort; on timeout/missing → defaults.
fn migrate_oracle_index_prefs_keychain_to_dev_file(
    dev_path: &Path,
) -> Result<OracleIndexPreferences, String> {
    // Production: bounded keychain read. Unit tests never call this path with a
    // live keychain — they drive `f39_apply_migrate_keychain_to_dev_file` with
    // injected prefs (see f39_* tests).
    let from_keychain = if cfg!(test) {
        // In-test: do not touch OS keychain. Production path always takes the
        // bounded read below (cfg!(test) is compile-time false for non-test builds).
        return Ok(default_oracle_index_preferences());
    } else {
        read_oracle_index_preferences_keychain_bounded(ORACLE_INDEX_PREFS_KEYCHAIN_TIMEOUT)
            .unwrap_or_else(|_| default_oracle_index_preferences())
    };
    f39_apply_migrate_keychain_to_dev_file(dev_path, &from_keychain)
}

fn read_oracle_index_preferences_keychain_inner() -> Result<OracleIndexPreferences, String> {
    match oracle_index_preferences_entry()?.get_password() {
        Ok(raw) => {
            let parsed: OracleIndexPreferences = serde_json::from_str(&raw)
                .map_err(|_| "Oracle index preferences are invalid.".to_string())?;
            sanitize_oracle_index_preferences(&parsed)
        }
        Err(KeyringError::NoEntry) => Ok(default_oracle_index_preferences()),
        Err(_) => Err(vault_error("read")),
    }
}

/// Keychain read on a helper thread with a hard timeout (F31). If the OS ACL
/// dialog would block forever, return defaults so pilot/UI stay live.
fn read_oracle_index_preferences_keychain_bounded(
    timeout: std::time::Duration,
) -> Result<OracleIndexPreferences, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("oracle-index-prefs-keychain".into())
        .spawn(move || {
            let _ = tx.send(read_oracle_index_preferences_keychain_inner());
        })
        .map_err(|e| format!("Could not spawn keychain reader: {e}"))?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Fail open to defaults: better a one-time reconfigure than a frozen app.
            Ok(default_oracle_index_preferences())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(vault_error("read"))
        }
    }
}

#[cfg(not(test))]
pub fn read_oracle_index_preferences() -> Result<OracleIndexPreferences, String> {
    // F31: debug DEV unlock never touches keychain (ad-hoc codesign ACL prompt
    // freezes the main thread / supervisor after every relink).
    if super::state::dev_unlock_enabled() {
        return read_oracle_index_preferences_dev_file();
    }
    read_oracle_index_preferences_keychain_bounded(ORACLE_INDEX_PREFS_KEYCHAIN_TIMEOUT)
}

fn sanitize_oracle_index_preferences(
    preferences: &OracleIndexPreferences,
) -> Result<OracleIndexPreferences, String> {
    let default = default_oracle_index_preferences();
    // Collect candidate roots: multi-root list first, then single alias, then default.
    let mut candidates: Vec<String> = preferences
        .index_roots
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if candidates.is_empty() {
        if let Some(r) = preferences
            .index_root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            candidates.push(r.to_string());
        } else if let Some(r) = default.index_root.as_deref() {
            candidates.push(r.to_string());
        }
    } else if let Some(primary) = preferences
        .index_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Ensure primary alias is present as first entry when provided.
        if !candidates.iter().any(|c| c == primary) {
            candidates.insert(0, primary.to_string());
        } else {
            // Move primary to front.
            candidates.retain(|c| c != primary);
            candidates.insert(0, primary.to_string());
        }
    }

    let mut index_roots: Vec<String> = Vec::new();
    for raw in candidates {
        let path = PathBuf::from(&raw);
        if !path.exists() || !path.is_dir() {
            return Err("Oracle index root must be an existing folder.".into());
        }
        let canon = path
            .canonicalize()
            .map_err(|_| "Oracle index root could not be resolved.".to_string())?
            .to_string_lossy()
            .to_string();
        if !index_roots.iter().any(|e| e == &canon) {
            index_roots.push(canon);
        }
    }
    let index_root = index_roots.first().cloned();
    // Coerce any value that is not "watch" or "commit" to None (don't store
    // garbage). Absent (None) is valid and means the default (watch).
    let index_mode = preferences
        .index_mode
        .as_deref()
        .filter(|m| matches!(*m, "watch" | "commit"))
        .map(str::to_owned);
    Ok(OracleIndexPreferences {
        auto_watch_on_unlock: preferences.auto_watch_on_unlock,
        index_root,
        index_roots,
        index_mode,
    })
}

fn default_oracle_index_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("ORACLE_INDEX_ROOT") {
        let path = PathBuf::from(root);
        if path.exists() && path.is_dir() {
            return Some(path);
        }
    }
    // Cross-platform home: USERPROFILE on Windows, HOME on macOS/Linux.
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    for base in ["Desktop", "Documents", "Downloads", ""] {
        for name in ["aspis bio", "Aspis Bio", "aspis-bio"] {
            let path = if base.is_empty() {
                home.join(name)
            } else {
                home.join(base).join(name)
            };
            if path.exists() && path.is_dir() {
                return Some(path);
            }
        }
    }
    None
}

pub fn save_oracle_llm_settings(
    settings: &OracleLlmSettings,
    api_key: Option<&str>,
) -> Result<OracleLlmSettingsStatus, String> {
    let cleaned = sanitize_oracle_llm_settings(settings)?;
    if let Some(api_key) = api_key.map(str::trim).filter(|value| !value.is_empty()) {
        if !valid_oracle_api_key(api_key) {
            return Ok(oracle_llm_settings_error_status(
                cleaned,
                "API key is too short or contains whitespace.",
            ));
        }
    }
    // F2: write the validated API KEY to its slot FIRST, and only AFTER the key
    // write succeeds persist the settings blob. If the key write fails (e.g.
    // after a base_url change), we abort BEFORE mutating the settings, so settings
    // never point at a new endpoint with no key landed.
    if let Some(api_key) = api_key.map(str::trim).filter(|value| !value.is_empty()) {
        oracle_llm_api_key_entry(&cleaned)?
            .set_password(api_key)
            .map_err(|_| vault_error("save"))?;
    }
    let raw = serde_json::to_string(&cleaned)
        .map_err(|_| "Oracle LLM settings could not be serialized.".to_string())?;
    oracle_llm_settings_entry()?
        .set_password(&raw)
        .map_err(|_| vault_error("save"))?;
    // Best-effort cleanup runs ONLY after the key + the settings write succeeded:
    // remove the legacy base_url-scoped orphan for the just-saved settings so a
    // stale duplicate is not left behind. Never touches the provider-only slot.
    if api_key
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        cleanup_legacy_oracle_llm_api_key(&cleaned);
    }
    oracle_llm_settings_status()
}

pub fn delete_oracle_llm_api_key() -> Result<OracleLlmSettingsStatus, String> {
    let settings = read_oracle_llm_settings().unwrap_or_else(|_| default_oracle_llm_settings());
    match oracle_llm_api_key_entry(&settings)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    // Best-effort: also clear the legacy base_url-scoped slot for the current
    // base_url so a delete fully clears any migration leftover.
    cleanup_legacy_oracle_llm_api_key(&settings);
    match legacy_oracle_llm_api_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    oracle_llm_settings_status()
}

/// Reads the dedicated Oracle LLM answer key for the given settings, walking the
/// migration read-fallback chain (provider-only slot → legacy base_url-scoped →
/// very-old unscoped Scaleway-only slot). "fallback" here is purely the migration
/// READ chain — there is no LLM-to-LLM fallback key.
pub fn read_oracle_llm_api_key_for_settings(
    settings: &OracleLlmSettings,
) -> Result<Option<String>, String> {
    // Preferred: the provider-only slot.
    match oracle_llm_api_key_entry(settings)?.get_password() {
        Ok(value) => return Ok(Some(value)),
        Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("read")),
    }
    // Migration read-fallback: the LEGACY base_url-scoped slot for THESE settings.
    // Lets an existing user keep reading "configured" until their next save
    // migrates the key to the provider-only slot.
    match legacy_oracle_llm_api_key_entry_for_settings(settings)?.get_password() {
        Ok(value) => return Ok(Some(value)),
        Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("read")),
    }
    // Last resort: the VERY OLD unscoped entry. Gated tightly: it was only ever
    // written for the Scaleway key (the sole provider that existed when the
    // unscoped format shipped). Never return it for a non-Scaleway provider —
    // otherwise a stale Scaleway key would be reported as "configured" for an
    // Infomaniak/Mistral provider.
    if settings.provider.trim().eq_ignore_ascii_case("scaleway") {
        match legacy_oracle_llm_api_key_entry()?.get_password() {
            Ok(value) => return Ok(Some(value)),
            Err(KeyringError::NoEntry) => {}
            Err(_) => return Err(vault_error("read")),
        }
    }
    Ok(None)
}

fn valid_oracle_api_key(api_key: &str) -> bool {
    api_key.len() >= 12 && !api_key.contains(char::is_whitespace)
}

fn oracle_llm_settings_error_status(
    settings: OracleLlmSettings,
    message: &str,
) -> OracleLlmSettingsStatus {
    OracleLlmSettingsStatus {
        settings,
        api_key_configured: false,
        status: "error".into(),
        message: Some(message.into()),
    }
}

pub fn read_oracle_llm_settings() -> Result<OracleLlmSettings, String> {
    match oracle_llm_settings_entry()?.get_password() {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|_| "Oracle LLM settings are invalid.".to_string())
            .and_then(|settings| sanitize_oracle_llm_settings(&settings)),
        Err(KeyringError::NoEntry) => Ok(default_oracle_llm_settings()),
        Err(_) => Err(vault_error("read")),
    }
}

pub fn oracle_llm_settings_status() -> Result<OracleLlmSettingsStatus, String> {
    let settings = read_oracle_llm_settings().unwrap_or_else(|_| default_oracle_llm_settings());
    let dedicated_api_key_configured = read_oracle_llm_api_key_for_settings(&settings)?.is_some();
    let provider_api_key_configured =
        settings.remote_enabled && read_llm_provider_token(&settings.provider)?.is_some();
    let api_key_configured = dedicated_api_key_configured || provider_api_key_configured;
    // LOCAL providers are keyless by design: never nag "missing_api_key".
    let is_local_provider = matches!(settings.provider.as_str(), "omlx" | "ollama");
    let is_disabled = !settings.remote_enabled && settings.provider.is_empty();
    let status = if is_disabled {
        "disabled"
    } else if is_local_provider {
        "local"
    } else if settings.remote_enabled && !api_key_configured {
        "missing_api_key"
    } else if settings.remote_enabled {
        "configured"
    } else {
        "local"
    };
    let message = if is_disabled {
        Some("Answer LLM is disabled — Oracle returns retrieval-only answers.".into())
    } else if is_local_provider {
        Some("Local loopback provider — keyless; prompts never leave this machine.".into())
    } else if settings.remote_enabled && !api_key_configured {
        Some("Remote Oracle LLM API key is not configured.".into())
    } else if provider_api_key_configured && !dedicated_api_key_configured {
        Some(format!(
            "Using the saved {} provider token for Oracle LLM requests.",
            llm_provider_label(&settings.provider)
        ))
    } else {
        None
    };
    Ok(OracleLlmSettingsStatus {
        settings,
        api_key_configured,
        status: status.into(),
        message,
    })
}

/// Single source of truth for "is an Oracle LLM answer key configured?".
///
/// Both the Settings status command and the doctor's provider check derive from
/// THIS function (via [`oracle_llm_settings_status`]), so they can never disagree:
/// the previous doctor path resolved the key through a *different* code path
/// (`resolve_oracle_llm_api_key`) than the status command, which let the doctor
/// report "no provider key" while the status reported configured (Bug B). Any
/// vault read failure degrades to `false` (treated as "not configured"), which
/// the UI surfaces as an actionable remediation rather than a crash. The key
/// value itself never leaves the vault layer — only the boolean.
pub fn oracle_llm_api_key_present() -> bool {
    oracle_llm_settings_status()
        .map(|status| status.api_key_configured)
        .unwrap_or(false)
}

fn llm_provider_label(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => "OpenAI",
        "omlx" => "oMLX (local)",
        "ollama" => "Ollama (local)",
        _ => "selected",
    }
}

fn sanitize_oracle_llm_settings(settings: &OracleLlmSettings) -> Result<OracleLlmSettings, String> {
    let provider = settings.provider.trim().to_ascii_lowercase();
    // Empty provider + disabled = user explicitly turned off answer LLM.
    if provider.is_empty() && !settings.remote_enabled {
        return Ok(OracleLlmSettings {
            provider: String::new(),
            model: String::new(),
            base_url: None,
            remote_enabled: false,
        });
    }
    let allowed = ["openai", "omlx", "ollama"];
    if !allowed.contains(&provider.as_str()) {
        return Err("Oracle LLM provider is not allowlisted.".into());
    }
    let model = settings.model.trim();
    if model.is_empty() || model.len() > 160 {
        return Err("Oracle LLM model is invalid.".into());
    }
    let remote_enabled = settings.remote_enabled;
    let base_url = sanitize_llm_base_url(&provider, settings.base_url.as_deref())?;
    Ok(OracleLlmSettings {
        provider,
        model: model.into(),
        base_url,
        remote_enabled,
    })
}

fn sanitize_llm_base_url(provider: &str, base_url: Option<&str>) -> Result<Option<String>, String> {
    base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if matches!(provider, "omlx" | "ollama") {
                // LOCAL providers: loopback-only, http allowed (no TLS on
                // 127.0.0.1), credentials/placeholders still rejected.
                let lower = value.to_ascii_lowercase();
                let loopback = [
                    "http://127.0.0.1",
                    "https://127.0.0.1",
                    "http://localhost",
                    "https://localhost",
                    "http://[::1]",
                    "https://[::1]",
                ]
                .iter()
                .any(|prefix| {
                    lower.strip_prefix(prefix).is_some_and(|rest| {
                        rest.is_empty() || rest.starts_with(':') || rest.starts_with('/')
                    })
                });
                if !loopback || value.contains('@') || value.contains('<') || value.contains('>') {
                    return Err(
                        "Local Oracle LLM base URL must stay on loopback (127.0.0.1).".to_string(),
                    );
                }
                return Ok(value.to_string());
            }
            if !value.starts_with("https://")
                || value.contains('@')
                || value.contains('<')
                || value.contains('>')
            {
                return Err(
                    "Oracle LLM base URL must be HTTPS without credentials or placeholders."
                        .to_string(),
                );
            }
            reject_ssrf_remote_host(value)?;
            Ok(value.to_string())
        })
        .transpose()
}

/// Generic SSRF guard for a remote (non-local) Oracle LLM base URL. Mirrors the
/// Censor Cloud validator: the endpoint may be ANY public https host (no
/// per-provider pinning), but loopback / IP-literals / cloud-metadata / intranet
/// names are refused so a saved base URL can't exfiltrate code to an internal
/// target. `value` is already known to start with `https://` and to contain no
/// `@`/`<`/`>`.
///
/// NOTE: this is a *string-level* guard (hostname allow/deny); it CANNOT defend
/// against DNS rebinding (e.g. `*.nip.io` → 127.0.0.1). Full protection needs
/// post-DNS IP filtering in the HTTP connector, tracked as a follow-up — mirroring
/// the identical limitation in the Censor Cloud validator (`censor/gemma.rs`).
fn reject_ssrf_remote_host(value: &str) -> Result<(), String> {
    // Reject invisible / bidi / control characters early (mirrors the Censor Cloud
    // validator's `is_forbidden_command_char` check) — these can be used for
    // spoofing or hiding the true authority in a URL.
    if value.chars().any(crate::backend::mini_coder::is_forbidden_command_char) {
        return Err("Oracle LLM base URL must not contain control, bidi or invisible characters.".to_string());
    }
    let rest = value.strip_prefix("https://").unwrap_or(value);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("Oracle LLM base URL must include a host.".to_string());
    }
    if authority.starts_with('[') {
        return Err("Oracle LLM base URL must be a hostname, not an IP literal.".to_string());
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(p) = port {
        if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit())
            || p.parse::<u32>().map(|n| n == 0 || n > 65535).unwrap_or(true)
        {
            return Err("Oracle LLM base URL has an invalid port.".to_string());
        }
    }
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "localhost" {
        return Err("Oracle LLM base URL must be a remote host (not localhost).".to_string());
    }
    let labels: Vec<&str> = host.split('.').collect();
    // Reject any all-numeric host (dotted-decimal IPv4 AND partial shorthands like
    // `127.1` / `127.0.1`, which getaddrinfo expands to 127.0.0.1). A real FQDN always
    // has an alphabetic TLD, so an all-numeric-label host is never a legitimate name.
    let is_numeric_host = !labels.is_empty()
        && labels.iter().all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    if is_numeric_host {
        return Err("Oracle LLM base URL must be a hostname, not an IP literal.".to_string());
    }
    if host_lower == "metadata.google.internal"
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".local")
    {
        return Err("Oracle LLM base URL targets a disallowed intranet/metadata host.".to_string());
    }
    if !host.contains('.') {
        return Err("Oracle LLM base URL must be a fully-qualified host (needs a dot).".to_string());
    }
    if !labels.iter().all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) {
        return Err("Oracle LLM base URL has an invalid host label.".to_string());
    }
    Ok(())
}

/// Storage-slot identifier for the dedicated Oracle LLM key entry.
///
/// This is PURELY a keyring slot name — the actual LLM endpoint always comes
/// from `settings.base_url` independently. A user has ONE key per provider, so
/// the scope depends on the PROVIDER ONLY (normalized lowercase/trim). It must
/// NOT include `base_url`: doing so moved the key to a different slot whenever
/// the user edited their base URL, making the key appear "not saved" or stale.
fn oracle_llm_key_scope(settings: &OracleLlmSettings) -> String {
    let mut hasher = Sha256::new();
    hasher.update(settings.provider.trim().to_ascii_lowercase().as_bytes());
    hex::encode(hasher.finalize())
}

/// LEGACY storage-slot identifier (provider + base_url) used before the scope
/// was made provider-only. Kept solely so the migration path can locate and
/// read/clean keys written by older builds under the current base_url.
fn legacy_oracle_llm_key_scope(settings: &OracleLlmSettings) -> String {
    let mut hasher = Sha256::new();
    hasher.update(settings.provider.trim().to_ascii_lowercase().as_bytes());
    hasher.update(b"\n");
    hasher.update(
        settings
            .base_url
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .as_bytes(),
    );
    hex::encode(hasher.finalize())
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_oracle_index_preferences_in_tests_never_touches_the_keyring() {
        // The cfg(test) twin returns defaults (or the override) WITHOUT any
        // keyring call: on a dev machine the real keychain item's ACL does not
        // include the per-build test binary, and get_password() would block
        // forever on an authorization prompt (the 2026-07-03 hanging-resolver
        // finding). If this test ever hangs or prompts, the seam regressed.
        let defaults = read_oracle_index_preferences().expect("defaults must load");
        assert_eq!(defaults.auto_watch_on_unlock, true);

        let mut custom = default_oracle_index_preferences();
        custom.auto_watch_on_unlock = false;
        set_oracle_index_preferences_override_for_test(Some(custom));
        let overridden = read_oracle_index_preferences().expect("override must load");
        assert_eq!(overridden.auto_watch_on_unlock, false);

        set_oracle_index_preferences_override_for_test(None);
        let restored = read_oracle_index_preferences().expect("defaults again");
        assert_eq!(restored.auto_watch_on_unlock, true);
    }

    /// F40: share the crate-wide DEV unlock / env test lock with `state::DEV_UNLOCK_ENV_TEST_LOCK`
    /// so vault prefs env mutations never race with unlock-env tests.
    fn f31_prefs_env_lock() -> &'static std::sync::Mutex<()> {
        // Same mutex instance as state::DEV_UNLOCK_ENV_TEST_LOCK (process-wide).
        &crate::backend::state::DEV_UNLOCK_ENV_TEST_LOCK
    }

    /// F31: DEV file store round-trips without keychain (the path used under
    /// `dev_unlock_enabled()` so ad-hoc-signed rebuilds never freeze UI).
    #[test]
    fn f31_oracle_index_prefs_dev_file_roundtrip() {
        let _guard = f31_prefs_env_lock().lock().expect("f31 prefs env lock");
        let dir = std::env::temp_dir().join(format!(
            "devboule-f31-prefs-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.json");
        let prev = std::env::var_os("DEVBOULE_ORACLE_INDEX_PREFS_PATH");
        std::env::set_var("DEVBOULE_ORACLE_INDEX_PREFS_PATH", &path);

        let mut prefs = default_oracle_index_preferences();
        prefs.auto_watch_on_unlock = false;
        prefs.index_mode = Some("commit".into());
        prefs.index_root = None;
        let raw = serde_json::to_string(&prefs).unwrap();
        write_oracle_index_preferences_dev_file(&raw).expect("write dev prefs");
        assert!(path.is_file(), "prefs file must exist at {}", path.display());

        let loaded = read_oracle_index_preferences_dev_file().expect("read dev prefs");
        assert_eq!(loaded.auto_watch_on_unlock, false);
        assert_eq!(loaded.index_mode.as_deref(), Some("commit"));

        match prev {
            Some(v) => std::env::set_var("DEVBOULE_ORACLE_INDEX_PREFS_PATH", v),
            None => std::env::remove_var("DEVBOULE_ORACLE_INDEX_PREFS_PATH"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn f39_oracle_index_prefs_have_workspace_detects_root() {
        let mut empty = default_oracle_index_preferences();
        empty.index_root = None;
        assert!(!oracle_index_prefs_have_workspace(&empty));
        empty.index_root = Some("".into());
        assert!(!oracle_index_prefs_have_workspace(&empty));
        empty.index_root = Some("   ".into());
        assert!(!oracle_index_prefs_have_workspace(&empty));
        empty.index_root = Some("/Users/user/Projects/sandbox".into());
        assert!(oracle_index_prefs_have_workspace(&empty));
    }

    #[test]
    fn f39_should_migrate_only_when_file_missing_and_workspace_set() {
        let mut with_ws = default_oracle_index_preferences();
        with_ws.index_root = Some("/Users/user/Projects/sandbox".into());
        let mut empty = default_oracle_index_preferences();
        empty.index_root = None;
        assert!(f39_should_migrate_keychain_to_dev(false, &with_ws));
        assert!(
            !f39_should_migrate_keychain_to_dev(true, &with_ws),
            "must not overwrite existing DEV file"
        );
        assert!(
            !f39_should_migrate_keychain_to_dev(false, &empty),
            "empty keychain prefs → no migrate"
        );
    }

    #[test]
    fn f39_apply_migrate_writes_dev_file_from_injected_keychain_prefs() {
        // Real path: inject keychain-shaped prefs (no OS keychain) → file appears.
        let dir = std::env::temp_dir().join(format!(
            "devboule-f39-migrate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".oracle-index-preferences.json");
        assert!(!path.exists());

        let mut from_keychain = default_oracle_index_preferences();
        from_keychain.index_root = Some("/Users/user/Projects/devboule-website".into());

        let out = f39_apply_migrate_keychain_to_dev_file(&path, &from_keychain)
            .expect("migrate write");
        assert_eq!(
            out.index_root.as_deref(),
            Some("/Users/user/Projects/devboule-website")
        );
        assert!(path.is_file(), "DEV prefs file must be created");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("devboule-website"));

        // Second call with file present must not require rewrite (decision false).
        assert!(!f39_should_migrate_keychain_to_dev(true, &from_keychain));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F31: missing DEV prefs file falls back to defaults (never keychain).
    #[test]
    fn f31_oracle_index_prefs_dev_file_missing_returns_defaults() {
        let _guard = f31_prefs_env_lock().lock().expect("f31 prefs env lock");
        let path = std::env::temp_dir().join(format!(
            "devboule-f31-missing-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let prev = std::env::var_os("DEVBOULE_ORACLE_INDEX_PREFS_PATH");
        std::env::set_var("DEVBOULE_ORACLE_INDEX_PREFS_PATH", &path);
        let loaded = read_oracle_index_preferences_dev_file().expect("defaults");
        assert!(loaded.auto_watch_on_unlock);
        match prev {
            Some(v) => std::env::set_var("DEVBOULE_ORACLE_INDEX_PREFS_PATH", v),
            None => std::env::remove_var("DEVBOULE_ORACLE_INDEX_PREFS_PATH"),
        }
    }

    /// RAII guard that snapshots EVERY Oracle LLM credential slot the mutating
    /// `#[ignore]` tests can touch (settings entry, provider-only key slot,
    /// legacy default-base_url key slot, unscoped legacy key slot) and restores
    /// them on drop. This prevents a test run from destroying the developer's
    /// REAL Oracle key as a side effect of the in-test `delete_*`/`save_*` calls
    /// — which is exactly how the live key was lost once before this guard
    /// existed. Never prints any key value.
    struct OracleVaultSnapshot {
        settings: Option<String>,
        primary_key: Option<String>,
        legacy_default_key: Option<String>,
        unscoped_legacy_key: Option<String>,
    }

    impl OracleVaultSnapshot {
        fn default_openai_settings() -> OracleLlmSettings {
            let mut settings = default_oracle_llm_settings();
            settings.base_url = Some("https://api.openai.com/v1/chat/completions".into());
            settings
        }

        fn capture() -> Self {
            let probe = Self::default_openai_settings();
            Self {
                settings: oracle_llm_settings_entry().unwrap().get_password().ok(),
                primary_key: oracle_llm_api_key_entry(&probe)
                    .unwrap()
                    .get_password()
                    .ok(),
                legacy_default_key: legacy_oracle_llm_api_key_entry_for_settings(&probe)
                    .unwrap()
                    .get_password()
                    .ok(),
                unscoped_legacy_key: legacy_oracle_llm_api_key_entry()
                    .unwrap()
                    .get_password()
                    .ok(),
            }
        }

        fn restore_slot(entry: Result<Entry, String>, value: &Option<String>) {
            if let Ok(entry) = entry {
                match value {
                    Some(raw) => {
                        let _ = entry.set_password(raw);
                    }
                    None => {
                        let _ = entry.delete_credential();
                    }
                }
            }
        }
    }

    impl Drop for OracleVaultSnapshot {
        fn drop(&mut self) {
            let probe = Self::default_openai_settings();
            Self::restore_slot(oracle_llm_settings_entry(), &self.settings);
            Self::restore_slot(oracle_llm_api_key_entry(&probe), &self.primary_key);
            Self::restore_slot(
                legacy_oracle_llm_api_key_entry_for_settings(&probe),
                &self.legacy_default_key,
            );
            Self::restore_slot(legacy_oracle_llm_api_key_entry(), &self.unscoped_legacy_key);
        }
    }

    /// READ-ONLY diagnostic: print exactly what the app's status command reports
    /// from the REAL credential store — non-secret fields only (provider,
    /// base_url, booleans, status, message). NEVER prints the key value. Used to
    /// reconcile "the key won't save" against the persisted vault state. Marked
    /// `#[ignore]`; run with `--ignored --nocapture`. Mutates nothing.
    #[test]
    #[ignore = "reads the real OS credential store; run with --ignored --nocapture to inspect the live Oracle LLM status"]
    fn oracle_llm_status_probe_readonly() {
        let settings = read_oracle_llm_settings().unwrap_or_else(|_| default_oracle_llm_settings());
        eprintln!("[probe] provider={}", settings.provider);
        eprintln!("[probe] base_url={:?}", settings.base_url);
        eprintln!("[probe] remote_enabled={}", settings.remote_enabled);
        eprintln!("[probe] key_scope={}", oracle_llm_key_scope(&settings));
        let dedicated = read_oracle_llm_api_key_for_settings(&settings);
        eprintln!(
            "[probe] dedicated_key_read = {:?}",
            dedicated.map(|o| o.is_some())
        );
        let provider_token = read_llm_provider_token(&settings.provider);
        eprintln!(
            "[probe] provider_token_read = {:?}",
            provider_token.map(|o| o.is_some())
        );
        match oracle_llm_settings_status() {
            Ok(s) => eprintln!(
                "[probe] STATUS status={} api_key_configured={} message={:?}",
                s.status, s.api_key_configured, s.message
            ),
            Err(e) => eprintln!("[probe] STATUS returned Err: {e}"),
        }
    }

    /// Bug B regression: a saved Oracle LLM API key must round-trip through the
    /// REAL OS credential store and be reported as configured by BOTH the status
    /// command and the doctor's provider check (single source of truth:
    /// [`oracle_llm_api_key_present`]).
    ///
    /// Root cause this guards: `keyring` ships NO default features, so without an
    /// explicit platform-native feature it used the in-memory `mock` store where
    /// `set_password` returned Ok but a freshly constructed `Entry` read back
    /// `NoEntry` — the settings + key never persisted and the status stayed
    /// `missing_api_key`. Marked `#[ignore]` because it MUTATES the developer's
    /// real OS credential store; run with `--ignored` to verify the persistence
    /// fix. It snapshots and restores any pre-existing settings entry.
    #[test]
    #[ignore = "mutates the real OS credential store; run with --ignored to verify keyring persistence"]
    fn oracle_llm_api_key_persists_and_is_reported_configured() {
        let _snapshot = OracleVaultSnapshot::capture();
        let mut settings = default_oracle_llm_settings();
        settings.base_url = Some("https://api.openai.com/v1/chat/completions".into());

        let _ = delete_oracle_llm_api_key();
        let save_status =
            save_oracle_llm_settings(&settings, Some("dummy-openai-key-123456789")).unwrap();
        assert_eq!(
            save_status.status, "configured",
            "save must report the key as configured once it persists"
        );
        assert!(save_status.api_key_configured);

        // Status command: the canonical "key present" computation.
        let status = oracle_llm_settings_status().unwrap();
        assert!(
            status.api_key_configured,
            "status must read the saved key back as configured"
        );
        assert_eq!(status.status, "configured");

        // Doctor's provider check shares the EXACT same vault logic.
        assert!(
            oracle_llm_api_key_present(),
            "doctor provider check must agree with the status command"
        );

        // The persisted settings must round-trip the base_url that keys the entry.
        let saved = read_oracle_llm_settings().unwrap();
        assert_eq!(
            saved.base_url.as_deref(),
            Some("https://api.openai.com/v1/chat/completions"),
            "base_url must survive persistence so the key-entry scope matches"
        );

        // Cleanup: drop the dummy key. The snapshot guard restores every slot.
        let _ = delete_oracle_llm_api_key();
    }

    #[test]
    fn oracle_llm_key_scope_is_stable_across_base_url_and_differs_by_provider() {
        let openai = OracleLlmSettings {
            provider: "openai".into(),
            model: "model-a".into(),
            base_url: Some("https://api.openai.com/v1/chat/completions".into()),
            remote_enabled: true,
        };
        // Same provider, DIFFERENT base_url (e.g. a custom deployment).
        let openai_custom_url = OracleLlmSettings {
            base_url: Some("https://api.openai.com/v1/deployments/abc/chat/completions".into()),
            ..openai.clone()
        };
        // Same provider, NO base_url.
        let openai_no_url = OracleLlmSettings {
            base_url: None,
            ..openai.clone()
        };
        let ollama = OracleLlmSettings {
            provider: "ollama".into(),
            base_url: Some("http://127.0.0.1:11434/v1/chat/completions".into()),
            ..openai.clone()
        };
        let omlx = OracleLlmSettings {
            provider: "omlx".into(),
            base_url: Some("http://127.0.0.1:8000/v1/chat/completions".into()),
            ..openai.clone()
        };

        // STABLE across base_url for the same provider.
        assert_eq!(
            oracle_llm_key_scope(&openai),
            oracle_llm_key_scope(&openai_custom_url),
            "scope must not change when only base_url changes"
        );
        assert_eq!(
            oracle_llm_key_scope(&openai),
            oracle_llm_key_scope(&openai_no_url),
            "scope must not change when base_url is dropped"
        );

        // DIFFERS across providers.
        assert_ne!(
            oracle_llm_key_scope(&openai),
            oracle_llm_key_scope(&ollama)
        );
        assert_ne!(
            oracle_llm_key_scope(&openai),
            oracle_llm_key_scope(&omlx)
        );

        // Provider normalization: case/whitespace must not split the slot.
        let openai_messy = OracleLlmSettings {
            provider: "  OpenAI  ".into(),
            ..openai.clone()
        };
        assert_eq!(
            oracle_llm_key_scope(&openai),
            oracle_llm_key_scope(&openai_messy),
            "provider must be normalized (trim/lowercase) before hashing"
        );
    }

    /// The dedicated key entry NAME is provider-only-scoped and STABLE across
    /// base_url edits. The `:primary:` segment is retained verbatim so a key saved
    /// by an older build (which wrote `oracle:llm_api_key:primary:{scope}`) still
    /// resolves with no migration. The role tag is the only structure left now
    /// that the LLM-to-LLM fallback (and its second slot) has been removed.
    #[test]
    fn oracle_llm_entry_name_is_provider_scoped_and_stable_across_base_url() {
        let mut openai = default_oracle_llm_settings();
        openai.base_url = Some("https://api.openai.com/v1/chat/completions".into());

        let name = oracle_llm_api_key_entry_name(&openai);

        // The hash suffix (provider-only scope) keys the slot under `:primary:`.
        let scope = oracle_llm_key_scope(&openai);
        assert_eq!(name, format!("oracle:llm_api_key:primary:{scope}"));

        // Stable across base_url edits (provider-only scope).
        let openai_custom_url = OracleLlmSettings {
            base_url: Some("https://api.openai.com/v1/deployments/abc/chat/completions".into()),
            ..openai.clone()
        };
        assert_eq!(
            name,
            oracle_llm_api_key_entry_name(&openai_custom_url),
            "the dedicated-key slot must be stable across base_url"
        );

        // The name can NEVER equal a LEGACY base_url-scoped name: the legacy name
        // is `oracle:llm_api_key:<hex>` (no role word), so the segment after the
        // second colon is hex, never "primary".
        let legacy_scope = legacy_oracle_llm_key_scope(&openai);
        let legacy_name = format!("oracle:llm_api_key:{legacy_scope}");
        assert_ne!(name, legacy_name);
    }

    /// Bug reproduction (now fixed): the user saves a key with one base_url,
    /// then the app reads it back with a DIFFERENT base_url for the same
    /// provider. Before the scope fix the read landed in a different slot and
    /// returned None ("key won't save"). With the provider-only scope the key
    /// round-trips regardless of base_url edits. Mutates the real OS store, so
    /// `#[ignore]`; snapshots and restores the prior settings/key.
    #[test]
    #[ignore = "mutates the real OS credential store; run with --ignored to verify the base_url-change round-trip"]
    fn oracle_llm_key_round_trips_when_base_url_changes_between_save_and_read() {
        let _snapshot = OracleVaultSnapshot::capture();

        let mut save_settings = default_oracle_llm_settings();
        save_settings.base_url = Some("https://api.openai.com/v1/chat/completions".into());

        let _ = delete_oracle_llm_api_key();
        let status =
            save_oracle_llm_settings(&save_settings, Some("dummy-openai-key-123456789")).unwrap();
        assert_eq!(status.status, "configured");

        // Same provider, DIFFERENT base_url — simulates the desync scenario.
        let mut read_settings = save_settings.clone();
        read_settings.base_url =
            Some("https://api.openai.com/v1/deployments/xyz/chat/completions".into());

        let read_back = read_oracle_llm_api_key_for_settings(&read_settings).unwrap();
        assert_eq!(
            read_back.as_deref(),
            Some("dummy-openai-key-123456789"),
            "key must round-trip even though base_url changed between save and read"
        );

        // Cleanup: drop the dummy key. The snapshot guard restores every slot.
        let _ = delete_oracle_llm_api_key();
    }

    /// Migration: a key stored by an OLDER build under the LEGACY base_url-scoped
    /// slot must still be read back as configured after the scope change (read
    /// fallback), and a subsequent save must migrate it to the provider-only
    /// slot AND delete the legacy orphan. Mutates the real OS store, so
    /// `#[ignore]`. Restores prior settings/key.
    #[test]
    #[ignore = "mutates the real OS credential store; run with --ignored to verify legacy-scope migration"]
    fn oracle_llm_legacy_scoped_key_migrates_to_provider_only_slot() {
        let _snapshot = OracleVaultSnapshot::capture();

        let mut settings = default_oracle_llm_settings();
        settings.base_url = Some("https://api.openai.com/v1/chat/completions".into());

        // Start clean, then plant a key in the LEGACY base_url-scoped slot only.
        let _ = delete_oracle_llm_api_key();
        legacy_oracle_llm_api_key_entry_for_settings(&settings)
            .unwrap()
            .set_password("legacy-openai-key-123456789")
            .unwrap();

        // The migration read-fallback must find it under the legacy slot.
        let read_back = read_oracle_llm_api_key_for_settings(&settings).unwrap();
        assert_eq!(read_back.as_deref(), Some("legacy-openai-key-123456789"));

        // A save migrates to the provider-only slot and removes the legacy orphan.
        let status = save_oracle_llm_settings(&settings, Some("new-openai-key-7890abcdef")).unwrap();
        assert_eq!(status.status, "configured");
        assert_eq!(
            oracle_llm_api_key_entry(&settings)
                .unwrap()
                .get_password()
                .unwrap(),
            "new-openai-key-7890abcdef",
            "key must live in the provider-only slot after save"
        );
        assert!(
            matches!(
                legacy_oracle_llm_api_key_entry_for_settings(&settings)
                    .unwrap()
                    .get_password(),
                Err(KeyringError::NoEntry)
            ),
            "legacy base_url-scoped orphan must be deleted after migration save"
        );

        // Cleanup: drop the dummy key. The snapshot guard restores every slot.
        let _ = delete_oracle_llm_api_key();
    }

    #[test]
    fn llm_provider_credential_account_returns_none_for_all() {
        // The generic `openai` provider has no shared provider token; the dedicated
        // Oracle key is the only source.
        assert_eq!(llm_provider_credential_account("openai"), None);
        assert_eq!(llm_provider_credential_account("omlx"), None);
        assert_eq!(llm_provider_credential_account("ollama"), None);
        // Legacy / removed providers also map to None.
        assert_eq!(llm_provider_credential_account("openrouter"), None);
        assert_eq!(llm_provider_credential_account("deepseek"), None);
        assert_eq!(llm_provider_credential_account("scaleway"), None);
        assert_eq!(llm_provider_credential_account("infomaniak"), None);
        assert_eq!(llm_provider_credential_account("mistral"), None);
    }

    #[test]
    fn legacy_remote_llm_providers_are_rejected() {
        for provider in ["scaleway", "infomaniak", "mistral", "openrouter", "deepseek"] {
            let settings = OracleLlmSettings {
                provider: provider.into(),
                model: "model-a".into(),
                base_url: Some("https://example.com/v1/chat/completions".into()),
                remote_enabled: true,
            };

            assert!(sanitize_oracle_llm_settings(&settings).is_err());
        }
    }

    #[test]
    fn ssrf_guard_allows_valid_remote_host() {
        let settings = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://api.openai.com/v1/chat/completions".into()),
            remote_enabled: true,
        };

        let out = sanitize_oracle_llm_settings(&settings).expect("valid openai URL must be accepted");
        assert_eq!(out.provider, "openai");
        assert!(out.remote_enabled);
    }

    #[test]
    fn ssrf_guard_rejects_loopback_and_metadata_hosts() {
        for bad_url in [
            "https://169.254.169.254/latest/meta-data/",
            "https://localhost/some/path",
            "https://metadata.google.internal/computeMetadata/v1/",
            "https://my-app.internal/api",
            "https://host.local/api",
        ] {
            let settings = OracleLlmSettings {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                base_url: Some(bad_url.into()),
                remote_enabled: true,
            };

            assert!(
                sanitize_oracle_llm_settings(&settings).is_err(),
                "SSRF guard must reject: {bad_url}"
            );
        }
    }

    #[test]
    fn ssrf_guard_rejects_ipv6_bracketed_and_bracketless() {
        for bad_url in [
            "https://[::1]/v1/chat/completions",
            "https://::1/v1/chat/completions",
        ] {
            let settings = OracleLlmSettings {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                base_url: Some(bad_url.into()),
                remote_enabled: true,
            };
            assert!(
                sanitize_oracle_llm_settings(&settings).is_err(),
                "SSRF guard must reject IPv6: {bad_url}"
            );
        }
    }

    #[test]
    fn ssrf_guard_rejects_trailing_dot_host() {
        for bad_url in [
            "https://localhost./v1",
            "https://metadata.google.internal./v1",
        ] {
            let settings = OracleLlmSettings {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                base_url: Some(bad_url.into()),
                remote_enabled: true,
            };
            assert!(
                sanitize_oracle_llm_settings(&settings).is_err(),
                "SSRF guard must reject trailing-dot host: {bad_url}"
            );
        }
    }

    #[test]
    fn ssrf_guard_rejects_ipv4_shorthand_loopback_bypasses() {
        // Partial dotted-decimal shorthands (`127.1`, `127.0.1`, `10.1`) have fewer
        // than 4 labels, so the old quad-only check missed them. getaddrinfo expands
        // e.g. `127.1` → `127.0.0.1`, making these real loopback-SSRF vectors.
        for bad_url in [
            "https://127.1/v1/chat/completions",
            "https://127.0.1/v1",
            "https://10.1/v1",
        ] {
            let settings = OracleLlmSettings {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                base_url: Some(bad_url.into()),
                remote_enabled: true,
            };
            assert!(
                sanitize_oracle_llm_settings(&settings).is_err(),
                "SSRF guard must reject IPv4 shorthand: {bad_url}"
            );
        }
    }

    #[test]
    fn ssrf_guard_allows_numeric_subdomain_with_alpha_labels() {
        // A host with a numeric subdomain label (e.g. `192`) but alphabetic TLD
        // labels is a legitimate FQDN — the all-numeric check must NOT reject it.
        let settings = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://192.host.openai.com/v1/chat/completions".into()),
            remote_enabled: true,
        };
        assert!(
            sanitize_oracle_llm_settings(&settings).is_ok(),
            "numeric subdomain FQDN must be accepted"
        );
    }

    #[test]
    fn ssrf_guard_rejects_port_zero_and_above_65535() {
        for bad_url in [
            "https://api.openai.com:0/v1/chat/completions",
            "https://api.openai.com:65536/v1",
        ] {
            let settings = OracleLlmSettings {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                base_url: Some(bad_url.into()),
                remote_enabled: true,
            };
            assert!(
                sanitize_oracle_llm_settings(&settings).is_err(),
                "SSRF guard must reject port out of range: {bad_url}"
            );
        }
    }

    #[test]
    fn ssrf_guard_accepts_valid_port() {
        let settings = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://api.openai.com:8443/v1/chat/completions".into()),
            remote_enabled: true,
        };
        assert!(sanitize_oracle_llm_settings(&settings).is_ok());
    }

    #[test]
    fn ssrf_guard_cross_wiring_local_provider_remote_url_and_vice_versa() {
        // A LOCAL provider (omlx) with a remote https base_url must be rejected
        // (local providers must use loopback). The sanitize path catches this
        // before the SSRF guard even fires.
        let local_with_remote = OracleLlmSettings {
            provider: "omlx".into(),
            model: "qwen".into(),
            base_url: Some("https://api.openai.com/v1".into()),
            remote_enabled: true,
        };
        assert!(sanitize_oracle_llm_settings(&local_with_remote).is_err());

        // A REMOTE provider (openai) with a loopback base_url must be rejected
        // (quad check: 127.0.0.1 is a bare IPv4 literal).
        let remote_with_loopback = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://127.0.0.1/v1".into()),
            remote_enabled: true,
        };
        assert!(sanitize_oracle_llm_settings(&remote_with_loopback).is_err());
    }

    #[test]
    fn ssrf_guard_rejects_control_or_bidi_char_in_host() {
        let settings = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://ev\u{202e}il.com/v1".into()),
            remote_enabled: true,
        };
        assert!(
            sanitize_oracle_llm_settings(&settings).is_err(),
            "SSRF guard must reject bidi char in URL"
        );
    }

    /// KNOWN LIMITATION: DNS rebinding (e.g. `*.nip.io` resolving to 127.0.0.1)
    /// passes the string-level SSRF guard because the guard checks hostnames, not
    /// resolved IPs. Full protection requires post-DNS SocketAddr filtering in the
    /// HTTP connector — tracked as a follow-up, mirroring the identical limitation
    /// in the Censor Cloud validator (`censor/gemma.rs`).
    #[test]
    fn dns_rebinding_host_passes_string_guard_known_limitation() {
        let settings = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://127.0.0.1.nip.io/v1/chat/completions".into()),
            remote_enabled: true,
        };
        assert!(
            sanitize_oracle_llm_settings(&settings).is_ok(),
            "string guard cannot catch DNS rebinding — this documents the accepted gap"
        );
    }

    #[test]
    fn ssrf_guard_rejects_single_label_host() {
        let settings = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://localhostt/api".into()),
            remote_enabled: true,
        };

        assert!(sanitize_oracle_llm_settings(&settings).is_err());
    }

    #[test]
    fn sanitize_accepts_local_providers_loopback_only() {
        // LOCAL providers are back (2026-06-12, loopback-only): omlx/ollama are
        // allowlisted, a None base_url is fine (the python answerer fills the
        // loopback default), a loopback http URL is accepted, and any
        // non-loopback URL is rejected fail-closed.
        let settings = OracleLlmSettings {
            provider: "ollama".into(),
            model: "qwen3.5:4b".into(),
            base_url: None,
            remote_enabled: true,
        };
        assert!(sanitize_oracle_llm_settings(&settings).is_ok());

        let loopback = OracleLlmSettings {
            provider: "omlx".into(),
            model: "qwen".into(),
            base_url: Some("http://127.0.0.1:8000/v1".into()),
            remote_enabled: true,
        };
        assert!(sanitize_oracle_llm_settings(&loopback).is_ok());

        let off_machine = OracleLlmSettings {
            provider: "omlx".into(),
            model: "qwen".into(),
            base_url: Some("http://evil.example.com:8000/v1".into()),
            remote_enabled: true,
        };
        assert!(sanitize_oracle_llm_settings(&off_machine).is_err());

        let localhost_https = OracleLlmSettings {
            provider: "ollama".into(),
            model: "qwen".into(),
            base_url: Some("http://localhost:11434/v1".into()),
            remote_enabled: true,
        };
        assert!(sanitize_oracle_llm_settings(&localhost_https).is_ok());
    }

    #[test]
    fn sanitize_accepts_openai_provider() {
        let settings = OracleLlmSettings {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            base_url: Some("https://api.openai.com/v1/chat/completions".into()),
            remote_enabled: true,
        };

        let out = sanitize_oracle_llm_settings(&settings).expect("openai must be accepted");
        assert_eq!(out.provider, "openai");
        assert!(out.remote_enabled);
    }

    // ── sanitize_oracle_index_preferences: index_mode coercion ────────────────

    fn prefs_with_mode(mode: Option<&str>) -> OracleIndexPreferences {
        OracleIndexPreferences {
            auto_watch_on_unlock: true,
            index_root: None,
            index_roots: vec![],
            index_mode: mode.map(str::to_owned),
        }
    }

    #[test]
    fn sanitize_oracle_index_preferences_keeps_commit() {
        let out = sanitize_oracle_index_preferences(&prefs_with_mode(Some("commit")))
            .expect("commit is valid");
        assert_eq!(out.index_mode.as_deref(), Some("commit"));
    }

    #[test]
    fn sanitize_oracle_index_preferences_keeps_watch() {
        let out = sanitize_oracle_index_preferences(&prefs_with_mode(Some("watch")))
            .expect("watch is valid");
        assert_eq!(out.index_mode.as_deref(), Some("watch"));
    }

    #[test]
    fn sanitize_oracle_index_preferences_coerces_garbage_to_none() {
        let out = sanitize_oracle_index_preferences(&prefs_with_mode(Some("garbage")))
            .expect("sanitize must succeed even for unknown mode");
        assert_eq!(
            out.index_mode, None,
            "unknown mode values must be coerced to None"
        );
    }

    #[test]
    fn sanitize_oracle_index_preferences_coerces_empty_to_none() {
        let out = sanitize_oracle_index_preferences(&prefs_with_mode(Some("")))
            .expect("sanitize must succeed for empty mode");
        assert_eq!(out.index_mode, None, "empty mode must be coerced to None");
    }

    #[test]
    fn sanitize_oracle_index_preferences_keeps_none() {
        let out =
            sanitize_oracle_index_preferences(&prefs_with_mode(None)).expect("None mode is valid");
        assert_eq!(out.index_mode, None);
    }

    /// Layer 2: multi-root sanitize canonicalizes and keeps both roots; primary
    /// is first. Legacy single-root still works.
    #[test]
    fn sanitize_oracle_index_preferences_multi_root_and_legacy() {
        let base = std::env::temp_dir().join(format!(
            "devboule-prefs-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let a = base.join("root-a");
        let b = base.join("root-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let multi = OracleIndexPreferences {
            auto_watch_on_unlock: true,
            index_root: Some(a.to_string_lossy().to_string()),
            index_roots: vec![
                a.to_string_lossy().to_string(),
                b.to_string_lossy().to_string(),
            ],
            index_mode: Some("watch".into()),
        };
        let out = sanitize_oracle_index_preferences(&multi).expect("multi-root ok");
        assert_eq!(out.index_roots.len(), 2, "{:?}", out.index_roots);
        assert_eq!(out.primary_index_root().as_deref(), out.index_root.as_deref());
        assert!(out.index_roots[0].contains("root-a") || out.index_roots[0].ends_with("root-a"));
        // Legacy single indexRoot only.
        let legacy = OracleIndexPreferences {
            auto_watch_on_unlock: true,
            index_root: Some(a.to_string_lossy().to_string()),
            index_roots: vec![],
            index_mode: None,
        };
        let out2 = sanitize_oracle_index_preferences(&legacy).expect("legacy ok");
        assert_eq!(out2.index_roots.len(), 1);
        assert!(out2.index_root.is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    // --- L2.4 Exa key ---------------------------------------------------------

    #[test]
    fn exa_key_save_rejects_too_short_or_whitespace_without_leaking_value() {
        // The reject path does NOT touch the keyring (it returns before set_password),
        // so it is safe to run unconditionally. The status it returns must carry the
        // present/absent shape and NEVER echo the rejected value back.
        let short = save_exa_key("abc").expect("save returns a status, not Err");
        assert!(!short.configured);
        assert_eq!(short.status, "error");
        let whitespace = save_exa_key("has space inside it").expect("status");
        assert!(!whitespace.configured);
        assert_eq!(whitespace.status, "error");
        // The status NEVER contains the raw value (write-only contract).
        for status in [&short, &whitespace] {
            let json = serde_json::to_string(status).unwrap();
            assert!(!json.contains("has space inside it"));
            assert_eq!(status.id, "exa_api_key");
        }
    }

    #[test]
    fn exa_status_struct_never_carries_the_value() {
        // The absent status (no keyring read needed for the shape assertion here, but
        // this reads the slot; on a clean machine it is absent) must report present/
        // absent ONLY — its serialized form must not include a `value`/key field.
        let status = exa_key_status().expect("status");
        let json = serde_json::to_string(&status).unwrap();
        // The struct has no value field at all; assert the wire shape stays
        // present/absent-only (id/label/configured/status/lastCheckedAt/message).
        assert!(json.contains("\"configured\""));
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"key\""));
    }

    #[test]
    #[ignore = "mutates the real OS credential store; run with --ignored to verify the Exa round-trip"]
    fn exa_key_round_trips_set_status_clear_status() {
        // Full lifecycle against the REAL keyring: set -> status(present) -> clear ->
        // status(absent). The raw value is NEVER returned by status — only the backend-
        // internal read_exa_key (used by the launch) ever sees it.
        let key = "exa-test-key-abcdef1234567890";
        // Start clean.
        let _ = delete_exa_key();
        assert!(!exa_key_status().unwrap().configured, "must start absent");

        let after_set = save_exa_key(key).unwrap();
        assert!(after_set.configured, "set must report present");
        assert_eq!(after_set.status, "configured");
        assert!(after_set.message.is_none());
        // status(present): never the value.
        let status_present = exa_key_status().unwrap();
        assert!(status_present.configured);
        assert!(!serde_json::to_string(&status_present)
            .unwrap()
            .contains(key));
        // The backend-internal reader (launch path) DOES see the raw value.
        assert_eq!(read_exa_key().unwrap().as_deref(), Some(key));

        let after_clear = delete_exa_key().unwrap();
        assert!(!after_clear.configured, "clear must report absent");
        assert_eq!(after_clear.status, "missing");
        assert!(!exa_key_status().unwrap().configured);
        assert_eq!(read_exa_key().unwrap(), None);
    }

    // --- Cloud LLM key (opt-in Cloud mode) ------------------------------------

    #[test]
    fn cloud_llm_key_save_rejects_too_short_or_whitespace_without_leaking_value() {
        // The reject path does NOT touch the keyring (it returns before set_password),
        // so it is safe to run unconditionally. The status must carry the present/absent
        // shape and NEVER echo the rejected value back.
        let short = save_cloud_llm_key("abc").expect("save returns a status, not Err");
        assert!(!short.configured);
        assert_eq!(short.status, "error");
        let whitespace = save_cloud_llm_key("has space inside it").expect("status");
        assert!(!whitespace.configured);
        assert_eq!(whitespace.status, "error");
        for status in [&short, &whitespace] {
            let json = serde_json::to_string(status).unwrap();
            assert!(!json.contains("has space inside it"));
            assert_eq!(status.id, "cloud_llm_api_key");
        }
    }

    #[test]
    fn cloud_llm_key_save_rejects_control_characters_without_leaking_value() {
        // A key carrying an embedded ASCII control char (`\x01`) is a paste/corruption
        // error reqwest would only reject later as an opaque request failure. The save
        // path rejects it up front BEFORE touching the keyring, and never echoes the value.
        let with_ctrl = "sk-cloud\u{0001}key-1234";
        let status = save_cloud_llm_key(with_ctrl).expect("save returns a status, not Err");
        assert!(!status.configured);
        assert_eq!(status.status, "error");
        assert_eq!(status.id, "cloud_llm_api_key");
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains('\u{0001}'));
        assert!(!json.contains("sk-cloud"));
        // DEL (0x7f) is also a control char and must be rejected.
        let with_del = "sk-cloud\u{007f}key-5678";
        let del_status = save_cloud_llm_key(with_del).expect("status");
        assert!(!del_status.configured);
        assert_eq!(del_status.status, "error");
    }

    #[test]
    fn censor_cloud_key_save_rejects_too_short_whitespace_and_control_without_leaking() {
        // Same up-front (pre-keyring) reject path as the cloud-llm key, so it is safe to run
        // unconditionally and must never echo the rejected value.
        let short = save_censor_cloud_key("abc").expect("save returns a status, not Err");
        assert!(!short.configured);
        assert_eq!(short.status, "error");
        assert_eq!(short.id, "censor_cloud_api_key");

        let whitespace = save_censor_cloud_key("has space inside it").expect("status");
        assert!(!whitespace.configured);
        assert_eq!(whitespace.status, "error");

        // Non-whitespace control char (\x01) passes the whitespace guard but must be rejected.
        let with_ctrl = "sk-censor\u{0001}key-1234";
        let ctrl = save_censor_cloud_key(with_ctrl).expect("status");
        assert!(!ctrl.configured);
        assert_eq!(ctrl.status, "error");

        for status in [&short, &whitespace, &ctrl] {
            let json = serde_json::to_string(status).unwrap();
            assert!(!json.contains("has space inside it"));
            assert!(!json.contains("sk-censor"));
            assert!(!json.contains('\u{0001}'));
        }
    }

    #[test]
    fn censor_cloud_status_struct_never_carries_the_value() {
        let status = censor_cloud_key_status().expect("status");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"configured\""));
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"key\""));
    }

    #[test]
    fn cloud_llm_status_struct_never_carries_the_value() {
        // Present/absent ONLY — the serialized form must not include a value/key field.
        let status = cloud_llm_key_status().expect("status");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"configured\""));
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"key\""));
    }

    // --- F46-close: Claude setup-token ----------------------------------------

    #[test]
    fn claude_oauth_token_save_rejects_too_short_whitespace_control_without_leaking() {
        let short = save_claude_oauth_token("abc").expect("status");
        assert!(!short.configured);
        assert_eq!(short.status, "error");
        assert_eq!(short.id, "claude_oauth_token");
        assert_eq!(short.label, "Claude setup-token");
        let whitespace = save_claude_oauth_token("has space inside it").expect("status");
        assert!(!whitespace.configured);
        let with_ctrl = save_claude_oauth_token("sk-claude\u{0001}token12").expect("status");
        assert!(!with_ctrl.configured);
        for status in [&short, &whitespace, &with_ctrl] {
            let json = serde_json::to_string(status).unwrap();
            assert!(!json.contains("has space inside it"));
            assert!(!json.contains("sk-claude"));
            assert!(!json.contains('\u{0001}'));
        }
    }

    #[test]
    fn claude_oauth_token_status_never_carries_the_value() {
        let status = claude_oauth_token_status().expect("status");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"configured\""));
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"key\""));
        assert!(!json.contains("\"token\""));
        assert_eq!(status.id, "claude_oauth_token");
        assert_eq!(status.label, "Claude setup-token");
    }

    // --- F50: per-role Cloud LLM keys -----------------------------------------

    #[test]
    fn cloud_llm_role_canonicalize_accepts_aliases_rejects_unknown() {
        assert_eq!(canonicalize_cloud_llm_role("orchestrator").unwrap(), "orchestrator");
        assert_eq!(canonicalize_cloud_llm_role("main-coder").unwrap(), "main");
        assert_eq!(canonicalize_cloud_llm_role("mini-coder").unwrap(), "mini");
        assert_eq!(canonicalize_cloud_llm_role("local").unwrap(), "coder");
        let err = canonicalize_cloud_llm_role("typo-role").unwrap_err();
        assert!(err.contains("typo-role"));
        assert!(err.contains("Allowed"));
    }

    #[test]
    fn cloud_llm_key_for_role_unknown_role_errors_before_keyring() {
        let err = read_cloud_llm_key_for_role("not-a-role").unwrap_err();
        assert!(err.contains("not-a-role"));
        assert!(save_cloud_llm_key_for_role("not-a-role", "sk-long-enough-key").is_err());
        assert!(delete_cloud_llm_key_for_role("???").is_err());
        assert!(cloud_llm_key_status_for_role("bogus").is_err());
    }

    #[test]
    fn cloud_llm_key_for_role_save_rejects_bad_paste_without_leaking() {
        let short = save_cloud_llm_key_for_role("verifier", "abc").expect("status not Err");
        assert!(!short.configured);
        assert_eq!(short.status, "error");
        assert_eq!(short.id, "cloud_llm_api_key:verifier");
        let whitespace = save_cloud_llm_key_for_role("mini", "has space inside it").expect("status");
        assert!(!whitespace.configured);
        let with_ctrl = save_cloud_llm_key_for_role("main", "sk-cloud\u{0001}key-1234").expect("status");
        assert!(!with_ctrl.configured);
        for status in [&short, &whitespace, &with_ctrl] {
            let json = serde_json::to_string(status).unwrap();
            assert!(!json.contains("has space inside it"));
            assert!(!json.contains("sk-cloud"));
            assert!(!json.contains('\u{0001}'));
        }
    }

    #[test]
    fn cloud_llm_key_status_for_role_never_carries_the_value() {
        // Missing slot is safe without keyring write; status must be present/absent only.
        let status = cloud_llm_key_status_for_role("orchestrator").expect("status");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"configured\""));
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"key\""));
        assert_eq!(status.id, "cloud_llm_api_key:orchestrator");
    }

    #[test]
    #[ignore = "mutates the real OS credential store; run with --ignored to verify per-role Cloud key override/fallback"]
    fn cloud_llm_key_for_role_overrides_shared_and_falls_back() {
        let shared = "sk-shared-fallback-key-abcdef1234";
        let role_key = "sk-role-override-key-xyz78901234";
        let _ = delete_cloud_llm_key_for_role("verifier");
        let _ = delete_cloud_llm_key();
        let _ = save_cloud_llm_key(shared).unwrap();
        // Role slot absent → fall back to shared.
        assert_eq!(
            read_cloud_llm_key_for_role("verifier").unwrap().as_deref(),
            Some(shared)
        );
        assert!(!cloud_llm_key_status_for_role("verifier").unwrap().configured);
        // Role slot present → overrides shared.
        let after = save_cloud_llm_key_for_role("verifier", role_key).unwrap();
        assert!(after.configured);
        assert!(!serde_json::to_string(&after).unwrap().contains(role_key));
        assert_eq!(
            read_cloud_llm_key_for_role("verifier").unwrap().as_deref(),
            Some(role_key)
        );
        let _ = delete_cloud_llm_key_for_role("verifier").unwrap();
        assert_eq!(
            read_cloud_llm_key_for_role("verifier").unwrap().as_deref(),
            Some(shared)
        );
        let _ = delete_cloud_llm_key().unwrap();
    }

    #[test]
    #[ignore = "mutates the real OS credential store; run with --ignored to verify the Cloud key round-trip"]
    fn cloud_llm_key_round_trips_set_status_clear_status() {
        // Full lifecycle against the REAL keyring: set -> status(present) -> clear ->
        // status(absent). The raw value is NEVER returned by status — only the backend-
        // internal read_cloud_llm_key (used by the launch) ever sees it.
        let key = "sk-cloud-test-key-abcdef1234567890";
        let _ = delete_cloud_llm_key();
        assert!(
            !cloud_llm_key_status().unwrap().configured,
            "must start absent"
        );

        let after_set = save_cloud_llm_key(key).unwrap();
        assert!(after_set.configured, "set must report present");
        assert_eq!(after_set.status, "configured");
        assert!(after_set.message.is_none());
        let status_present = cloud_llm_key_status().unwrap();
        assert!(status_present.configured);
        assert!(!serde_json::to_string(&status_present)
            .unwrap()
            .contains(key));
        assert_eq!(read_cloud_llm_key().unwrap().as_deref(), Some(key));

        let after_clear = delete_cloud_llm_key().unwrap();
        assert!(!after_clear.configured, "clear must report absent");
        assert_eq!(after_clear.status, "missing");
        assert!(!cloud_llm_key_status().unwrap().configured);
        assert_eq!(read_cloud_llm_key().unwrap(), None);
    }

    // ---- Websearch parameterized key tests ------------------------------------

    #[test]
    fn websearch_key_status_rejects_unknown_provider() {
        let result = websearch_key_status("not_a_real_provider");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("not_a_real_provider"));
        assert!(msg.contains("Allowed"));
    }

    #[test]
    fn websearch_save_key_rejects_unknown_provider() {
        let result = save_websearch_key("not_a_real_provider", "some-long-key-12345678");
        assert!(result.is_err());
    }

    #[test]
    fn websearch_delete_key_rejects_unknown_provider() {
        let result = delete_websearch_key("not_a_real_provider");
        assert!(result.is_err());
    }

    #[test]
    fn websearch_save_rejects_too_short_or_whitespace_without_leaking() {
        let short = save_websearch_key("brave", "abc").expect("save returns a status");
        assert!(!short.configured);
        assert_eq!(short.status, "error");
        assert!(short.message.as_ref().unwrap().contains("Brave"));
        let whitespace = save_websearch_key("brave", "has space inside").expect("status");
        assert!(!whitespace.configured);
        assert_eq!(whitespace.status, "error");
        for s in [&short, &whitespace] {
            let json = serde_json::to_string(s).unwrap();
            assert!(!json.contains("has space inside"));
        }
    }

    /// H2-3: websearch key save must reject control characters (same as cloud LLM keys)
    /// without leaking the rejected value into the status message / JSON.
    #[test]
    fn websearch_save_rejects_control_characters_without_leaking() {
        let with_ctrl = "brave-key\u{0001}suffix-long-enough";
        let status = save_websearch_key("brave", with_ctrl).expect("status not Err");
        assert!(!status.configured);
        assert_eq!(status.status, "error");
        assert!(
            status
                .message
                .as_ref()
                .unwrap()
                .contains("control characters"),
            "expected control-char message, got: {:?}",
            status.message
        );
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains('\u{0001}'));
        assert!(!json.contains("brave-key"));
        // CR / DEL also rejected.
        let with_cr = save_websearch_key("brave", "brave-key\rsuffix-long-enough").expect("status");
        assert!(!with_cr.configured);
        assert_eq!(with_cr.status, "error");
        let with_del = save_websearch_key("brave", "brave-key\u{007f}suffix-long-enough")
            .expect("status");
        assert!(!with_del.configured);
        assert_eq!(with_del.status, "error");
    }

    /// H2-4: generic provider token paste rejects whitespace / control (pre-keyring).
    #[test]
    fn websearch_key_status_returns_present_absent_only() {
        // Exa reuses the legacy `provider:exa` entry. Present/absent status
        // must never carry a value field.
        let status = websearch_key_status("exa").expect("status");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("configured"));
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"key\""));
    }

    #[test]
    fn websearch_key_status_all_7_providers_accepted() {
        for provider in ["exa", "brave", "tavily", "perplexity", "gemini_search", "openai_search", "parallel"] {
            let status = websearch_key_status(provider).expect("status");
            assert!(status.id.contains(provider));
        }
    }

    #[test]
    fn websearch_config_provider_allowlist_covers_all_vault_ids() {
        assert!(validate_websearch_provider("exa").is_ok());
        assert!(validate_websearch_provider("brave").is_ok());
        assert!(validate_websearch_provider("tavily").is_ok());
        assert!(validate_websearch_provider("perplexity").is_ok());
        assert!(validate_websearch_provider("gemini_search").is_ok());
        assert!(validate_websearch_provider("openai_search").is_ok());
        assert!(validate_websearch_provider("parallel").is_ok());
    }
}
