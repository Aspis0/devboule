//! Context preparation for the Oracle answer pipeline.
//!
//! Port of `answerer.py` context-preparation helpers: `prepared_context`,
//! `focused_excerpt`, `redact_secret_tokens`, superseded-context filter,
//! domain disambiguation filter.

use std::collections::HashSet;
use std::env;
use std::sync::OnceLock;

use regex::Regex;

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

pub const NOT_FOUND_PHRASE: &str = "not found in corpus";

/// Retrieval depth fed to the LLM — mirrors `answerer.py::MAX_PROMPT_CHUNKS`.
pub fn max_prompt_chunks() -> usize {
    env::var("ORACLE_ASK_MAX_CHUNKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
}

/// Max chars per chunk in the prompt — mirrors `answerer.py::MAX_CHARS_PER_CHUNK`.
pub fn max_chars_per_chunk() -> usize {
    env::var("ORACLE_ASK_MAX_CHARS_PER_CHUNK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2800)
}

/// Max chars for the final answer — mirrors `answerer.py::MAX_ANSWER_CHARS`.
pub fn max_answer_chars() -> usize {
    env::var("ORACLE_ASK_MAX_ANSWER_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3200)
}

/// Stopwords for focused_excerpt term extraction — byte-exact set from Python.
const EXCERPT_STOPWORDS: &[&str] = &[
    "about", "and", "are", "does", "for", "from", "how", "the", "this", "that", "what", "when",
    "where", "which", "with",
];

// ═══════════════════════════════════════════════════════════════════════════
// RawChunk — input format (matches Python context dicts)
// ═══════════════════════════════════════════════════════════════════════════

/// Intermediate chunk representation used by `prepared_context`.
///
/// Field names match the Python dict shape produced by `dump_golden.py` and
/// the query engine context payload.
#[derive(Debug, Clone, Default)]
pub struct RawChunk {
    pub chunk_id: String,
    pub file_source: String,
    pub chunk_index: Option<i64>,
    pub start_char: Option<i64>,
    pub end_char: Option<i64>,
    pub text: String,
    pub score: f64,
    pub retrieval: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
}

// ═══════════════════════════════════════════════════════════════════════════
// PreparedChunk — output of prepared_context, input to build_answer_prompt
// ═══════════════════════════════════════════════════════════════════════════

/// A context chunk prepared for prompt assembly.
#[derive(Debug, Clone)]
pub struct PreparedChunk {
    pub r#ref: String,
    pub chunk_id: String,
    pub file_source: String,
    pub chunk_index: Option<i64>,
    pub start_char: Option<i64>,
    pub end_char: Option<i64>,
    pub retrieval: String,
    pub score: f64,
    pub text: String,
    pub kind: String,
    pub symbol_name: String,
    pub signature: String,
    pub language: String,
    pub line_start: i64,
    pub line_end: i64,
}

// ═══════════════════════════════════════════════════════════════════════════
// prepared_context — main entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Prepare raw context chunks for the prompt.
///
/// Mirrors `answerer.py::prepared_context`:
/// 1. Take 2×MAX_PROMPT_CHUNKS candidates
/// 2. Drop superseded context
/// 3. Domain disambiguation filter
/// 4. Truncate to MAX_PROMPT_CHUNKS
/// 5. Per-chunk: assign ref, apply focused_excerpt
pub fn prepared_context(chunks: &[RawChunk], query: &str) -> Vec<PreparedChunk> {
    let max_chunks = max_prompt_chunks();
    let mut candidate_chunks: Vec<RawChunk> = chunks
        .iter()
        .take((max_chunks * 2).max(max_chunks))
        .cloned()
        .collect();

    // Drop superseded context.
    let current: Vec<RawChunk> = candidate_chunks
        .iter()
        .filter(|c| !is_superseded_context(c))
        .cloned()
        .collect();
    if !current.is_empty() {
        candidate_chunks = current;
    }

    // Domain disambiguation filter.
    candidate_chunks = filter_domain_context(candidate_chunks, query);

    let limit = max_chars_per_chunk();
    let mut prepared = Vec::new();
    for (index, chunk) in candidate_chunks.iter().take(max_chunks).enumerate() {
        let text = chunk.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let ref_label = format!("C{}", index + 1);
        prepared.push(PreparedChunk {
            r#ref: ref_label,
            chunk_id: chunk.chunk_id.clone(),
            file_source: chunk.file_source.clone(),
            chunk_index: chunk.chunk_index,
            start_char: chunk.start_char,
            end_char: chunk.end_char,
            retrieval: chunk.retrieval.clone(),
            score: chunk.score,
            text: focused_excerpt(&text, query, limit),
            kind: chunk.kind.clone(),
            symbol_name: chunk.symbol_name.clone(),
            signature: chunk.signature.clone(),
            language: chunk.language.clone(),
            line_start: chunk.line_start.unwrap_or(0),
            line_end: chunk.line_end.unwrap_or(0),
        });
    }
    prepared
}

// ═══════════════════════════════════════════════════════════════════════════
// Superseded context filter
// ═══════════════════════════════════════════════════════════════════════════

/// Check if a chunk is superseded — mirrors `answerer.py::is_superseded_context`.
pub fn is_superseded_context(chunk: &RawChunk) -> bool {
    let source = chunk_file_source(chunk).to_lowercase();
    let text = chunk.text.to_lowercase();
    let text_head = if text.len() > 1600 {
        &text[..1600]
    } else {
        &text
    };
    if text_head.contains("superseded") {
        return true;
    }
    if text_head.contains("no longer in production") {
        return true;
    }
    if text_head.contains("historical architecture") {
        return true;
    }
    if source.contains("/adr/") && text_head.contains("kept for the historical") {
        return true;
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// Domain disambiguation filter
// ═══════════════════════════════════════════════════════════════════════════

/// Domain disambiguation filter — mirrors `answerer.py::filter_domain_context`.
pub fn filter_domain_context(chunks: Vec<RawChunk>, query: &str) -> Vec<RawChunk> {
    let q = query.to_lowercase();

    if q.contains("orasis") {
        let orasis: Vec<RawChunk> = chunks
            .iter()
            .filter(|c| chunk_file_source(c).contains("/orasis/"))
            .cloned()
            .collect();
        if !orasis.is_empty() {
            return orasis;
        }
        return chunks;
    }

    if q.contains("biovision") {
        let direct: Vec<RawChunk> = chunks
            .iter()
            .filter(|c| !chunk_file_source(c).contains("/orasis/"))
            .cloned()
            .collect();
        if !direct.is_empty() {
            return direct;
        }
        return chunks;
    }

    let is_rnaseq = q.contains("rna-seq") || q.contains("rnaseq");
    let has_output_term = ["output", "result", "download", "browser", "release"]
        .iter()
        .any(|t| q.contains(t));
    if is_rnaseq && has_output_term {
        let implementation: Vec<RawChunk> = chunks
            .iter()
            .filter(|c| {
                let src = chunk_file_source(c);
                src.contains("/aspis-bio-rnaseq-api/src/")
                    || src.contains("/aspis-bio-website/public/")
            })
            .cloned()
            .collect();
        if !implementation.is_empty() {
            return implementation;
        }
        return chunks;
    }

    chunks
}

/// Extract the file source from a chunk, normalizing path separators.
fn chunk_file_source(chunk: &RawChunk) -> String {
    chunk.file_source.replace('\\', "/").to_lowercase()
}

// ═══════════════════════════════════════════════════════════════════════════
// focused_excerpt — windowed text extraction with query-term weighting
// ═══════════════════════════════════════════════════════════════════════════

/// Extract a focused excerpt from chunk text around query-term hits.
///
/// Port of `answerer.py::focused_excerpt`.  **Determinism note**: Python's
/// `query_terms()` returns a `set[str]` whose iteration order varies with
/// `PYTHONHASHSEED`; the per-term 40-position cap means the first iterated
/// term accumulates up to 40 positions while later terms get fewer.  Rust's
/// `HashSet` has a different (but stable within a binary) iteration order,
/// so the selected window may differ from Python.  The golden fixture disables
/// this function (MAX_CHARS_PER_CHUNK=100000) for byte-equality; see
/// `golden/README.md` deviation #1.
pub fn focused_excerpt(text: &str, query: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }

    let terms = excerpt_query_terms(query);
    if terms.is_empty() {
        return truncate_text(text, limit);
    }

    let lower = text.to_lowercase();
    let mut positions: Vec<usize> = Vec::new();

    for term in &terms {
        let mut start = 0;
        loop {
            if let Some(index) = lower[start..].find(term.as_str()) {
                let abs_index = start + index;
                positions.push(abs_index);
                start = abs_index + term.len();
                if positions.len() >= 40 {
                    break;
                }
            } else {
                break;
            }
        }
        if positions.len() >= 40 {
            break;
        }
    }

    if positions.is_empty() {
        return truncate_text(text, limit);
    }

    let mut best_start = 0usize;
    let mut best_score = -1i64;

    for &position in &positions {
        // Mirror Python exactly:
        // start = max(0, position - limit // 3)
        // end = min(len(text), start + limit)
        // start = max(0, end - limit)
        let py_start = 0_usize.max(position.saturating_sub(limit / 3));
        let py_end = text.len().min(py_start + limit);
        let py_start2 = 0_usize.max(py_end.saturating_sub(limit));

        let window = &lower[py_start2..py_end];
        let mut score: i64 = 0;
        for term in &terms {
            let count = window.matches(term.as_str()).count() as i64;
            score += count * term_weight(term) as i64;
        }
        if score > best_score {
            best_score = score;
            best_start = py_start2;
        }
    }

    let excerpt_end = (best_start + limit).min(text.len());
    let mut result = text[best_start..excerpt_end].trim().to_string();

    if best_start > 0 {
        result = format!("[excerpt starts mid-chunk]\n{}", result);
    }
    if best_start + limit < text.len() {
        result = format!("{}\n[excerpt ends mid-chunk]", result);
    }
    result
}

/// Extract query terms for focused_excerpt — byte-exact port of Python.
fn excerpt_query_terms(query: &str) -> HashSet<String> {
    let re = excerpt_token_re();
    let lower = query.to_lowercase();
    re.find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .filter(|term| term.len() >= 3 && !EXCERPT_STOPWORDS.contains(&term.as_str()))
        .collect()
}

fn excerpt_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[a-z0-9_/-]+").unwrap())
}

/// Term weight for focused_excerpt scoring — byte-exact from Python.
fn term_weight(term: &str) -> i64 {
    if matches!(
        term,
        "gpu" | "min_scale" | "max_scale" | "scaleway" | "cloudflare" | "worker" | "workers"
    ) {
        3
    } else {
        1
    }
}

/// Truncate text with a marker — mirrors `answerer.py::truncate_text`.
pub fn truncate_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        text.to_string()
    } else {
        // Python: text[:limit].rstrip() + "\n[truncated]"
        let truncated = &text[..limit];
        let rstripped = truncated.trim_end();
        format!("{}\n[truncated]", rstripped)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// redact_secret_tokens — belt-and-suspenders secret redaction
// ═══════════════════════════════════════════════════════════════════════════

const SECRET_REDACTION: &str = "[redacted-secret]";

/// Redact secret-looking tokens in chunk text.
///
/// Port of `answerer.py::redact_secret_tokens` — every regex byte-exact.
pub fn redact_secret_tokens(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let mut redacted = text.to_string();

    // Apply all SECRET_PATTERNS in order.
    for pattern in secret_patterns() {
        redacted = pattern.replace_all(&redacted, SECRET_REDACTION).to_string();
    }

    // High-entropy base64 runs (40+ chars) with mixed character classes.
    // Collect positions first, then replace in reverse to avoid index shifts.
    let re = secret_high_entropy_re();
    let mut entropy_positions: Vec<(usize, usize)> = re
        .find_iter(&redacted)
        .map(|m| {
            let token = m.as_str();
            let has_lower = token.chars().any(|c| c.is_lowercase());
            let has_upper = token.chars().any(|c| c.is_uppercase());
            let has_digit = token.chars().any(|c| c.is_ascii_digit());
            let classes = has_lower as i32 + has_upper as i32 + has_digit as i32;
            if classes >= 2 {
                Some((m.start(), m.end()))
            } else {
                None
            }
        })
        .flatten()
        .collect();
    entropy_positions.reverse();
    for (start, end) in entropy_positions {
        redacted.replace_range(start..end, SECRET_REDACTION);
    }

    // Hex runs (40+ hex chars).
    let re = secret_hex_re();
    let hex_positions: Vec<(usize, usize)> = re
        .find_iter(&redacted)
        .map(|m| (m.start(), m.end()))
        .collect();
    let mut hex_positions = hex_positions;
    hex_positions.reverse();
    for (start, end) in hex_positions {
        redacted.replace_range(start..end, SECRET_REDACTION);
    }

    redacted
}

/// Compile-time secret patterns — byte-exact regexes from Python.
fn secret_patterns() -> Vec<Regex> {
    vec![
        // GitHub-style tokens (ghp_, gho_, ghu_, ghs_, ghr_, github_pat_...).
        Regex::new(r"\bgh[opusr]_[A-Za-z0-9]{20,}\b").unwrap(),
        Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b").unwrap(),
        // Scaleway secret keys / access keys (SCW...).
        Regex::new(r"\bSCW[A-Za-z0-9]{12,}\b").unwrap(),
        // AWS-style access key ids.
        Regex::new(r"\bAKIA[0-9A-Z]{12,}\b").unwrap(),
        // Slack / xoxb-style tokens.
        Regex::new(r"\bxox[baprs]-[A-Za-z0-9\-]{10,}\b").unwrap(),
        // Bearer-shaped authorization values.
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}").unwrap(),
        // JWT-shaped strings (three base64url segments).
        Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b").unwrap(),
        // Generic key=value secret assignments.
        Regex::new(r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|access[_-]?key)\b\s*[:=]\s*['"]?[A-Za-z0-9/_+\-]{16,}['"]?"#).unwrap(),
    ]
}

fn secret_high_entropy_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[A-Za-z0-9+/]{40,}={0,2}\b").unwrap())
}

fn secret_hex_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[0-9a-fA-F]{40,}\b").unwrap())
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

// ═══════════════════════════════════════════════════════════════════════════
// Public helpers used by guardrails.rs and extractive.rs
// ═══════════════════════════════════════════════════════════════════════════

/// Public re-export of excerpt query terms as a HashSet for guardrails/extractive.
pub fn excerpt_query_terms_set(query: &str) -> HashSet<String> {
    excerpt_query_terms(query)
}

/// Public re-export of term_weight for extractive.rs best_sentence scoring.
pub fn term_weight_pub(term: &str) -> i64 {
    term_weight(term)
}

/// Alias used by guardrails.rs.
pub use excerpt_query_terms_set as focused_excerpt_query_terms_pub;
