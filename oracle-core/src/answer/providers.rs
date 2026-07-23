//! LLM provider configuration, validation, and OpenAI-chat-compatible API call.
//!
//! Port of `oracle/server/answerer.py` provider layer — FAIL-CLOSED on
//! non-allowlisted providers, recoverable on missing credentials.

use std::collections::HashSet;
use std::env;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};

use crate::answer::AnswerError;

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

const DEFAULT_LLM_MODEL: &str = "gpt-4o-mini";
pub const LLM_TEMPERATURE: f64 = 0.1;
const DEFAULT_MAX_TOKENS: u32 = 1500;
pub const LOCAL_LLM_PROVIDERS: &[&str] = &["omlx", "ollama"];
const REMOTE_PROVIDERS: &[&str] = &["openai"];

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
            .unwrap_or_else(|_| "openai".to_string())
            .trim()
            .to_lowercase()
    } else {
        source.provider.trim().to_lowercase()
    };

    let allowed = all_allowed_providers();
    if !allowed.contains(provider.as_str()) {
        return Err(AnswerError::PrivacyGate(format!(
            "Oracle LLM provider {:?} is not allowlisted; \
             allowed: openai (OpenAI-compatible remote, keyed) and \
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
                .map(|n| n == 0 || n > 65535)
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
    // Reject any all-numeric host (dotted-decimal IPv4 AND partial shorthands like
    // `127.1` / `127.0.1`, which getaddrinfo expands to 127.0.0.1). A real FQDN always
    // has an alphabetic TLD, so an all-numeric-label host is never a legitimate name.
    let is_numeric_host = !labels.is_empty()
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    if is_numeric_host {
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

    // Post-DNS SSRF gate: resolve the host and reject private/loopback/
    // link-local/metadata ranges (lexical checks alone are rebinding-unsafe).
    let port_u16: u16 = port
        .and_then(|p| p.parse().ok())
        .unwrap_or(443);
    reject_blocked_resolved_ips(host, port_u16)?;

    Ok(())
}

/// Resolve `host:port` and reject any address in blocked ranges (fail-closed).
fn reject_blocked_resolved_ips(host: &str, port: u16) -> Result<(), AnswerError> {
    let addrs = match (host, port).to_socket_addrs() {
        Ok(iter) => iter.collect::<Vec<_>>(),
        Err(_) => {
            return Err(AnswerError::Validation(
                "Remote Oracle LLM base URL host could not be resolved.".to_string(),
            ));
        }
    };
    if addrs.is_empty() {
        return Err(AnswerError::Validation(
            "Remote Oracle LLM base URL host resolved to no addresses.".to_string(),
        ));
    }
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(AnswerError::PrivacyGate(
                "Remote Oracle LLM base URL must not resolve to a private, loopback, \
                 link-local, or metadata address."
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(v4: Ipv4Addr) -> bool {
    if v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_multicast()
    {
        return true;
    }
    let o = v4.octets();
    // CGNAT 100.64.0.0/10
    if o[0] == 100 && (o[1] & 0xc0) == 64 {
        return true;
    }
    // 0.0.0.0/8
    if o[0] == 0 {
        return true;
    }
    // IETF protocol assignments 192.0.0.0/24 (excl. documentation already covered)
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return true;
    }
    false
}

fn is_blocked_ipv6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
        return true;
    }
    let s = v6.segments();
    // Unique local fc00::/7
    if (s[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // Link-local fe80::/10
    if (s[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    // IPv4-mapped ::ffff:0:0/96 — re-check the embedded v4.
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0xffff {
        let v4 = Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        );
        return is_blocked_ipv4(v4);
    }
    // Deprecated IPv4-compatible ::a.b.c.d
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        if let Some(v4) = v6.to_ipv4() {
            return is_blocked_ipv4(v4);
        }
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// LLM call
// ═══════════════════════════════════════════════════════════════════════════

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

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    headers.insert(
        "HTTP-Referer",
        HeaderValue::from_static("https://aspis-bio.com"),
    );
    headers.insert("X-Title", HeaderValue::from_static("Devboule Oracle"));
    if !config.api_key.is_empty() {
        let auth = HeaderValue::try_from(format!("Bearer {}", config.api_key)).map_err(|_| {
            AnswerError::Validation(
                "Remote Oracle LLM API key contains invalid header characters.".to_string(),
            )
        })?;
        headers.insert("Authorization", auth);
    }

    let url = chat_completions_url(&config.base_url);

    // reqwest::blocking owns an internal tokio runtime; building/dropping it on a
    // tokio worker (the axum /ask handler calls this synchronously inside async)
    // panics with "Cannot drop a runtime ... from within an asynchronous context".
    // Run the whole blocking HTTP exchange on a dedicated OS thread so the blocking
    // client never touches the async runtime. Safe from a plain sync caller too.
    //
    // When a tokio runtime IS current (the async /ask handler), wrap the whole
    // scoped-thread join in `block_in_place` so tokio can spin a replacement worker
    // while we wait — a slow LLM (up to the 60 s client timeout) must not pin the
    // shared worker and starve other tasks.  When NO runtime is current (sync CLI
    // path) `block_in_place` would panic, so run the scoped-thread directly.
    let do_request = || -> Result<String, AnswerError> {
        std::thread::scope(|scope| {
            scope
                .spawn(move || -> Result<String, AnswerError> {
                    let client = Client::builder()
                        .timeout(Duration::from_secs(60))
                        .default_headers(headers)
                        .build()
                        .map_err(|e| {
                            AnswerError::Network(format!("Failed to create HTTP client: {}", e))
                        })?;

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
                        .map_err(|e| {
                            AnswerError::Network(format!("Failed to parse LLM response: {}", e))
                        })?;

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
                })
                .join()
                .map_err(|_| {
                    AnswerError::Network("LLM request worker thread panicked.".to_string())
                })?
        })
    };

    let content: Result<String, AnswerError> = match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| do_request()),
        Err(_) => do_request(),
    };

    content
}

fn truncate_err(s: &str) -> String {
    let cleaned: String = s.split_whitespace().collect::<Vec<&str>>().join(" ");
    if cleaned.len() > 220 {
        cleaned[..220].to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_ipv4_ranges() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 168, 1, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(169, 254, 169, 254)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(!is_blocked_ipv4(Ipv4Addr::new(8, 8, 8, 8)));
    }

    #[test]
    fn authorization_header_rejects_crlf_key() {
        // Production path: HeaderValue::try_from(...).map_err(|_| Validation).
        // CR/LF in the key must never panic via .parse().unwrap().
        assert!(
            HeaderValue::try_from("Bearer key\r\ninjected").is_err(),
            "CR/LF in Authorization value must be rejected"
        );
        assert!(HeaderValue::try_from("Bearer key\ninjected").is_err());
    }

    #[test]
    fn authorization_header_accepts_clean_key() {
        let ok = HeaderValue::try_from("Bearer sk-clean-key-value").expect("valid header");
        assert!(ok.to_str().unwrap().starts_with("Bearer "));
    }
}
