//! Answer normalization and guardrails.
//!
//! Port of `answerer.py` guardrail checks: non-English rejection, too-generic
//! detection, unsupported-claims/grounding-term checks, citation validation.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::answer::context::{
    max_answer_chars, truncate_text, NOT_FOUND_PHRASE,
};
use crate::answer::extractive::{extractive_answer, domain_extractive_answer};
use crate::answer::PreparedChunk;

// ═══════════════════════════════════════════════════════════════════════════
// Constants (byte-exact from Python)
// ═══════════════════════════════════════════════════════════════════════════

/// Non-English phrase markers.
const NON_ENGLISH_PHRASES: &[&str] = &[
    " non trovato nel corpus",
    " la risposta ",
    " les agents ",
    " los agentes ",
    " el codigo ",
    " el código ",
    " le code ",
    " e' ",
    " è ",
    " puede ",
    " pourrait ",
];

/// Per-language marker word sets (byte-exact from Python).
/// Index 0 = Italian, 1 = Spanish, 2 = French.
const NON_ENGLISH_MARKER_SETS: &[&[&str]] = &[
    &[
        "risposta",
        "forniti",
        "fornito",
        "codice",
        "agenti",
        "questo",
        "questa",
        "usando",
        "evita",
        "limita",
        "sono",
        "perche",
        "perché",
    ],
    &[
        "respuesta",
        "codigo",
        "código",
        "archivo",
        "agentes",
        "tarea",
        "estado",
        "usa",
        "usan",
        "desde",
        "porque",
        "sin",
    ],
    &[
        "réponse",
        "reponse",
        "fichier",
        "agents",
        "tâche",
        "tache",
        "état",
        "etat",
        "utilise",
        "depuis",
        "parce",
        "sans",
    ],
];

/// High-risk claim terms (byte-exact from Python).
const HIGH_RISK_CLAIM_TERMS: &[&str] = &[
    "all",
    "always",
    "automatically",
    "bypass",
    "bypasses",
    "bypassed",
    "delete",
    "deletes",
    "free",
    "never",
    "no",
    "paid",
    "skip",
    "skips",
    "terminate",
    "terminates",
    "without",
];

/// Claim stopwords (byte-exact from Python).
const CLAIM_STOPWORDS: &[&str] = &[
    "about",
    "after",
    "also",
    "and",
    "are",
    "before",
    "both",
    "but",
    "can",
    "does",
    "for",
    "from",
    "into",
    "that",
    "the",
    "then",
    "they",
    "this",
    "through",
    "when",
    "where",
    "which",
    "with",
];

/// Common grounded terms that are NOT flagged as unsupported.
const COMMON_GROUNDED_TERMS: &[&str] = &[
    "api", "app", "cpu", "gpu", "http", "https", "json", "llm", "mcp", "oracle", "ui", "url",
    "vm",
];

// ═══════════════════════════════════════════════════════════════════════════
// normalize_answer — main guardrail entry point
// ═══════════════════════════════════════════════════════════════════════════

/// The parsed LLM JSON response shape.
#[derive(Debug, Default, Clone)]
pub struct ParsedAnswer {
    pub answer: Option<String>,
    pub citations: Option<Vec<serde_json::Value>>,
    pub not_found: Option<bool>,
    pub suggested_path: Option<String>,
}

/// Normalize and validate a parsed LLM answer.
///
/// Mirrors `answerer.py::normalize_answer`.  Returns an `AnswerPayload`-shaped
/// dict (as a serde_json::Value for flexibility).
pub fn normalize_answer(
    query: &str,
    parsed: &ParsedAnswer,
    context: &[PreparedChunk],
) -> NormalizedAnswer {
    let answer_text = clean_answer(parsed.answer.as_deref().unwrap_or(""));
    let parsed_not_found = parsed.not_found.unwrap_or(false);
    let not_found_phrase_in_answer = answer_text.to_lowercase().contains(NOT_FOUND_PHRASE);
    let not_found = parsed_not_found || not_found_phrase_in_answer;

    if answer_text.is_empty() {
        let ea = extractive_answer(query, context, Some("LLM returned empty or invalid JSON"));
        return NormalizedAnswer {
            answer: ea.answer,
            citations: ea.citations,
            not_found: ea.not_found,
            suggested_path: ea.suggested_path,
            answer_source: ea.answer_source,
            fallback_reason: ea.fallback_reason,
        };
    }

    if not_found {
        let grounded = domain_extractive_answer(
            query,
            context,
            Some("LLM returned not_found despite matching code evidence"),
        );
        if let Some(grounded) = grounded {
            return grounded;
        }
        let suggested = suggest_path(query, context);
        return NormalizedAnswer {
            answer: ensure_not_found_prefix(&answer_text),
            citations: vec![],
            not_found: true,
            suggested_path: suggested,
            answer_source: Some("not_found".to_string()),
            fallback_reason: None,
        };
    }

    let citations = normalize_citations(parsed.citations.as_deref().unwrap_or(&[]), context);
    if citations.is_empty() {
        let ea = extractive_answer(query, context, Some("LLM returned no valid citations"));
        return NormalizedAnswer {
            answer: ea.answer,
            citations: ea.citations,
            not_found: ea.not_found,
            suggested_path: ea.suggested_path,
            answer_source: ea.answer_source,
            fallback_reason: ea.fallback_reason,
        };
    }
    if answer_is_too_generic(query, &answer_text, context) {
        let ea = extractive_answer(query, context, Some("LLM returned a generic answer"));
        return NormalizedAnswer {
            answer: ea.answer,
            citations: ea.citations,
            not_found: ea.not_found,
            suggested_path: ea.suggested_path,
            answer_source: ea.answer_source,
            fallback_reason: ea.fallback_reason,
        };
    }
    if answer_has_non_english_markers(&answer_text) {
        let ea = extractive_answer(query, context, Some("LLM returned a non-English answer"));
        return NormalizedAnswer {
            answer: ea.answer,
            citations: ea.citations,
            not_found: ea.not_found,
            suggested_path: ea.suggested_path,
            answer_source: ea.answer_source,
            fallback_reason: ea.fallback_reason,
        };
    }
    if answer_has_unsupported_natural_claims(&answer_text, &citations, context) {
        let ea = extractive_answer(
            query,
            context,
            Some("LLM answer included unsupported natural-language claims"),
        );
        return NormalizedAnswer {
            answer: ea.answer,
            citations: ea.citations,
            not_found: ea.not_found,
            suggested_path: ea.suggested_path,
            answer_source: ea.answer_source,
            fallback_reason: ea.fallback_reason,
        };
    }
    if answer_has_unsupported_grounding_terms(&answer_text, &citations, context) {
        let ea = extractive_answer(
            query,
            context,
            Some("LLM answer included unsupported identifiers or paths"),
        );
        return NormalizedAnswer {
            answer: ea.answer,
            citations: ea.citations,
            not_found: ea.not_found,
            suggested_path: ea.suggested_path,
            answer_source: ea.answer_source,
            fallback_reason: ea.fallback_reason,
        };
    }

    NormalizedAnswer {
        answer: truncate_text(&answer_text, max_answer_chars()),
        citations,
        not_found: false,
        suggested_path: None,
        answer_source: Some("llm".to_string()),
        fallback_reason: None,
    }
}

/// Normalized answer result.
#[derive(Debug, Clone)]
pub struct NormalizedAnswer {
    pub answer: String,
    pub citations: Vec<CitationRef>,
    pub not_found: bool,
    pub suggested_path: Option<String>,
    pub answer_source: Option<String>,
    pub fallback_reason: Option<String>,
}

/// A citation reference.
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

// ═══════════════════════════════════════════════════════════════════════════
// Citation normalization
// ═══════════════════════════════════════════════════════════════════════════

/// Normalize raw LLM citations against prepared context.
///
/// Mirrors `answerer.py::normalize_citations`.
pub fn normalize_citations(
    raw_citations: &[serde_json::Value],
    context: &[PreparedChunk],
) -> Vec<CitationRef> {
    let by_ref: HashSet<&str> = context.iter().map(|c| c.r#ref.as_str()).collect();
    let by_chunk_id: HashSet<&str> = context
        .iter()
        .filter(|c| !c.chunk_id.is_empty())
        .map(|c| c.chunk_id.as_str())
        .collect();

    let mut citations = Vec::new();
    let mut seen = HashSet::new();

    for raw in raw_citations {
        let ref_id = extract_ref_from_json(raw, &by_chunk_id, context);
        let ref_id = match ref_id {
            Some(r) => r,
            None => continue,
        };
        if !by_ref.contains(ref_id.as_str()) {
            continue;
        }
        // Find the matching prepared chunk.
        let item = context.iter().find(|c| c.r#ref == ref_id);
        let item = match item {
            Some(i) => i,
            None => continue,
        };
        if seen.contains(&item.chunk_id) {
            continue;
        }
        seen.insert(item.chunk_id.clone());
        citations.push(CitationRef {
            ref_id: item.r#ref.clone(),
            file_source: item.file_source.clone(),
            chunk_id: item.chunk_id.clone(),
            chunk_index: item.chunk_index,
            start_char: item.start_char,
            end_char: item.end_char,
            retrieval: item.retrieval.clone(),
            score: item.score,
        });
    }
    citations
}

fn extract_ref_from_json(
    raw: &serde_json::Value,
    by_chunk_id: &HashSet<&str>,
    context: &[PreparedChunk],
) -> Option<String> {
    match raw {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => {
            let ref_val = map
                .get("ref")
                .or_else(|| map.get("source_ref"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(ref r) = ref_val {
                if by_chunk_id.contains(r.as_str()) {
                    // Try to resolve via chunk_id.
                    if let Some(item) = context.iter().find(|c| c.chunk_id == *r) {
                        return Some(item.r#ref.clone());
                    }
                }
                return Some(r.clone());
            }
            None
        }
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Guardrail checks
// ═══════════════════════════════════════════════════════════════════════════

/// Check if answer is too generic — mirrors `answerer.py::answer_is_too_generic`.
pub fn answer_is_too_generic(query: &str, answer: &str, context: &[PreparedChunk]) -> bool {
    let lower = answer.to_lowercase();
    let meta_prefixes = [
        "based on the provided",
        "the provided code snippets",
        "here is an analysis",
        "this code appears",
    ];
    if meta_prefixes.iter().any(|p| lower.starts_with(p))
        || lower.contains("here is an analysis")
    {
        return true;
    }

    // Domain-specific generic check for RNA-seq queries.
    let q_terms = crate::answer::context::focused_excerpt_query_terms_pub(query);
    let rnaseq_query_terms: HashSet<&str> = [
        "rna-seq", "rnaseq", "output", "outputs", "download", "browser",
    ]
    .iter()
    .copied()
    .collect();
    if !q_terms.is_empty() && answer.len() > 40 {
        let domain_terms = [
            "output_renders",
            "artifact_url",
            "manifest_url",
            "downloadrenderedartifact",
            "requestoutputrenderrecordwithpayload",
            "content-disposition",
            "results ready",
        ];
        if !domain_terms.iter().any(|t| lower.contains(t)) {
            let context_text: String = context.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n").to_lowercase();
            if domain_terms.iter().any(|t| context_text.contains(t)) {
                return true;
            }
        }
    }
    false
}

/// Check if answer has non-English markers — mirrors `answerer.py::answer_has_non_english_markers`.
pub fn answer_has_non_english_markers(answer: &str) -> bool {
    let normalized = format!(" {} ", answer.to_lowercase());

    // Check phrase markers.
    if NON_ENGLISH_PHRASES
        .iter()
        .any(|m| normalized.contains(m))
    {
        return true;
    }

    // Check word-level markers.
    let re = non_english_word_re();
    let words: HashSet<String> = re
        .find_iter(&normalized)
        .map(|m| m.as_str().to_string())
        .collect();

    NON_ENGLISH_MARKER_SETS
        .iter()
        .any(|markers| {
            let matching = markers
                .iter()
                .filter(|m| words.contains(*m))
                .count();
            matching >= 2
        })
}

fn non_english_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-zàèéìòùáéíóúñç']+").unwrap())
}

/// Check if answer has unsupported natural-language claims.
///
/// Mirrors `answerer.py::answer_has_unsupported_natural_claims`.
pub fn answer_has_unsupported_natural_claims(
    answer: &str,
    citations: &[CitationRef],
    context: &[PreparedChunk],
) -> bool {
    let support = normalize_support_text(&cited_support_text(citations, context));
    if support.is_empty() {
        return false;
    }
    for sentence in answer_sentences(answer) {
        let terms = natural_claim_terms(&sentence);
        if terms.is_empty() {
            continue;
        }
        let risky: Vec<&str> = terms
            .iter()
            .filter(|t| HIGH_RISK_CLAIM_TERMS.contains(&t.as_str()))
            .copied()
            .collect();
        if !risky.is_empty() && !risky.iter().all(|t| support.contains(t)) {
            return true;
        }
        let supported_count = terms.iter().filter(|t| support.contains(t.as_str())).count();
        if terms.len() >= 7 && supported_count < (2).max(terms.len() / 3) {
            return true;
        }
    }
    false
}

/// Check if answer has unsupported grounding terms.
///
/// Mirrors `answerer.py::answer_has_unsupported_grounding_terms`.
pub fn answer_has_unsupported_grounding_terms(
    answer: &str,
    _citations: &[CitationRef],
    context: &[PreparedChunk],
) -> bool {
    let terms = answer_grounding_terms(answer);
    if terms.is_empty() {
        return false;
    }
    // Ground against FULL retrieved context (not just cited subset).
    let support = normalize_support_text(
        &context
            .iter()
            .map(context_support_text)
            .collect::<Vec<_>>()
            .join(""),
    );
    let unsupported: Vec<&str> = terms
        .iter()
        .filter(|t| {
            let norm = normalize_grounding_term(t);
            !support.contains(&norm)
                && !support.contains(&norm.replace('/', "/"))
        })
        .copied()
        .collect();
    // Tolerance: allow up to 2 stray terms.
    unsupported.len() > 2
}

// ═══════════════════════════════════════════════════════════════════════════
// Support text helpers
// ═══════════════════════════════════════════════════════════════════════════

fn cited_support_text(citations: &[CitationRef], context: &[PreparedChunk]) -> String {
    let refs: HashSet<&str> = citations.iter().map(|c| c.ref_id.as_str()).collect();
    context
        .iter()
        .filter(|c| refs.contains(c.r#ref.as_str()))
        .map(context_support_text)
        .collect::<Vec<_>>()
        .join("")
}

fn context_support_text(item: &PreparedChunk) -> String {
    vec![
        item.file_source.clone(),
        item.chunk_id.clone(),
        item.text.clone(),
    ]
    .join("\n")
}

fn normalize_support_text(text: &str) -> String {
    let replaced = text.replace('\\', "/");
    let lower = replaced.to_lowercase();
    let re = whitespace_re();
    re.replace_all(&lower, " ").to_string()
}

fn whitespace_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s+").unwrap())
}

// ═══════════════════════════════════════════════════════════════════════════
// Grounding terms extraction
// ═══════════════════════════════════════════════════════════════════════════

/// Extract grounding terms from answer text.
///
/// Mirrors `answerer.py::answer_grounding_terms`.
pub fn answer_grounding_terms(answer: &str) -> Vec<String> {
    let mut terms: HashSet<String> = HashSet::new();

    // Backtick-delimited code spans.
    for cap in backtick_re().captures_iter(answer) {
        if let Some(m) = cap.get(1) {
            let value = m.as_str().trim();
            if !value.is_empty() {
                terms.insert(value.to_string());
                // Split the value into sub-tokens.
                for piece in grounding_piece_re().find_iter(value) {
                    terms.insert(piece.as_str().to_string());
                }
            }
        }
    }

    // File path extensions.
    for m in file_ext_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }

    // camelCase identifiers.
    for m in camel_case_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }

    // snake_case identifiers.
    for m in snake_case_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }

    // ALL_CAPS identifiers (>= 4 chars).
    for m in all_caps_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }

    // Normalize and filter.
    let common: HashSet<String> = COMMON_GROUNDED_TERMS
        .iter()
        .map(|s| s.to_string())
        .collect();
    terms
        .iter()
        .map(|t| normalize_grounding_term(t))
        .filter(|t| t.len() >= 3 && !common.contains(t.as_str()))
        .collect()
}

fn backtick_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`([^`]{2,120})`").unwrap())
}

fn grounding_piece_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9_./\\:\-]+").unwrap())
}

fn file_ext_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[\w./\\\-]+\.(?:rs|py|tsx|ts|jsx|js|mjs|md|json|toml|ya?ml)\b").unwrap()
    })
}

fn camel_case_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[a-z]+[A-Z][A-Za-z0-9]*\b").unwrap())
}

fn snake_case_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[a-z][a-z0-9]+_[a-z0-9_]+\b").unwrap())
}

fn all_caps_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[A-Z][A-Z0-9_]{3,}\b").unwrap())
}

fn normalize_grounding_term(term: &str) -> String {
    term.trim_matches(|c: char| c == '`' || c == '\'' || c == '"' || c == '.' || c == ','
        || c == ';' || c == ':' || c == '(' || c == ')' || c == '[' || c == ']'
        || c == '{' || c == '}' || c == ' ')
        .replace('\\', "/")
        .to_lowercase()
}

// ═══════════════════════════════════════════════════════════════════════════
// Sentence splitting and claim term extraction
// ═══════════════════════════════════════════════════════════════════════════

/// Split answer into sentences — mirrors `answerer.py::answer_sentences`.
pub fn answer_sentences(answer: &str) -> Vec<String> {
    let re = sentence_split_re();
    re.split(answer)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn sentence_split_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?<=[.!?])\s+").unwrap())
}

/// Extract natural claim terms from a sentence (excluding code spans).
///
/// Mirrors `answerer.py::natural_claim_terms`.
pub fn natural_claim_terms(sentence: &str) -> Vec<String> {
    // Remove code spans.
    let re = backtick_re();
    let without_code = re.replace_all(sentence, " ");
    let re = claim_token_re();
    let stop: HashSet<&str> = CLAIM_STOPWORDS.iter().copied().collect();
    re.find_iter(&without_code)
        .map(|m| m.as_str().to_string())
        .filter(|term| term.len() >= 3 && !stop.contains(term.as_str()))
        .collect()
}

fn claim_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-z0-9_\-]+").unwrap())
}

// ═══════════════════════════════════════════════════════════════════════════
// General helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Clean answer text — mirrors `answerer.py::clean_answer`.
pub fn clean_answer(value: &str) -> String {
    let text = value.trim().to_string();
    whitespace_re().replace_all(&text, " ").to_string()
}

/// Ensure answer starts with NOT_FOUND_PHRASE.
fn ensure_not_found_prefix(answer: &str) -> String {
    if answer.to_lowercase().starts_with(NOT_FOUND_PHRASE) {
        answer.to_string()
    } else {
        format!("{}: {}", NOT_FOUND_PHRASE, answer)
    }
}

/// Suggest a file path based on the query — mirrors `answerer.py::suggest_path`.
pub fn suggest_path(query: &str, context: &[PreparedChunk]) -> Option<String> {
    if let Some(first) = context.first() {
        if !first.file_source.is_empty() {
            return Some(first.file_source.clone());
        }
    }
    let q = query.to_lowercase();
    if q.contains("scaleway") || q.contains("gpu") || q.contains("serverless") {
        return Some("src-tauri/src/backend/ or Scaleway provider docs".to_string());
    }
    if q.contains("cloudflare") || q.contains("worker") {
        return Some("cloudflare/workers/ or worker source files".to_string());
    }
    if q.contains("oracle") || q.contains("mcp") {
        return Some("oracle/".to_string());
    }
    if q.contains("frontend") || q.contains("ui") || q.contains("view") {
        return Some("src/components/".to_string());
    }
    None
}

/// Parse a JSON response from the LLM — mirrors `answerer.py::parse_json_response`.
pub fn parse_json_response(raw: &str) -> ParsedAnswer {
    let text = raw.trim();
    if text.is_empty() {
        return ParsedAnswer::default();
    }

    // Try full parse.
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(obj) = parsed.as_object() {
            return json_value_to_parsed(obj);
        }
    }

    // Try extracting a JSON object from the text.
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end > start {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text[start..=end])
                {
                    if let Some(obj) = parsed.as_object() {
                        return json_value_to_parsed(obj);
                    }
                }
            }
        }
    }

    ParsedAnswer::default()
}

fn json_value_to_parsed(obj: &serde_json::Map<String, serde_json::Value>) -> ParsedAnswer {
    ParsedAnswer {
        answer: obj.get("answer").and_then(|v| v.as_str()).map(String::from),
        citations: obj
            .get("citations")
            .and_then(|v| v.as_array())
            .cloned(),
        not_found: obj.get("not_found").and_then(|v| v.as_bool()),
        suggested_path: obj
            .get("suggested_path")
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}
