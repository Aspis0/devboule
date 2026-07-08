use super::model::{
    AuxCredentialStatus, CloudflareAgentTokenProfileStatus, OracleIndexPreferences,
    OracleLlmSettings, OracleLlmSettingsStatus, ProviderId, ProviderScopeStatus, SecretStatus,
};
use chrono::Utc;
use keyring::{Entry, Error as KeyringError};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const SERVICE: &str = "Aspis Management";

#[derive(Debug, Clone, Copy)]
pub struct CloudflareAgentTokenProfileSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub role: &'static str,
    pub env_var: &'static str,
    pub message: &'static str,
}

pub const CLOUDFLARE_AGENT_TOKEN_PROFILES: &[CloudflareAgentTokenProfileSpec] = &[
    CloudflareAgentTokenProfileSpec {
        id: "verifier-readonly",
        label: "Verifier read-only",
        role: "orchestrator/verifier",
        env_var: "ASPIS_CLOUDFLARE_VERIFIER_TOKEN",
        message: "Read-only Cloudflare token for orchestrator and verifier agents.",
    },
    CloudflareAgentTokenProfileSpec {
        id: "coder-worker-write",
        label: "Coder Worker write",
        role: "coder",
        env_var: "ASPIS_CLOUDFLARE_CODER_WORKER_WRITE_TOKEN",
        message: "Workers Scripts Write token for coder agents, without account-admin scope.",
    },
    CloudflareAgentTokenProfileSpec {
        id: "secrets-rotator",
        label: "Secrets rotator",
        role: "coder-secret-rotation",
        env_var: "ASPIS_CLOUDFLARE_SECRETS_ROTATOR_TOKEN",
        message: "Dedicated secret-rotation token, used only by guarded Cloudflare mutation tools.",
    },
];

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn entry(provider: ProviderId) -> Result<Entry, String> {
    Entry::new(SERVICE, provider.credential_account()).map_err(|_| vault_error("open"))
}

fn account_entry(account: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, account).map_err(|_| vault_error("open"))
}

fn cloudflare_agent_token_profile_account(profile_id: &str) -> Result<String, String> {
    let spec = cloudflare_agent_token_profile_spec(profile_id)?;
    Ok(format!("provider:cloudflare_agent_profile:{}", spec.id))
}

fn scope_entry(provider: ProviderId) -> Result<Entry, String> {
    Entry::new(SERVICE, provider.scope_credential_account()).map_err(|_| vault_error("open"))
}

fn scaleway_object_access_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "aux:scaleway_object_access_key").map_err(|_| vault_error("open"))
}

fn scaleway_object_secret_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "aux:scaleway_object_secret_key").map_err(|_| vault_error("open"))
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
fn cloud_llm_key_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, "provider:cloud_llm").map_err(|_| vault_error("open"))
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

pub fn save_token(provider: ProviderId, token: &str) -> Result<SecretStatus, String> {
    let cleaned = token.trim();
    if cleaned.len() < 16 {
        return Ok(SecretStatus {
            provider,
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Token is too short to save.".into()),
        });
    }

    entry(provider)?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    status(provider)
}

pub fn delete_token(provider: ProviderId) -> Result<SecretStatus, String> {
    match entry(provider)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    status(provider)
}

pub fn read_token(provider: ProviderId) -> Result<Option<String>, String> {
    match entry(provider)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

fn llm_provider_credential_account(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "scaleway" => Some("provider:scaleway_ai"),
        "infomaniak" => Some("provider:infomaniak"),
        "mistral" => Some("provider:mistral"),
        _ => None,
    }
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

pub fn status(provider: ProviderId) -> Result<SecretStatus, String> {
    match read_token(provider) {
        Ok(Some(_)) => Ok(SecretStatus {
            provider,
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Ok(None) => Ok(SecretStatus {
            provider,
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some(format!("{} token is not configured.", provider.label())),
        }),
        Err(e) => Ok(SecretStatus {
            provider,
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(e),
        }),
    }
}

pub fn all_statuses() -> Result<Vec<SecretStatus>, String> {
    Ok(vec![
        status(ProviderId::Cloudflare)?,
        status(ProviderId::Scaleway)?,
    ])
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

/// Present/absent status ONLY — never the value. Mirrors
/// `scaleway_object_access_key_status`.
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
            message: Some("Required for the local orchestrator's web-search egress.".into()),
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
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id: format!("{provider}_api_key"),
            label: format!("{label} web-search API key"),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(format!("{label} API key is too short or contains whitespace.")),
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

/// ROLE UNTANGLE (2026-07): collapse spawn-role aliases to the canonical roles so
/// the vault selection is defensive even if a caller forgets to normalize. Mirrors
/// `agent_role::canonicalize_launch_role` and ROLE_ALIASES in aspis_mcp.py.
/// "orchestrator" is FIRST-CLASS (it no longer folds to coder as an alias; it is
/// its own arm) — for TOKEN selection it deliberately holds the same write profile
/// as the coder (owner decision: the orchestrator is the frontier planning tier
/// that sees AND manages the project's providers; mutations stay task-audited
/// server-side). What it never holds is a file-write path — that's the mini/coder
/// executor's job, not a token concern.
fn canonical_agent_role(role: &str) -> &'static str {
    match role.trim().to_ascii_lowercase().as_str() {
        "verifier" => "verifier",
        "orchestrator" => "orchestrator",
        // coder + its legacy writer aliases (architect/code) -> coder.
        "coder" | "architect" | "code" => "coder",
        _ => "",
    }
}

pub fn cloudflare_agent_token_profile_id_for_role(role: &str) -> Option<&'static str> {
    // coder AND orchestrator -> the scoped write profile (the orchestrator manages
    // the infra it plans — owner decision, role untangle 2026-07); verifier ->
    // read-only. Explicit per-role arms, no alias fold hiding the decision.
    match canonical_agent_role(role) {
        "verifier" => Some("verifier-readonly"),
        "coder" | "orchestrator" => Some("coder-worker-write"),
        _ => None,
    }
}

pub fn cloudflare_agent_token_profile_spec(
    profile_id: &str,
) -> Result<CloudflareAgentTokenProfileSpec, String> {
    let id = profile_id.trim().to_ascii_lowercase();
    CLOUDFLARE_AGENT_TOKEN_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "Unknown Cloudflare agent token profile.".to_string())
}

pub fn read_cloudflare_agent_token_profile_token(
    profile_id: &str,
) -> Result<Option<String>, String> {
    let account = cloudflare_agent_token_profile_account(profile_id)?;
    match account_entry(&account)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

pub fn save_cloudflare_agent_token_profile(
    profile_id: &str,
    token: &str,
) -> Result<CloudflareAgentTokenProfileStatus, String> {
    let spec = cloudflare_agent_token_profile_spec(profile_id)?;
    let cleaned = token.trim();
    if cleaned.len() < 16 || cleaned.chars().any(char::is_whitespace) {
        return Ok(cloudflare_agent_token_profile_status_with(
            spec,
            false,
            "error",
            Some("Token is too short or contains whitespace."),
        ));
    }
    let account = cloudflare_agent_token_profile_account(profile_id)?;
    account_entry(&account)?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    cloudflare_agent_token_profile_status(profile_id)
}

pub fn delete_cloudflare_agent_token_profile(
    profile_id: &str,
) -> Result<CloudflareAgentTokenProfileStatus, String> {
    let account = cloudflare_agent_token_profile_account(profile_id)?;
    match account_entry(&account)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    cloudflare_agent_token_profile_status(profile_id)
}

pub fn cloudflare_agent_token_profile_status(
    profile_id: &str,
) -> Result<CloudflareAgentTokenProfileStatus, String> {
    let spec = cloudflare_agent_token_profile_spec(profile_id)?;
    match read_cloudflare_agent_token_profile_token(profile_id) {
        Ok(Some(_)) => Ok(cloudflare_agent_token_profile_status_with(
            spec,
            true,
            "configured",
            Some(spec.message),
        )),
        Ok(None) => Ok(cloudflare_agent_token_profile_status_with(
            spec,
            false,
            "missing",
            Some("Profile token is not configured."),
        )),
        Err(e) => Ok(cloudflare_agent_token_profile_status_with(
            spec,
            false,
            "error",
            Some(e.as_str()),
        )),
    }
}

pub fn all_cloudflare_agent_token_profile_statuses(
) -> Result<Vec<CloudflareAgentTokenProfileStatus>, String> {
    CLOUDFLARE_AGENT_TOKEN_PROFILES
        .iter()
        .map(|profile| cloudflare_agent_token_profile_status(profile.id))
        .collect()
}

/// D1: profile ids a given agent role is allowed to receive. Canonical role model
/// after the Phase B merge: verifier gets the read-only profile only; coder gets
/// its scoped write profile. The launch path normalizes "orchestrator" -> "coder"
/// before this call (and canonical_agent_role folds any stray alias defensively),
/// so the merged former-orchestrator legitimately receives the coder WRITE profile
/// BY DESIGN — it plans AND writes. The secrets-rotator profile is NEVER injected
/// into a launched agent through this path (reserved for guarded mutation tools).
pub fn cloudflare_agent_token_profile_ids_for_role(role: &str) -> &'static [&'static str] {
    match canonical_agent_role(role) {
        "verifier" => &["verifier-readonly"],
        // Orchestrator holds the same write profile as the coder (owner decision,
        // role untangle 2026-07 — it plans AND manages the infra).
        "coder" | "orchestrator" => &["coder-worker-write"],
        _ => &[],
    }
}

/// D1: role-filtered replacement for read_cloudflare_agent_token_profile_envs.
/// Only the profile(s) the role is allowed to hold are returned, so a coder no
/// longer receives the verifier token, and no role receives the rotator token.
pub fn read_cloudflare_agent_token_profile_envs_for_role(
    role: &str,
) -> Result<Vec<(String, String)>, String> {
    let allowed = cloudflare_agent_token_profile_ids_for_role(role);
    let mut envs = Vec::new();
    for profile in CLOUDFLARE_AGENT_TOKEN_PROFILES {
        if !allowed.contains(&profile.id) {
            continue;
        }
        if let Some(token) = read_cloudflare_agent_token_profile_token(profile.id)? {
            envs.push((profile.env_var.to_string(), token));
        }
    }
    Ok(envs)
}

fn cloudflare_agent_token_profile_status_with(
    spec: CloudflareAgentTokenProfileSpec,
    configured: bool,
    status: &str,
    message: Option<&str>,
) -> CloudflareAgentTokenProfileStatus {
    CloudflareAgentTokenProfileStatus {
        id: spec.id.into(),
        label: spec.label.into(),
        role: spec.role.into(),
        configured,
        status: status.into(),
        env_var: spec.env_var.into(),
        credential_account: cloudflare_agent_token_profile_account(spec.id).unwrap_or_default(),
        last_checked_at: Some(now()),
        message: message.map(String::from),
    }
}

/// Remote-first default: Oracle answers are API-only (remote providers).
///
/// The local Ollama + qwen3.5:4b chat path has been removed entirely; only the
/// remote providers (scaleway / infomaniak / mistral) are supported. Scaleway
/// is the default remote provider because the app already manages a Scaleway
/// token for the Cloud pages, so Oracle answering works out of the box for
/// anyone who has that token saved; users without it get a "missing_api_key"
/// nudge and extractive (retrieval-only) answers until they add a key.
/// NOTE: the local *embedder* (Qwen3-Embedding-0.6B) is unaffected and remains
/// mandatory for retrieval.
pub fn default_oracle_llm_settings() -> OracleLlmSettings {
    OracleLlmSettings {
        provider: "scaleway".into(),
        model: "voxtral-small-24b-2507".into(),
        base_url: None,
        remote_enabled: true,
    }
}

pub fn default_oracle_index_preferences() -> OracleIndexPreferences {
    OracleIndexPreferences {
        auto_watch_on_unlock: true,
        index_root: default_oracle_index_root().map(|path| path.to_string_lossy().to_string()),
        index_mode: None,
    }
}

pub fn save_oracle_index_preferences(
    preferences: &OracleIndexPreferences,
) -> Result<OracleIndexPreferences, String> {
    let cleaned = sanitize_oracle_index_preferences(preferences)?;
    let raw = serde_json::to_string(&cleaned)
        .map_err(|_| "Oracle index preferences could not be serialized.".to_string())?;
    oracle_index_preferences_entry()?
        .set_password(&raw)
        .map_err(|_| vault_error("save"))?;
    read_oracle_index_preferences()
}

/// TEST SEAM: unit tests must NEVER reach the OS keyring through this read. On a
/// dev machine the "Aspis Management" keychain item EXISTS but the per-build test
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

#[cfg(not(test))]
pub fn read_oracle_index_preferences() -> Result<OracleIndexPreferences, String> {
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

fn sanitize_oracle_index_preferences(
    preferences: &OracleIndexPreferences,
) -> Result<OracleIndexPreferences, String> {
    let default = default_oracle_index_preferences();
    let raw_root = preferences
        .index_root
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| default.index_root.as_deref().map(PathBuf::from));
    let index_root = match raw_root {
        Some(path) => {
            if !path.exists() || !path.is_dir() {
                return Err("Oracle index root must be an existing folder.".into());
            }
            Some(
                path.canonicalize()
                    .map_err(|_| "Oracle index root could not be resolved.".to_string())?
                    .to_string_lossy()
                    .to_string(),
            )
        }
        None => None,
    };
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
        "scaleway" => "Scaleway",
        "infomaniak" => "Infomaniak",
        "mistral" => "Mistral",
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
    let allowed = ["scaleway", "infomaniak", "mistral", "omlx", "ollama"];
    if !allowed.contains(&provider.as_str()) {
        return Err("Oracle LLM provider is not allowlisted.".into());
    }
    let model = settings.model.trim();
    if model.is_empty() || model.len() > 160 {
        return Err("Oracle LLM model is invalid.".into());
    }
    let remote_enabled = settings.remote_enabled;
    let base_url = sanitize_llm_base_url(&provider, settings.base_url.as_deref())?;
    if provider == "infomaniak" && remote_enabled && base_url.is_none() {
        return Err("Infomaniak Oracle LLM requires the product-specific HTTPS base URL.".into());
    }
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
            if let Some(host) = allowed_llm_host_prefix(provider) {
                let lower = value.to_ascii_lowercase();
                let host = host.to_ascii_lowercase();
                if lower != format!("https://{host}")
                    && !lower.starts_with(&format!("https://{host}/"))
                {
                    return Err(
                        "Oracle LLM base URL host does not match the selected provider."
                            .to_string(),
                    );
                }
            }
            Ok(value.to_string())
        })
        .transpose()
}

fn allowed_llm_host_prefix(provider: &str) -> Option<&'static str> {
    match provider {
        "scaleway" => Some("api.scaleway.ai"),
        "infomaniak" => Some("api.infomaniak.com"),
        "mistral" => Some("api.mistral.ai"),
        _ => None,
    }
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

pub fn save_scaleway_object_access_key(access_key: &str) -> Result<AuxCredentialStatus, String> {
    let cleaned = access_key.trim();
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id: "scaleway_object_access_key".into(),
            label: "Scaleway Object Storage access key".into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Access key is too short or contains whitespace.".into()),
        });
    }

    scaleway_object_access_key_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    scaleway_object_access_key_status()
}

pub fn delete_scaleway_object_access_key() -> Result<AuxCredentialStatus, String> {
    match scaleway_object_access_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    scaleway_object_access_key_status()
}

pub fn read_scaleway_object_access_key() -> Result<Option<String>, String> {
    match scaleway_object_access_key_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

pub fn scaleway_object_access_key_status() -> Result<AuxCredentialStatus, String> {
    match read_scaleway_object_access_key() {
        Ok(Some(_)) => Ok(AuxCredentialStatus {
            id: "scaleway_object_access_key".into(),
            label: "Scaleway Object Storage access key".into(),
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Ok(None) => Ok(AuxCredentialStatus {
            id: "scaleway_object_access_key".into(),
            label: "Scaleway Object Storage access key".into(),
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some("Required for live Object Storage bucket inventory.".into()),
        }),
        Err(e) => Ok(AuxCredentialStatus {
            id: "scaleway_object_access_key".into(),
            label: "Scaleway Object Storage access key".into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(e),
        }),
    }
}

pub fn save_scaleway_object_secret_key(secret_key: &str) -> Result<AuxCredentialStatus, String> {
    let cleaned = secret_key.trim();
    if cleaned.len() < 16 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            id: "scaleway_object_secret_key".into(),
            label: "Scaleway Object Storage secret key".into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some("Secret key is too short or contains whitespace.".into()),
        });
    }

    scaleway_object_secret_key_entry()?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    scaleway_object_secret_key_status()
}

pub fn delete_scaleway_object_secret_key() -> Result<AuxCredentialStatus, String> {
    match scaleway_object_secret_key_entry()?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    scaleway_object_secret_key_status()
}

pub fn read_scaleway_object_secret_key() -> Result<Option<String>, String> {
    match scaleway_object_secret_key_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

pub fn scaleway_object_secret_key_status() -> Result<AuxCredentialStatus, String> {
    match read_scaleway_object_secret_key() {
        Ok(Some(_)) => Ok(AuxCredentialStatus {
            id: "scaleway_object_secret_key".into(),
            label: "Scaleway Object Storage secret key".into(),
            configured: true,
            status: "configured".into(),
            last_checked_at: Some(now()),
            message: None,
        }),
        Ok(None) => Ok(AuxCredentialStatus {
            id: "scaleway_object_secret_key".into(),
            label: "Scaleway Object Storage secret key".into(),
            configured: false,
            status: "missing".into(),
            last_checked_at: Some(now()),
            message: Some(
                "Required with the access key for live Object Storage bucket inventory.".into(),
            ),
        }),
        Err(e) => Ok(AuxCredentialStatus {
            id: "scaleway_object_secret_key".into(),
            label: "Scaleway Object Storage secret key".into(),
            configured: false,
            status: "error".into(),
            last_checked_at: Some(now()),
            message: Some(e),
        }),
    }
}

pub fn save_scope(provider: ProviderId, pinned_id: &str) -> Result<ProviderScopeStatus, String> {
    let cleaned = pinned_id.trim();
    scope_entry(provider)?
        .set_password(cleaned)
        .map_err(|_| vault_error("save"))?;
    scope_status(provider)
}

pub fn delete_scope(provider: ProviderId) -> Result<ProviderScopeStatus, String> {
    match scope_entry(provider)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(_) => return Err(vault_error("delete")),
    }
    scope_status(provider)
}

pub fn read_scope(provider: ProviderId) -> Result<Option<String>, String> {
    match scope_entry(provider)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(_) => Err(vault_error("read")),
    }
}

pub fn scope_status(provider: ProviderId) -> Result<ProviderScopeStatus, String> {
    match read_scope(provider) {
        Ok(Some(value)) => Ok(ProviderScopeStatus {
            provider,
            configured: true,
            pinned_id: Some(value),
            label: provider_scope_label(provider).into(),
            message: None,
        }),
        Ok(None) => Ok(ProviderScopeStatus {
            provider,
            configured: false,
            pinned_id: None,
            label: provider_scope_label(provider).into(),
            message: Some(provider_scope_missing_message(provider).into()),
        }),
        Err(e) => Ok(ProviderScopeStatus {
            provider,
            configured: false,
            pinned_id: None,
            label: provider_scope_label(provider).into(),
            message: Some(e),
        }),
    }
}

pub fn all_scope_statuses() -> Result<Vec<ProviderScopeStatus>, String> {
    Ok(vec![
        scope_status(ProviderId::Cloudflare)?,
        scope_status(ProviderId::Scaleway)?,
    ])
}

fn provider_scope_label(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Cloudflare => "Cloudflare account id",
        ProviderId::Scaleway => "Scaleway project id",
    }
}

fn provider_scope_missing_message(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Cloudflare => {
            "Optional. Required only when the token can see multiple Cloudflare accounts."
        }
        ProviderId::Scaleway => {
            "Optional. Required only when multiple accessible projects normalize to aspis-bio."
        }
    }
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
        fn default_scaleway_settings() -> OracleLlmSettings {
            let mut settings = default_oracle_llm_settings();
            settings.provider = "scaleway".into();
            settings.base_url = Some("https://api.scaleway.ai/v1/chat/completions".into());
            settings
        }

        fn capture() -> Self {
            let probe = Self::default_scaleway_settings();
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
            let probe = Self::default_scaleway_settings();
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
        settings.base_url = Some("https://api.scaleway.ai/v1/chat/completions".into());

        let _ = delete_oracle_llm_api_key();
        let save_status =
            save_oracle_llm_settings(&settings, Some("dummy-scaleway-key-123456")).unwrap();
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
            Some("https://api.scaleway.ai/v1/chat/completions"),
            "base_url must survive persistence so the key-entry scope matches"
        );

        // Cleanup: drop the dummy key. The snapshot guard restores every slot.
        let _ = delete_oracle_llm_api_key();
    }

    #[test]
    fn cloudflare_agent_profile_ids_are_role_least_privilege() {
        // ROLE UNTANGLE (2026-07, owner decision): the orchestrator is FIRST-CLASS
        // and holds the SAME scoped write profile as the coder — it is the frontier
        // planning tier that sees and manages the project's providers (mutations
        // stay claimed-task + evidence audited server-side). Explicit arm, not an
        // alias fold.
        assert_eq!(
            cloudflare_agent_token_profile_ids_for_role("orchestrator"),
            &["coder-worker-write"]
        );
        // Verifier remains strictly read-only.
        assert_eq!(
            cloudflare_agent_token_profile_ids_for_role("verifier"),
            &["verifier-readonly"]
        );
        // Coder gets only its scoped write profile.
        assert_eq!(
            cloudflare_agent_token_profile_ids_for_role("coder"),
            &["coder-worker-write"]
        );
        // No role receives the secrets-rotator profile via this path.
        for role in ["orchestrator", "verifier", "coder", "unknown"] {
            assert!(
                !cloudflare_agent_token_profile_ids_for_role(role).contains(&"secrets-rotator"),
                "role {role} must never receive the secrets-rotator profile"
            );
        }
        // Unknown roles get nothing.
        assert!(cloudflare_agent_token_profile_ids_for_role("unknown").is_empty());
    }

    /// The dedicated-key scope is a STORAGE SLOT: it must be STABLE across
    /// base_url edits for the same provider (a user has one key per provider),
    /// and DIFFER across providers. Regression guard for the bug where the
    /// scope included base_url, moving the key to a different slot whenever the
    /// user edited their endpoint URL.
    #[test]
    fn oracle_llm_key_scope_is_stable_across_base_url_and_differs_by_provider() {
        let scaleway = OracleLlmSettings {
            provider: "scaleway".into(),
            model: "model-a".into(),
            base_url: Some("https://api.scaleway.ai/v1/chat/completions".into()),
            remote_enabled: true,
        };
        // Same provider, DIFFERENT base_url (e.g. a custom Scaleway deployment).
        let scaleway_custom_url = OracleLlmSettings {
            base_url: Some("https://api.scaleway.ai/v1/deployments/abc/chat/completions".into()),
            ..scaleway.clone()
        };
        // Same provider, NO base_url.
        let scaleway_no_url = OracleLlmSettings {
            base_url: None,
            ..scaleway.clone()
        };
        let infomaniak = OracleLlmSettings {
            provider: "infomaniak".into(),
            base_url: Some("https://api.infomaniak.com/2/ai/123/openai/v1/chat/completions".into()),
            ..scaleway.clone()
        };
        let mistral = OracleLlmSettings {
            provider: "mistral".into(),
            base_url: Some("https://api.mistral.ai/v1/chat/completions".into()),
            ..scaleway.clone()
        };

        // STABLE across base_url for the same provider.
        assert_eq!(
            oracle_llm_key_scope(&scaleway),
            oracle_llm_key_scope(&scaleway_custom_url),
            "scope must not change when only base_url changes"
        );
        assert_eq!(
            oracle_llm_key_scope(&scaleway),
            oracle_llm_key_scope(&scaleway_no_url),
            "scope must not change when base_url is dropped"
        );

        // DIFFERS across providers.
        assert_ne!(
            oracle_llm_key_scope(&scaleway),
            oracle_llm_key_scope(&infomaniak)
        );
        assert_ne!(
            oracle_llm_key_scope(&scaleway),
            oracle_llm_key_scope(&mistral)
        );

        // Provider normalization: case/whitespace must not split the slot.
        let scaleway_messy = OracleLlmSettings {
            provider: "  Scaleway  ".into(),
            ..scaleway.clone()
        };
        assert_eq!(
            oracle_llm_key_scope(&scaleway),
            oracle_llm_key_scope(&scaleway_messy),
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
        let mut scaleway = default_oracle_llm_settings();
        scaleway.provider = "scaleway".into();
        scaleway.base_url = Some("https://api.scaleway.ai/v1/chat/completions".into());

        let name = oracle_llm_api_key_entry_name(&scaleway);

        // The hash suffix (provider-only scope) keys the slot under `:primary:`.
        let scope = oracle_llm_key_scope(&scaleway);
        assert_eq!(name, format!("oracle:llm_api_key:primary:{scope}"));

        // Stable across base_url edits (provider-only scope).
        let scaleway_custom_url = OracleLlmSettings {
            base_url: Some("https://api.scaleway.ai/v1/deployments/abc/chat/completions".into()),
            ..scaleway.clone()
        };
        assert_eq!(
            name,
            oracle_llm_api_key_entry_name(&scaleway_custom_url),
            "the dedicated-key slot must be stable across base_url"
        );

        // The name can NEVER equal a LEGACY base_url-scoped name: the legacy name
        // is `oracle:llm_api_key:<hex>` (no role word), so the segment after the
        // second colon is hex, never "primary".
        let legacy_scope = legacy_oracle_llm_key_scope(&scaleway);
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
        save_settings.provider = "scaleway".into();
        save_settings.base_url = Some("https://api.scaleway.ai/v1/chat/completions".into());

        let _ = delete_oracle_llm_api_key();
        let status =
            save_oracle_llm_settings(&save_settings, Some("dummy-scaleway-key-123456")).unwrap();
        assert_eq!(status.status, "configured");

        // Same provider, DIFFERENT base_url — simulates the desync scenario.
        let mut read_settings = save_settings.clone();
        read_settings.base_url =
            Some("https://api.scaleway.ai/v1/deployments/xyz/chat/completions".into());

        let read_back = read_oracle_llm_api_key_for_settings(&read_settings).unwrap();
        assert_eq!(
            read_back.as_deref(),
            Some("dummy-scaleway-key-123456"),
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
        settings.provider = "scaleway".into();
        settings.base_url = Some("https://api.scaleway.ai/v1/chat/completions".into());

        // Start clean, then plant a key in the LEGACY base_url-scoped slot only.
        let _ = delete_oracle_llm_api_key();
        legacy_oracle_llm_api_key_entry_for_settings(&settings)
            .unwrap()
            .set_password("legacy-scaleway-key-123456")
            .unwrap();

        // The migration read-fallback must find it under the legacy slot.
        let read_back = read_oracle_llm_api_key_for_settings(&settings).unwrap();
        assert_eq!(read_back.as_deref(), Some("legacy-scaleway-key-123456"));

        // A save migrates to the provider-only slot and removes the legacy orphan.
        let status = save_oracle_llm_settings(&settings, Some("new-scaleway-key-7890ab")).unwrap();
        assert_eq!(status.status, "configured");
        assert_eq!(
            oracle_llm_api_key_entry(&settings)
                .unwrap()
                .get_password()
                .unwrap(),
            "new-scaleway-key-7890ab",
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
    fn llm_provider_tokens_include_infomaniak_provider_account() {
        assert_eq!(
            llm_provider_credential_account("scaleway"),
            Some("provider:scaleway_ai")
        );
        assert_eq!(
            llm_provider_credential_account("infomaniak"),
            Some("provider:infomaniak")
        );
        assert_eq!(
            llm_provider_credential_account("mistral"),
            Some("provider:mistral")
        );
        assert_eq!(llm_provider_credential_account("openrouter"), None);
    }

    #[test]
    fn infomaniak_requires_base_url_when_remote_enabled() {
        let settings = OracleLlmSettings {
            provider: "infomaniak".into(),
            model: "model-a".into(),
            base_url: None,
            remote_enabled: true,
        };

        assert!(sanitize_oracle_llm_settings(&settings).is_err());
    }

    #[test]
    fn removed_remote_llm_providers_are_rejected() {
        for provider in ["openrouter", "openai", "openai_compatible"] {
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
    fn llm_base_url_must_match_selected_provider() {
        let settings = OracleLlmSettings {
            provider: "scaleway".into(),
            model: "model-a".into(),
            base_url: Some("https://api.infomaniak.com/2/ai/123/openai/v1".into()),
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
    fn sanitize_accepts_scaleway_provider() {
        let settings = OracleLlmSettings {
            provider: "scaleway".into(),
            model: "voxtral-small-24b-2507".into(),
            base_url: Some("https://api.scaleway.ai/v1/chat/completions".into()),
            remote_enabled: true,
        };

        let out = sanitize_oracle_llm_settings(&settings).expect("scaleway must be accepted");
        assert_eq!(out.provider, "scaleway");
        assert!(out.remote_enabled);
    }

    // ── sanitize_oracle_index_preferences: index_mode coercion ────────────────

    fn prefs_with_mode(mode: Option<&str>) -> OracleIndexPreferences {
        OracleIndexPreferences {
            auto_watch_on_unlock: true,
            index_root: None,
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
