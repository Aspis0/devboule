//! Oracle answer synthesis — Rust port of `oracle/server/answerer.py`.
//!
//! Provides the `answer_from_context` entry point and the `LlmAnswerer`
//! implementation of `ContextAnswerer` from `crate::query::engine`.

pub mod context;
pub mod extractive;
pub mod guardrails;
pub mod prompt;
pub mod providers;

use std::env;

use anyhow::Result;

use crate::answer::context::{PreparedChunk, RawChunk};
use crate::answer::extractive::extractive_answer;
use crate::answer::guardrails::normalize_answer;
use crate::answer::prompt::build_answer_prompt;
use crate::answer::providers::{generate_with_openai_compatible, normalize_llm_config, LlmConfig};
use crate::query::engine::{AnswerPayload, ContextAnswerer, ContextChunk};

// ═══════════════════════════════════════════════════════════════════════════
// Shared types — single definition used by all sub-modules
// ═══════════════════════════════════════════════════════════════════════════

/// Error variants for the answer pipeline.
#[derive(Debug)]
pub enum AnswerError {
    /// Privacy/allowlist violation — FAIL-CLOSED, never degrade to extractive.
    PrivacyGate(String),
    /// Recoverable validation error (missing key/model) — degrade to extractive.
    Validation(String),
    /// LLM generation error — degrade to extractive.
    Generation(String),
    /// Network error — degrade to extractive.
    Network(String),
}

impl std::fmt::Display for AnswerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnswerError::PrivacyGate(s) => write!(f, "Privacy gate: {}", s),
            AnswerError::Validation(s) => write!(f, "Validation: {}", s),
            AnswerError::Generation(s) => write!(f, "Generation: {}", s),
            AnswerError::Network(s) => write!(f, "Network: {}", s),
        }
    }
}

impl std::error::Error for AnswerError {}

/// A citation reference within a prepared context.
#[derive(Debug, Clone)]
pub struct CitationRef {
    pub ref_id: String,
    pub file_source: String,
    pub chunk_id: String,
    pub chunk_index: Option<i64>,
    pub start_char: Option<i64>,
    pub end_char: Option<i64>,
    pub retrieval: String,
    pub score: f64,
}

/// Normalized answer result used by all sub-modules.
#[derive(Debug, Clone)]
pub struct NormalizedAnswer {
    pub answer: String,
    pub citations: Vec<CitationRef>,
    pub not_found: bool,
    pub suggested_path: Option<String>,
    pub answer_source: Option<String>,
    pub fallback_reason: Option<String>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Answer from context — main entry point mirroring `answerer.py::answer_from_context`.
pub fn answer_from_context(
    query: &str,
    chunks: &[ContextChunk],
    llm_config: Option<&LlmConfig>,
) -> Result<NormalizedAnswer, AnswerError> {
    let raw_chunks: Vec<RawChunk> = chunks.iter().map(context_chunk_to_raw).collect();
    let context = context::prepared_context(&raw_chunks, query);

    if context.is_empty() {
        return Ok(extractive::not_found_answer(query, &[], None));
    }

    if env::var("ORACLE_ASK_DISABLE_LLM")
        .unwrap_or_default()
        .trim()
        == "1"
    {
        return Ok(extractive_answer(
            query,
            &context,
            Some("LLM disabled for bounded smoke/test run"),
        ));
    }

    let prompt = build_answer_prompt(query, &context);
    let config = match normalize_llm_config(llm_config) {
        Ok(c) => c,
        // Python RE-RAISES OraclePrivacyGateError — the caller answers HTTP
        // 500, an answer dict is never fabricated. Mirror it: propagate.
        Err(e @ AnswerError::PrivacyGate(_)) => return Err(e),
        Err(_) => {
            return Ok(extractive_answer(
                query,
                &context,
                Some("LLM config normalization failed"),
            ));
        }
    };

    answer_with_llm_config(query, &prompt, &context, &config)
}

/// Attempt answer with a specific LLM config.
fn answer_with_llm_config(
    query: &str,
    prompt: &str,
    context: &[PreparedChunk],
    config: &LlmConfig,
) -> Result<NormalizedAnswer, AnswerError> {
    let is_local = providers::LOCAL_LLM_PROVIDERS.contains(&config.provider.as_str());
    let needs_key = !is_local;

    if (needs_key && config.api_key.is_empty()) || config.model.is_empty() {
        let reason = if needs_key && config.api_key.is_empty() {
            "Remote Oracle LLM API key is not configured."
        } else {
            "Oracle LLM model is not configured."
        };
        let mut answer = extractive_answer(query, context, Some(reason));
        answer.llm_provider = Some(config.provider.clone());
        answer.llm_model = Some(config.model.clone());
        return Ok(answer);
    }

    let raw_response = match generate_with_openai_compatible(prompt, config) {
        Ok(r) => r,
        // Python re-raises the privacy gate; everything else degrades.
        Err(e @ AnswerError::PrivacyGate(_)) => return Err(e),
        Err(e) => {
            let short = short_error_string(&e.to_string());
            let mut answer = extractive_answer(
                query,
                context,
                Some(&format!("LLM generation failed: {}", short)),
            );
            answer.llm_provider = Some(config.provider.clone());
            answer.llm_model = Some(config.model.clone());
            return Ok(answer);
        }
    };

    let parsed = guardrails::parse_json_response(&raw_response);
    let mut answer = normalize_answer(query, &parsed, context);
    answer.llm_provider = Some(config.provider.clone());
    answer.llm_model = Some(config.model.clone());
    Ok(answer)
}

/// Convert a ContextChunk to a RawChunk.
fn context_chunk_to_raw(c: &ContextChunk) -> RawChunk {
    RawChunk {
        chunk_id: c.chunk_id.clone(),
        file_source: c.file_source.clone(),
        chunk_index: Some(c.chunk_index as i64),
        start_char: Some(c.start_char as i64),
        end_char: Some(c.end_char as i64),
        text: c.text.clone(),
        score: c.score,
        retrieval: c.retrieval.clone(),
        kind: c.kind.clone(),
        symbol_name: c.symbol_name.clone(),
        signature: c.signature.clone(),
        language: c.language.clone(),
        line_start: Some(c.line_start as i64),
        line_end: Some(c.line_end as i64),
    }
}

/// Python `short_error`: whitespace-collapse, 220 CHARS, class-name fallback
/// for empty messages (Rust has no class name — "error" stands in).
fn short_error_string(s: &str) -> String {
    let cleaned: String = s.split_whitespace().collect::<Vec<&str>>().join(" ");
    let truncated: String = cleaned.chars().take(220).collect();
    if truncated.is_empty() {
        "error".to_string()
    } else {
        truncated
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// LlmAnswerer — ContextAnswerer trait implementation
// ═══════════════════════════════════════════════════════════════════════════

/// LLM-backed answerer implementing `ContextAnswerer`.
pub struct LlmAnswerer {
    config: Option<LlmConfig>,
}

impl LlmAnswerer {
    pub fn with_config(config: LlmConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    pub fn from_env() -> Self {
        Self { config: None }
    }
}

impl ContextAnswerer for LlmAnswerer {
    fn answer(&self, query: &str, context_chunks: &[ContextChunk]) -> Result<AnswerPayload> {
        // A privacy-gate violation propagates as Err (Python re-raises it and
        // the HTTP layer answers 500); every other failure degraded already.
        let result = answer_from_context(query, context_chunks, self.config.as_ref())?;

        Ok(AnswerPayload {
            answer: result.answer,
            citations: result
                .citations
                .into_iter()
                .map(|c| crate::query::engine::Citation {
                    ref_id: c.ref_id,
                    file_source: c.file_source,
                    chunk_id: c.chunk_id,
                    chunk_index: c.chunk_index,
                    start_char: c.start_char,
                    end_char: c.end_char,
                    retrieval: c.retrieval,
                    score: c.score,
                })
                .collect(),
            not_found: result.not_found,
            suggested_path: result.suggested_path,
            answer_source: result.answer_source,
            fallback_reason: result.fallback_reason,
            llm_provider: result.llm_provider,
            llm_model: result.llm_model,
        })
    }
}
