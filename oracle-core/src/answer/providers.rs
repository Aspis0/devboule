//! LLM provider configuration, validation, and OpenAI-chat-compatible API call.
//!
//! Port of `oracle/server/answerer.py` provider layer — FAIL-CLOSED on
//! non-allowlisted providers, recoverable on missing credentials.

use std::collections::HashSet;
use std::env;
use std::time::Duration;

use reqwest::blocking::Client;

use crate::answer::AnswerError;

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

const DEFAULT_LLM_MODEL: &str = "voxtral-small-24b-2507";
pub const LLM_TEMPERATURE: f64 = 0.1;
const DEFAULT_MAX_TOKENS: u32 = 1500;
pub const LOCAL_LLM_PROVIDERS: &[&str] = &["omlx", "ollama"];
const REMOTE_PROVIDERS: &[&str] = &["openai", "openrouter", "deepseek"];

// ═══════════════════════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone, Default)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
}

/// Custom Debug redacts `api_key` so it can never leak via `{:?}` into a log.
impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "[unset]"
                } else {
                    "[redacted]"
                },
            )
            .finish()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Provider allowlist helpers
// ═══════════════════════════════════════════════════════════════════════════

fn all_allowed_providers() -> HashSet<&'static str> {
    REMOTE_PROVIDERS
        .iter()
        .chain(LOCAL_LLM_PROVIDERS.iter())
        .copied()
        .collect()
}


// ═══════════════════════════════════════════════════════════════════════════
// Default base URLs (byte-exact from Python)
// ═══════════════════════════════════════════════════════════════════════════

pub fn default_base_url(provider: &str) -> String {
    match provider {
        "omlx" => "http://127.0.0.1:8000/v1/chat/completions".to_string(),
        "ollama" => "http://127.0.0.1:11434/v1/chat/completions".to_string(),
        "openai" => "https://api.openai.com/v1/chat/completions".to_string(),
        "openrouter" => "https://openrouter.ai/api/v1/chat/completions".to_string(),
        "deepseek" => "https://api.deepseek.com/v1/chat/completions".to_string(),
        _ => String::new(),
    }
}

pub fn chat_completions_url(base_url: &str) -> String {
    let url = base_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return url.to_string();
    }
    if url.ends_with("/chat/completions") {
        return url.to_string();
    }
    if url.ends_with("/v1") || url.ends_with("/openai/v1") {
        return format!("{}/chat/completions", url);
    }
    url.to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// Config normalization
// ═══════════════════════════════════════════════════════════════════════════

pub fn normalize_llm_config(config: Option<&LlmConfig>) -> Result<LlmConfig, AnswerError> {
    let source = config.cloned().unwrap_or_default();

    let provider = if source.provider.trim().is_empty() {
        env::var("ORACLE_LLM_PROVIDER")
            .unwrap_or_else(|_| "openrouter".to_string())
            .trim()
            .to_lowercase()
    } else {
        source.provider.trim().to_lowercase()
    };

    let allowed = all_allowed_providers();
    if !allowed.contains(provider.as_str()) {
        return Err(AnswerError::PrivacyGate(format!(
            "Oracle LLM provider {:?} is not allowlisted; \
             allowed: openai / openrouter / deepseek (remote, keyed) and \
             omlx / ollama (local, loopback-only).",
            provider
        )));
    }

    let model = if source.model.trim().is_empty() {
        env::var("ORACLE_LLM_MODEL")
            .unwrap_or_else(|_| DEFAULT_LLM_MODEL.to_string())
            .trim()
            .to_string()
    } else {
        source.model.trim().to_string()
    };

    let base_url = if source.base_url.trim().is_empty() {
        env::var("ORACLE_LLM_BASE_URL")
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        source.base_url.trim().to_string()
    };
    let base_url = if base_url.is_empty() {
        default_base_url(&provider)
    } else {
        base_url
    };

    let api_key = if source.api_key.trim().is_empty() {
        env::var("ORACLE_LLM_API_KEY")
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        source.api_key.trim().to_string()
    };

    Ok(LlmConfig {
        provider,
        model,
        base_url,
        api_key,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Validation
// ═══════════════════════════════════════════════════════════════════════════

pub fn enforce_remote_llm_provider_allowlist(provider: &str) -> Result<(), AnswerError> {
    let provider = provider.trim().to_lowercase();
    if !all_allowed_providers().contains(provider.as_str()) {
        return Err(AnswerError::PrivacyGate(
            "Oracle LLM provider is not allowlisted.".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_remote_llm_config(config: &LlmConfig) -> Result<(), AnswerError> {
    enforce_remote_llm_provider_allowlist(&config.provider)?;

    let provider = config.provider.trim().to_lowercase();

    if LOCAL_LLM_PROVIDERS.contains(&provider.as_str()) {
        if config.model.is_empty() {
            return Err(AnswerError::Validation(
                "Local Oracle LLM requires a model name.".to_string(),
            ));
        }
        let local_url = chat_completions_url(&config.base_url);
        let parsed = url::Url::parse(&local_url).map_err(|_| {
            AnswerError::Validation("Local Oracle LLM base URL is invalid.".to_string())
        })?;
        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(AnswerError::Validation(
                "Local Oracle LLM base URL is invalid.".to_string(),
            ));
        }
        let hostname = parsed.host_str().unwrap_or("");
        if !["127.0.0.1", "localhost", "::1"].contains(&hostname) {
            return Err(AnswerError::PrivacyGate(
                "Local Oracle LLM endpoints must stay on loopback (127.0.0.1).".to_string(),
            ));
        }
        return Ok(());
    }

    if config.api_key.is_empty() {
        return Err(AnswerError::Validation(
            "Remote Oracle LLM requires an API key saved in Devboule.".to_string(),
        ));
    }
    if config.model.is_empty() {
        return Err(AnswerError::Validation(
            "Remote Oracle LLM requires a model name.".to_string(),
        ));
    }
    let base_url = chat_completions_url(&config.base_url);
    let parsed = url::Url::parse(&base_url).map_err(|_| {
        AnswerError::Validation("Remote Oracle LLM base URL must be HTTPS.".to_string())
    })?;
    if parsed.scheme() != "https" {
        return Err(AnswerError::Validation(
            "Remote Oracle LLM base URL must be HTTPS.".to_string(),
        ));
    }
    validate_host_for_remote_llm(&base_url)
}

/// Generic SSRF-guarded host validator for remote OpenAI-compatible endpoints.
/// Mirrors the app's Censor Cloud validator (`validate_cloud_base_for_censor`).
/// Operates on a fully-formed `https://…` URL string.
fn validate_host_for_remote_llm(url: &str) -> Result<(), AnswerError> {
    let url_after_https = match url.strip_prefix("https://") {
        Some(rest) => rest,
        None => {
            return Err(AnswerError::Validation(
                "Remote Oracle LLM base URL must be HTTPS.".to_string(),
            ));
        }
    };

    let authority = url_after_https
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");

    if authority.is_empty() {
        return Err(AnswerError::Validation(
            "Remote Oracle LLM base URL is missing a host.".to_string(),
        ));
    }
    if authority.contains('@') {
        return Err(AnswerError::PrivacyGate(
            "Remote Oracle LLM base URL must not contain userinfo (\"@\")."
                .to_string(),
        ));
    }
    if authority.starts_with('[') {
        return Err(AnswerError::PrivacyGate(
            "Remote Oracle LLM base URL must not use an IPv6 literal.".to_string(),
        ));
    }

    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };

    if let Some(p) = port {
        if p.is_empty()
            || p.len() > 5
            || !p.bytes().all(|b| b.is_ascii_digit())
            || p.parse::<u32>()
                .map(|n| n > 65535)
                .unwrap_or(true)
        {
            return Err(AnswerError::Validation(
                "Remote Oracle LLM base URL has an invalid port.".to_string(),
            ));
        }
    }

    let host_lower = host.to_ascii_lowercase();

    if host_lower == "localhost" {
        return Err(AnswerError::PrivacyGate(
            "Remote Oracle LLM base URL must not point to localhost.".to_string(),
        ));
    }

    let labels: Vec<&str> = host.split('.').collect();
    let is_quad = labels.len() == 4
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    if is_quad {
        return Err(AnswerError::PrivacyGate(
            "Remote Oracle LLM base URL must not be a bare IPv4 address.".to_string(),
        ));
    }

    if host_lower == "metadata.google.internal"
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".local")
    {
        return Err(AnswerError::PrivacyGate(
            "Remote Oracle LLM base URL must not target intranet/metadata hosts."
                .to_string(),
        ));
    }

    if !host.contains('.') {
        return Err(AnswerError::PrivacyGate(
            "Remote Oracle LLM base URL host must be fully-qualified.".to_string(),
        ));
    }

    if !labels
        .iter()
        .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    {
        return Err(AnswerError::Validation(
            "Remote Oracle LLM base URL contains an invalid host label.".to_string(),
        ));
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// LLM call
// ═══════════════════════════════════════════════════════════════════════════

pub fn answer_json_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["answer", "citations", "not_found", "suggested_path"],
        "properties": {
            "answer": {"type": "string"},
            "citations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["ref"],
                    "properties": {"ref": {"type": "string"}}
                }
            },
            "not_found": {"type": "boolean"},
            "suggested_path": {"anyOf": [{"type": "string"}, {"type": "null"}]}
        }
    })
}

pub fn generate_with_openai_compatible(
    prompt: &str,
    config: &LlmConfig,
) -> Result<String, AnswerError> {
    validate_remote_llm_config(config)?;

    let max_tokens: u32 = env::var("ORACLE_ASK_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_TOKENS);

    let mut body = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": LLM_TEMPERATURE,
        "max_tokens": max_tokens,
    });

    body["response_format"] = serde_json::json!({"type": "json_object"});

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("HTTP-Referer", "https://aspis-bio.com".parse().unwrap());
    headers.insert("X-Title", "Devboule Oracle".parse().unwrap());
    if !config.api_key.is_empty() {
        headers.insert(
            "Authorization",
            format!("Bearer {}", config.api_key).parse().unwrap(),
        );
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .default_headers(headers)
        .build()
        .map_err(|e| AnswerError::Network(format!("Failed to create HTTP client: {}", e)))?;

    let url = chat_completions_url(&config.base_url);

    let response = client.post(&url).json(&body).send().map_err(|e| {
        AnswerError::Network(format!(
            "LLM request failed: {}",
            truncate_err(&e.to_string())
        ))
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().unwrap_or_default();
        return Err(AnswerError::Network(format!(
            "LLM request failed ({}): {}",
            status,
            truncate_err(&text)
        )));
    }

    let payload: serde_json::Value = response
        .json()
        .map_err(|e| AnswerError::Network(format!("Failed to parse LLM response: {}", e)))?;

    if let Some(content) = payload
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        return Ok(content.to_string());
    }
    if let Some(output) = payload.get("output_text").and_then(|o| o.as_str()) {
        return Ok(output.to_string());
    }

    Err(AnswerError::Generation(
        "Remote Oracle LLM response did not include chat content.".to_string(),
    ))
}

fn truncate_err(s: &str) -> String {
    let cleaned: String = s.split_whitespace().collect::<Vec<&str>>().join(" ");
    if cleaned.len() > 220 {
        cleaned[..220].to_string()
    } else {
        cleaned
    }
}
