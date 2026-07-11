//! Answer normalization and guardrails.
//!
//! Port of `answerer.py` guardrail checks: non-English rejection, too-generic
//! detection, unsupported-claims/grounding-term checks, citation validation.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::answer::context::{max_answer_chars, truncate_text, NOT_FOUND_PHRASE};
use crate::answer::extractive::{domain_extractive_answer, extractive_answer};
use crate::answer::{CitationRef, NormalizedAnswer, PreparedChunk};

// ═══════════════════════════════════════════════════════════════════════════
// Constants (byte-exact from Python)
// ═══════════════════════════════════════════════════════════════════════════

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

const NON_ENGLISH_MARKER_SETS: &[&[&str]] = &[
    &[
        "risposta", "forniti", "fornito", "codice", "agenti", "questo", "questa", "usando",
        "evita", "limita", "sono", "perche", "perché",
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
        "réponse", "reponse", "fichier", "agents", "tâche", "tache", "état", "etat", "utilise",
        "depuis", "parce", "sans",
    ],
];

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

const CLAIM_STOPWORDS: &[&str] = &[
    "about", "after", "also", "and", "are", "before", "both", "but", "can", "does", "for", "from",
    "into", "that", "the", "then", "they", "this", "through", "when", "where", "which", "with",
];

const COMMON_GROUNDED_TERMS: &[&str] = &[
    "api", "app", "cpu", "gpu", "http", "https", "json", "llm", "mcp", "oracle", "ui", "url", "vm",
];

// ═══════════════════════════════════════════════════════════════════════════
// ParsedAnswer — shape returned by parse_json_response
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Default, Clone)]
pub struct ParsedAnswer {
    pub answer: Option<String>,
    pub citations: Option<Vec<serde_json::Value>>,
    pub not_found: Option<bool>,
}

// ═══════════════════════════════════════════════════════════════════════════
// normalize_answer — main guardrail entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Normalize and validate a parsed LLM answer.
pub fn normalize_answer(
    query: &str,
    parsed: &ParsedAnswer,
    context: &[PreparedChunk],
) -> NormalizedAnswer {
    let answer_text = clean_answer(parsed.answer.as_deref().unwrap_or(""));
    let parsed_not_found = parsed.not_found.unwrap_or(false);
    let not_found_in_answer = answer_text.to_lowercase().contains(NOT_FOUND_PHRASE);
    let not_found = parsed_not_found || not_found_in_answer;

    if answer_text.is_empty() {
        let ea = extractive_answer(query, context, Some("LLM returned empty or invalid JSON"));
        return ea;
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
        let suggested = crate::answer::context::suggest_path(query, context);
        return NormalizedAnswer {
            answer: ensure_not_found_prefix(&answer_text),
            citations: vec![],
            not_found: true,
            suggested_path: suggested,
            answer_source: Some("not_found".to_string()),
            fallback_reason: None,
            llm_provider: None,
            llm_model: None,
        };
    }

    let citations = normalize_citations(parsed.citations.as_deref().unwrap_or(&[]), context);
    if citations.is_empty() {
        return extractive_answer(query, context, Some("LLM returned no valid citations"));
    }
    if answer_is_too_generic(query, &answer_text, context) {
        return extractive_answer(query, context, Some("LLM returned a generic answer"));
    }
    if answer_has_non_english_markers(&answer_text) {
        return extractive_answer(query, context, Some("LLM returned a non-English answer"));
    }
    if answer_has_unsupported_natural_claims(&answer_text, &citations, context) {
        return extractive_answer(
            query,
            context,
            Some("LLM answer included unsupported natural-language claims"),
        );
    }
    if answer_has_unsupported_grounding_terms(&answer_text, &citations, context) {
        return extractive_answer(
            query,
            context,
            Some("LLM answer included unsupported identifiers or paths"),
        );
    }

    NormalizedAnswer {
        answer: truncate_text(&answer_text, max_answer_chars()),
        citations,
        not_found: false,
        suggested_path: None,
        answer_source: Some("llm".to_string()),
        fallback_reason: None,
        llm_provider: None,
        llm_model: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Citation normalization
// ═══════════════════════════════════════════════════════════════════════════

pub fn normalize_citations(
    raw_citations: &[serde_json::Value],
    context: &[PreparedChunk],
) -> Vec<CitationRef> {
    let by_ref: HashSet<&str> = context.iter().map(|c| c.r#ref.as_str()).collect();
    let mut citations = Vec::new();
    let mut seen = HashSet::new();

    for raw in raw_citations {
        let ref_id = match raw {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(map) => {
                // Python: `raw.get("ref") or raw.get("source_ref")` — a null
                // or EMPTY "ref" falls through to source_ref (falsy chain).
                let non_empty_str = |v: &serde_json::Value| {
                    v.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                };
                let ref_val = map
                    .get("ref")
                    .and_then(non_empty_str)
                    .or_else(|| map.get("source_ref").and_then(non_empty_str));
                if let Some(ref r) = ref_val {
                    // Try chunk_id resolution.
                    if let Some(item) = context.iter().find(|c| c.chunk_id == *r) {
                        Some(item.r#ref.clone())
                    } else {
                        Some(r.clone())
                    }
                } else {
                    // Try chunk_id from the dict.
                    map.get("chunk_id")
                        .and_then(|v| v.as_str())
                        .and_then(|cid| context.iter().find(|c| c.chunk_id == cid))
                        .map(|c| c.r#ref.clone())
                }
            }
            _ => None,
        };
        let ref_id = match ref_id {
            Some(r) => r,
            None => continue,
        };
        if !by_ref.contains(ref_id.as_str()) {
            continue;
        }
        let item = match context.iter().find(|c| c.r#ref == ref_id) {
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

// ═══════════════════════════════════════════════════════════════════════════
// Guardrail checks
// ═══════════════════════════════════════════════════════════════════════════

/// Check if answer is too generic.
pub fn answer_is_too_generic(query: &str, answer: &str, context: &[PreparedChunk]) -> bool {
    let lower = answer.to_lowercase();
    let meta_prefixes = [
        "based on the provided",
        "the provided code snippets",
        "here is an analysis",
        "this code appears",
    ];
    if meta_prefixes.iter().any(|p| lower.starts_with(p)) || lower.contains("here is an analysis") {
        return true;
    }

    let q_terms = crate::answer::context::excerpt_query_terms_set(query);
    // Python gates this whole check on RNA-seq terms in the QUERY:
    // `({"rna-seq", ...} & q_terms) and len(answer) > 40`.
    let rnaseq_q: HashSet<&str> = [
        "rna-seq", "rnaseq", "output", "outputs", "download", "browser",
    ]
    .iter()
    .copied()
    .collect();
    let rnaseq_query = q_terms.iter().any(|t| rnaseq_q.contains(t.as_str()));
    if rnaseq_query && answer.len() > 40 {
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
            let context_text: String = context
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();
            if domain_terms.iter().any(|t| context_text.contains(t)) {
                return true;
            }
        }
    }
    false
}

/// Check if answer has non-English markers.
pub fn answer_has_non_english_markers(answer: &str) -> bool {
    let normalized = format!(" {} ", answer.to_lowercase());
    if NON_ENGLISH_PHRASES.iter().any(|m| normalized.contains(m)) {
        return true;
    }
    let re = non_english_word_re();
    let words: HashSet<String> = re
        .find_iter(&normalized)
        .map(|m| m.as_str().to_string())
        .collect();
    NON_ENGLISH_MARKER_SETS.iter().any(|markers| {
        markers
            .iter()
            .filter(|m| words.contains(*m as &str))
            .count()
            >= 2
    })
}

fn non_english_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-zàèéìòùáéíóúñç']+").unwrap())
}

/// Check if answer has unsupported natural-language claims.
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
            .map(|s| s.as_str())
            .collect();
        if !risky.is_empty() && !risky.iter().all(|t| support.contains(*t)) {
            return true;
        }
        let supported_count = terms
            .iter()
            .filter(|t| support.contains(t.as_str()))
            .count();
        if terms.len() >= 7 && supported_count < (2).max(terms.len() / 3) {
            return true;
        }
    }
    false
}

/// Check if answer has unsupported grounding terms.
pub fn answer_has_unsupported_grounding_terms(
    answer: &str,
    _citations: &[CitationRef],
    context: &[PreparedChunk],
) -> bool {
    let terms = answer_grounding_terms(answer);
    if terms.is_empty() {
        return false;
    }
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
            !support.contains(&norm) && !support.contains(&norm.replace('/', "/"))
        })
        .map(|s| s.as_str())
        .collect();
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
    whitespace_re().replace_all(&lower, " ").to_string()
}

fn whitespace_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s+").unwrap())
}

// ═══════════════════════════════════════════════════════════════════════════
// Grounding terms extraction
// ═══════════════════════════════════════════════════════════════════════════

pub fn answer_grounding_terms(answer: &str) -> Vec<String> {
    let mut terms: HashSet<String> = HashSet::new();
    for cap in backtick_re().captures_iter(answer) {
        if let Some(m) = cap.get(1) {
            let value = m.as_str().trim();
            if !value.is_empty() {
                terms.insert(value.to_string());
                for piece in grounding_piece_re().find_iter(value) {
                    terms.insert(piece.as_str().to_string());
                }
            }
        }
    }
    for m in file_ext_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }
    for m in camel_case_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }
    for m in snake_case_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }
    for m in all_caps_re().find_iter(answer) {
        terms.insert(m.as_str().to_string());
    }
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

pub fn normalize_grounding_term(term: &str) -> String {
    term.trim_matches(|c: char| {
        c == '`'
            || c == '\''
            || c == '"'
            || c == '.'
            || c == ','
            || c == ';'
            || c == ':'
            || c == '('
            || c == ')'
            || c == '['
            || c == ']'
            || c == '{'
            || c == '}'
            || c == ' '
    })
    .replace('\\', "/")
    .to_lowercase()
}

// ═══════════════════════════════════════════════════════════════════════════
// Sentence splitting and claim terms
// ═══════════════════════════════════════════════════════════════════════════

pub fn answer_sentences(answer: &str) -> Vec<String> {
    split_after_sentence_punct(answer, false)
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Python `re.split(r"(?<=[.!?])\s+", text)` (and, when
/// `split_newline_runs`, the `|\n+` alternative of `best_sentence`):
/// split at whitespace runs that FOLLOW sentence punctuation, KEEPING the
/// punctuation on the sentence — the `regex` crate has no lookbehind, so
/// this is a char-walk with identical semantics.
pub(crate) fn split_after_sentence_punct(text: &str, split_newline_runs: bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let after_punct = matches!(cur.chars().last(), Some('.' | '!' | '?'));
        if c.is_whitespace() && after_punct {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            parts.push(std::mem::take(&mut cur));
            continue;
        }
        if split_newline_runs && c == '\n' {
            while i < chars.len() && chars[i] == '\n' {
                i += 1;
            }
            parts.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(c);
        i += 1;
    }
    parts.push(cur);
    parts
}

pub fn natural_claim_terms(sentence: &str) -> Vec<String> {
    let without_code = backtick_re().replace_all(sentence, " ");
    let stop: HashSet<&str> = CLAIM_STOPWORDS.iter().copied().collect();
    claim_token_re()
        .find_iter(&without_code)
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

pub fn clean_answer(value: &str) -> String {
    let text = value.trim().to_string();
    whitespace_re().replace_all(&text, " ").to_string()
}

fn ensure_not_found_prefix(answer: &str) -> String {
    if answer.to_lowercase().starts_with(NOT_FOUND_PHRASE) {
        answer.to_string()
    } else {
        format!("{}: {}", NOT_FOUND_PHRASE, answer)
    }
}

/// Parse a JSON response from the LLM.
pub fn parse_json_response(raw: &str) -> ParsedAnswer {
    let text = raw.trim();
    if text.is_empty() {
        return ParsedAnswer::default();
    }
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(obj) = parsed.as_object() {
            return json_value_to_parsed(obj);
        }
    }
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end > start {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text[start..=end]) {
                    if let Some(obj) = parsed.as_object() {
                        return json_value_to_parsed(obj);
                    }
                }
            }
        }
    }
    ParsedAnswer::default()
}

/// Python `bool(value)` over a JSON value.
fn json_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

fn json_value_to_parsed(obj: &serde_json::Map<String, serde_json::Value>) -> ParsedAnswer {
    ParsedAnswer {
        answer: obj.get("answer").and_then(|v| v.as_str()).map(String::from),
        citations: obj.get("citations").and_then(|v| v.as_array()).cloned(),
        // Python: `bool(parsed.get("not_found"))` — TRUTHINESS, not strict
        // bool: "false" (non-empty string) is TRUE, 0/""/null/[] are false.
        not_found: obj.get("not_found").map(json_truthy),
    }
}
