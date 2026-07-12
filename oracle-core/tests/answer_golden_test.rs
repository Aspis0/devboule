//! Golden fixture tests for the Oracle answer pipeline.
//!
//! Tests `build_answer_prompt`, `redact_secret_tokens`, and guardrail/unit
//! behaviors against frozen Python output.  No live LLM calls.

use std::collections::HashMap;
use std::env;
use std::fs;

use oracle_core::answer::context::{
    prepared_context, redact_secret_tokens, PreparedChunk, RawChunk,
};
use oracle_core::answer::guardrails::{
    answer_has_non_english_markers, answer_has_unsupported_natural_claims,
};
use oracle_core::answer::prompt::build_answer_prompt;
use oracle_core::answer::providers::{
    chat_completions_url, default_base_url, enforce_remote_llm_provider_allowlist,
    normalize_llm_config, validate_remote_llm_config, LlmConfig,
};
use oracle_core::answer::{AnswerError, CitationRef};

// ═══════════════════════════════════════════════════════════════════════════
// Secret injection text (matches dump_golden.py exactly)
// ═══════════════════════════════════════════════════════════════════════════

const SECRET_INJECTION: &str = "\
\n# Secrets injection for redaction testing:\
\n# AWS: AKIAIOSFODNN7EXAMPLE key_id=AKIA1234567890ABCDEF\
\n# GitHub PAT: github_pat_11ABCDEF0123456789_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMN\
\n# GitHub token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef\
\n# Scaleway: SCWabcdefghijklmnopqrstuvwxyz12\
\n# Slack: xoxb-123456789012-1234567890123-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef\
\n# Bearer: Bearer eyJhbGciOiJIUzI1NiJ9.payload.signature\
\n# JWT: eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U\
\n# Generic: api_key=a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6\
\n# Base64 run: dGhpcyBpcyBhIHZlcnkgbG9uZyBhcml0cmFyeSBhc3NpZ25tZW50IHRoYXQgbmVlZHNSZWRhY3Rpb24K\
\n# Hex run: 0123456789abcdef0123456789abcdef01234567\
\n";

// ═══════════════════════════════════════════════════════════════════════════
// Fixture loading helpers
// ═══════════════════════════════════════════════════════════════════════════

fn fixtures_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("golden")
        .join("fixtures")
}

#[derive(serde::Deserialize)]
struct ChunkDict {
    id: String,
    #[serde(default)]
    file_sorgente: String,
    #[serde(default)]
    file_id: String,
    #[serde(default)]
    chunk_index: i64,
    #[serde(default)]
    start_char: i64,
    #[serde(default)]
    end_char: i64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    symbol_name: String,
    #[serde(default)]
    signature: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    line_start: i64,
    #[serde(default)]
    line_end: i64,
}

#[derive(serde::Deserialize)]
struct LexicalQueryResult {
    #[serde(default)]
    chunk_scores: HashMap<String, f64>,
}

#[derive(serde::Deserialize)]
struct AnswerPromptFixture {
    query: String,
    context_chunk_ids: Vec<String>,
    prompt: String,
    redaction_test: Option<Vec<RedactionTestEntry>>,
}

#[derive(serde::Deserialize)]
struct RedactionTestEntry {
    #[serde(rename = "ref")]
    _ref: Option<String>,
    #[allow(dead_code)]
    chunk_id: String,
    #[allow(dead_code)]
    file_source: String,
    text_original: String,
    text_redacted: String,
}

fn load_chunks() -> HashMap<String, Vec<ChunkDict>> {
    let path = fixtures_dir().join("chunks.json");
    let data = fs::read_to_string(&path).expect("Failed to read chunks.json");
    serde_json::from_str(&data).expect("Failed to parse chunks.json")
}

fn load_lexical() -> HashMap<String, LexicalQueryResult> {
    let path = fixtures_dir().join("lexical.json");
    let data = fs::read_to_string(&path).expect("Failed to read lexical.json");
    serde_json::from_str(&data).expect("Failed to parse lexical.json")
}

fn load_answer_fixtures() -> Vec<AnswerPromptFixture> {
    let path = fixtures_dir().join("answer_prompt.json");
    let data = fs::read_to_string(&path).expect("Failed to read answer_prompt.json");
    serde_json::from_str(&data).expect("Failed to parse answer_prompt.json")
}

fn flatten_chunks(chunks: &HashMap<String, Vec<ChunkDict>>) -> HashMap<String, &ChunkDict> {
    let mut flat = HashMap::new();
    for file_chunks in chunks.values() {
        for chunk in file_chunks {
            flat.insert(chunk.id.clone(), chunk);
        }
    }
    flat
}

fn chunk_dict_to_raw(cd: &ChunkDict, score: f64) -> RawChunk {
    RawChunk {
        chunk_id: cd.id.clone(),
        file_source: if cd.file_sorgente.is_empty() {
            cd.file_id.clone()
        } else {
            cd.file_sorgente.clone()
        },
        chunk_index: Some(cd.chunk_index),
        start_char: Some(cd.start_char),
        end_char: Some(cd.end_char),
        text: cd.text.clone(),
        score,
        retrieval: "lexical".to_string(),
        kind: cd.kind.clone(),
        symbol_name: cd.symbol_name.clone(),
        signature: cd.signature.clone(),
        language: cd.language.clone(),
        line_start: Some(cd.line_start),
        line_end: Some(cd.line_end),
    }
}

/// Find first divergence between two strings for debugging.
#[allow(dead_code)]
fn first_divergence(a: &str, b: &str) -> String {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    for (i, (ac, bc)) in a_chars.iter().zip(b_chars.iter()).enumerate() {
        if ac != bc {
            let start = i.saturating_sub(30);
            let end_a = (i + 30).min(a_chars.len());
            let end_b = (i + 30).min(b_chars.len());
            let context_a: String = a_chars[start..end_a].iter().collect();
            let context_b: String = b_chars[start..end_b].iter().collect();
            return format!(
                "First divergence at char index {}: expected {:?}, got {:?}\n  Context expected: {:?}\n  Context got:      {:?}",
                i, ac, bc, context_a, context_b
            );
        }
    }
    if a_chars.len() != b_chars.len() {
        format!(
            "Lengths differ: expected {} chars, got {} chars",
            a_chars.len(),
            b_chars.len()
        )
    } else {
        "Strings are identical".to_string()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Golden test: build_answer_prompt byte-equality
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn golden_answer_prompt_byte_equal() {
    env::set_var("ORACLE_ASK_MAX_CHARS_PER_CHUNK", "100000");

    let all_chunks = load_chunks();
    let flat = flatten_chunks(&all_chunks);
    let lexical = load_lexical();
    let fixtures = load_answer_fixtures();

    for (fixture_idx, fixture) in fixtures.iter().enumerate() {
        let query = &fixture.query;
        let chunk_ids = &fixture.context_chunk_ids;

        let context_for_prepared: Vec<RawChunk> = chunk_ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| flat.get(id.as_str()).map(|cd| (i, cd)))
            .map(|(i, cd)| {
                let score = lexical
                    .get(query)
                    .and_then(|lr| lr.chunk_scores.get(&cd.id))
                    .copied()
                    .unwrap_or(0.0);
                let mut raw = chunk_dict_to_raw(cd, score);
                // Inject fake secrets into first chunk of first query (matches dump_golden.py).
                if i == 0 && fixture_idx == 0 {
                    raw.text.push_str(SECRET_INJECTION);
                }
                raw
            })
            .collect();

        let prepared = prepared_context(&context_for_prepared, query);
        let prompt = build_answer_prompt(query, &prepared);

        if prompt != fixture.prompt {
            let diag = first_divergence(&prompt, &fixture.prompt);
            panic!("Prompt byte-equality failed for query: {}\n{}", query, diag);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Golden test: redact_secret_tokens byte-equality
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn golden_redaction_byte_equal() {
    let fixtures = load_answer_fixtures();
    for fixture in &fixtures {
        if let Some(ref entries) = fixture.redaction_test {
            for entry in entries {
                let redacted = redact_secret_tokens(&entry.text_original);
                assert_eq!(
                    redacted, entry.text_redacted,
                    "Redaction mismatch for chunk {} (query: {})",
                    entry.chunk_id, fixture.query
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Unit tests: guardrails
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_non_english_rejection_italian() {
    assert!(answer_has_non_english_markers(
        "La risposta non trovato nel corpus è corretta."
    ));
}

#[test]
fn test_non_english_rejection_spanish() {
    assert!(answer_has_non_english_markers(
        "El código del agentes es correcto."
    ));
}

#[test]
fn test_non_english_rejection_french() {
    assert!(answer_has_non_english_markers(
        "Le code des agents est correct."
    ));
}

#[test]
fn test_non_english_accept_english() {
    assert!(!answer_has_non_english_markers(
        "The Scaleway instance lifecycle manages compute resources."
    ));
}

#[test]
fn test_unsupported_claims_high_risk() {
    let context = vec![PreparedChunk {
        r#ref: "C1".to_string(),
        chunk_id: "test".to_string(),
        file_source: "test.rs".to_string(),
        chunk_index: Some(0),
        start_char: Some(0),
        end_char: Some(100),
        retrieval: "lexical".to_string(),
        score: 1.0,
        text: "The function always deletes the instance.".to_string(),
        kind: "function".to_string(),
        symbol_name: "cleanup".to_string(),
        signature: String::new(),
        language: "rust".to_string(),
        line_start: 0,
        line_end: 10,
    }];
    let citations = vec![CitationRef {
        ref_id: "C1".to_string(),
        file_source: "test.rs".to_string(),
        chunk_id: "test".to_string(),
        chunk_index: Some(0),
        start_char: Some(0),
        end_char: Some(100),
        retrieval: "lexical".to_string(),
        score: 1.0,
    }];
    assert!(answer_has_unsupported_natural_claims(
        "The function always deletes the instance automatically.",
        &citations,
        &context,
    ));
}

// ═══════════════════════════════════════════════════════════════════════════
// Unit tests: provider allowlist
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_unknown_provider_fail_closed() {
    let result = normalize_llm_config(Some(&LlmConfig {
        provider: "anthropic".to_string(),
        model: "test".to_string(),
        base_url: String::new(),
        api_key: String::new(),
    }));
    assert!(result.is_err());
    match result.unwrap_err() {
        AnswerError::PrivacyGate(_) => {}
        other => panic!("Expected PrivacyGate, got {:?}", other),
    }
}

#[test]
fn test_missing_key_degrades_to_extractive() {
    let result = normalize_llm_config(Some(&LlmConfig {
        provider: "openai".to_string(),
        model: "test".to_string(),
        base_url: String::new(),
        api_key: String::new(),
    }));
    assert!(result.is_ok());
    let config = result.unwrap();
    assert!(config.api_key.is_empty());
}

#[test]
fn test_loopback_validation_rejects_non_loopback() {
    let result = validate_remote_llm_config(&LlmConfig {
        provider: "ollama".to_string(),
        model: "test".to_string(),
        base_url: "http://10.0.0.1:11434/v1/chat/completions".to_string(),
        api_key: String::new(),
    });
    assert!(result.is_err());
    match result.unwrap_err() {
        AnswerError::PrivacyGate(_) => {}
        other => panic!("Expected PrivacyGate for non-loopback, got {:?}", other),
    }
}

#[test]
fn test_loopback_validation_accepts_localhost() {
    let result = validate_remote_llm_config(&LlmConfig {
        provider: "ollama".to_string(),
        model: "test".to_string(),
        base_url: "http://localhost:11434/v1/chat/completions".to_string(),
        api_key: String::new(),
    });
    assert!(result.is_ok());
}

#[test]
fn test_loopback_validation_accepts_127_0_0_1() {
    let result = validate_remote_llm_config(&LlmConfig {
        provider: "omlx".to_string(),
        model: "test".to_string(),
        base_url: "http://127.0.0.1:8000/v1/chat/completions".to_string(),
        api_key: String::new(),
    });
    assert!(result.is_ok());
}

#[test]
fn test_enforce_allowlist_unknown_provider() {
    let result = enforce_remote_llm_provider_allowlist("anthropic");
    assert!(result.is_err());
}

#[test]
fn test_enforce_allowlist_known_provider() {
    for provider in &["openai", "omlx", "ollama"] {
        let result = enforce_remote_llm_provider_allowlist(provider);
        assert!(result.is_ok(), "Provider {} should be allowed", provider);
    }
}

#[test]
fn test_default_base_urls() {
    assert_eq!(
        default_base_url("openai"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        default_base_url("omlx"),
        "http://127.0.0.1:8000/v1/chat/completions"
    );
    assert_eq!(
        default_base_url("ollama"),
        "http://127.0.0.1:11434/v1/chat/completions"
    );
}

#[test]
fn test_chat_completions_url() {
    assert_eq!(
        chat_completions_url("https://api.openai.com/v1"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_url("https://api.openai.com/v1/chat/completions"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_url("https://openrouter.ai/api/v1"),
        "https://openrouter.ai/api/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_url("https://api.deepseek.com/v1/"),
        "https://api.deepseek.com/v1/chat/completions"
    );
}

// ── P5-review regression tests ──────────────────────────────────────────────

fn cfg(provider: &str, base_url: &str) -> oracle_core::answer::providers::LlmConfig {
    oracle_core::answer::providers::LlmConfig {
        provider: provider.to_string(),
        model: "test-model".to_string(),
        base_url: base_url.to_string(),
        api_key: "test-key".to_string(),
    }
}

/// Generic SSRF-guarded host validation: provider names are labels only;
/// base_url may target ANY public https OpenAI-compatible endpoint.
#[test]
fn generic_host_guard_remote_providers() {
    use oracle_core::answer::providers::validate_remote_llm_config;

    // openai → own host: OK
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://api.openai.com/v1/chat/completions"
    ))
    .is_ok());

    // openai → openrouter host: also OK (no per-provider pinning).
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://openrouter.ai/api/v1/chat/completions"
    ))
    .is_ok());

    // openai → deepseek host: also OK.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://api.deepseek.com/v1/chat/completions"
    ))
    .is_ok());

    // Valid explicit port :443 is now ALLOWED.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://api.openai.com:443/v1"
    ))
    .is_ok());

    // Case-insensitive host match.
    assert!(
        validate_remote_llm_config(&cfg("openai", "https://Api.OPENAI.COM/v1")).is_ok()
    );

    // SSRF: bare IPv4 literal.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://169.254.169.254/v1/chat/completions"
    ))
    .is_err());

    // SSRF: partial IPv4 shorthands (getaddrinfo expands 127.1 → 127.0.0.1).
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://127.1/v1/chat/completions"
    ))
    .is_err());
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://127.0.1/v1"
    ))
    .is_err());

    // SSRF: all-numeric-label host (2 labels, not just 4).
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://10.1/v1"
    ))
    .is_err());

    // Numeric subdomain FQDN must still be accepted (alphabetic TLD present).
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://192.host.deepseek.com/v1/chat/completions"
    ))
    .is_ok());

    // SSRF: localhost.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://localhost/v1/chat/completions"
    ))
    .is_err());

    // SSRF: single-label host (no dot).
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://internalhost/v1/chat/completions"
    ))
    .is_err());

    // SSRF: .internal host.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://myhost.internal/v1/chat/completions"
    ))
    .is_err());

    // SSRF: .local host.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://printer.local/v1/chat/completions"
    ))
    .is_err());

    // Malformed: invalid port.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://api.openai.com:99999/v1"
    ))
    .is_err());

    // Under the GENERIC guard (owner's "open api" choice) there is NO per-provider
    // host pinning: any well-formed public https FQDN is accepted, so a host like
    // `api.openai.com.evil.com` is a legitimate (if unusual) user-chosen endpoint,
    // not a rejected "subdomain trick". The guard only blocks loopback / IP-literals
    // / intranet-metadata, which the assertions above cover.
    assert!(validate_remote_llm_config(&cfg(
        "openai",
        "https://api.openai.com.evil.com/v1"
    ))
    .is_ok());
}

/// focused_excerpt behavior (review F14): untested by the golden prompts
/// because the fixtures disable it via a huge limit.
#[test]
fn focused_excerpt_behavior() {
    use oracle_core::answer::context::focused_excerpt;

    // Short text passes through untouched.
    let short = "fn spawn_gpu() {}";
    assert_eq!(focused_excerpt(short, "gpu spawn", 2800), short);

    // Long text with a late match: the excerpt window centers on matched
    // terms and carries the mid-chunk markers.
    let filler = "x".repeat(4000);
    let long = format!("{filler}\nThe scaleway gpu spawn limit lives here.\n{filler}");
    let out = focused_excerpt(&long, "scaleway gpu spawn limit", 400);
    assert!(
        out.contains("scaleway gpu spawn limit"),
        "window must cover the match"
    );
    assert!(
        out.contains("[excerpt starts mid-chunk]"),
        "leading marker expected"
    );
    assert!(
        out.contains("[excerpt ends mid-chunk]"),
        "trailing marker expected"
    );
    assert!(
        out.chars().count() < long.chars().count(),
        "must actually truncate"
    );

    // No term matches: Python falls back to truncate_text (head + marker).
    let no_match = focused_excerpt(&long, "zzz qqq", 400);
    assert!(no_match.contains("[truncated]"));
    assert!(!no_match.contains("[excerpt starts mid-chunk]"));

    // Determinism: identical inputs give identical output (unlike Python,
    // whose per-term 40-position cap depends on hash-seeded term order).
    assert_eq!(
        focused_excerpt(&long, "scaleway gpu spawn limit", 400),
        focused_excerpt(&long, "scaleway gpu spawn limit", 400)
    );
}
