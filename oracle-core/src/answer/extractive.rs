//! Extractive fallback chain and domain-specific extractive answers.
//!
//! Port of `answerer.py` extractive helpers and
//! `structural_synthesis.py::structural_extractive_answer`.

use crate::answer::context::{max_answer_chars, suggest_path, truncate_text, NOT_FOUND_PHRASE};
use crate::answer::{CitationRef, NormalizedAnswer, PreparedChunk};

// ═══════════════════════════════════════════════════════════════════════════
// Main extractive fallback chain
// ═══════════════════════════════════════════════════════════════════════════

/// Extractive answer fallback chain — exact order from Python:
/// 1. Domain extractive (5 templates)
/// 2. Structural synthesis
/// 3. Generic best-sentence fallback
pub fn extractive_answer(
    query: &str,
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> NormalizedAnswer {
    if context.is_empty() {
        return not_found_answer(query, context, reason);
    }
    if let Some(domain) = domain_extractive_answer(query, context, reason) {
        return domain;
    }
    if let Some(structural) = structural_extractive_answer(query, context, reason) {
        return structural;
    }

    // Generic best-sentence fallback.
    let cite_count = 3.min(context.len());
    let citations: Vec<CitationRef> = context[..cite_count].iter().map(context_citation).collect();

    let mut excerpts: Vec<String> = Vec::new();
    for item in &context[..cite_count] {
        if let Some(excerpt) = best_sentence(&item.text, query) {
            excerpts.push(format!("{}: {}", item.file_source, excerpt));
        }
    }

    let body = if !excerpts.is_empty() {
        excerpts.join(" ")
    } else {
        let files: Vec<&str> = context[..cite_count]
            .iter()
            .map(|c| c.file_source.as_str())
            .collect();
        format!(
            "The best matching Oracle context is in {}.",
            files.join(", ")
        )
    };

    let mut prefix =
        "Oracle found relevant code evidence, but the answer model could not produce a complete grounded response."
            .to_string();
    if let Some(r) = reason {
        prefix = format!("{} {}.", prefix, r);
    }

    NormalizedAnswer {
        answer: truncate_text(
            &format!("{} Best evidence: {}", prefix, body),
            max_answer_chars(),
        ),
        citations,
        not_found: false,
        suggested_path: None,
        answer_source: Some("extractive_fallback".to_string()),
        fallback_reason: reason.map(String::from),
        llm_provider: None,
        llm_model: None,
    }
}

/// Not-found answer.
pub fn not_found_answer(
    query: &str,
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> NormalizedAnswer {
    let suffix = match reason {
        Some(r) => format!(": {}", r),
        None => String::new(),
    };
    NormalizedAnswer {
        answer: format!("{}{}.", NOT_FOUND_PHRASE, suffix),
        citations: vec![],
        not_found: true,
        suggested_path: suggest_path(query, context),
        answer_source: Some("not_found".to_string()),
        fallback_reason: None,
        llm_provider: None,
        llm_model: None,
    }
}

/// Build a citation from a PreparedChunk.
pub fn context_citation(item: &PreparedChunk) -> CitationRef {
    CitationRef {
        ref_id: item.r#ref.clone(),
        file_source: item.file_source.clone(),
        chunk_id: item.chunk_id.clone(),
        chunk_index: item.chunk_index,
        start_char: item.start_char,
        end_char: item.end_char,
        retrieval: item.retrieval.clone(),
        score: item.score,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Domain extractive answers (5 hardcoded templates)
// ═══════════════════════════════════════════════════════════════════════════

pub fn domain_extractive_answer(
    query: &str,
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> Option<NormalizedAnswer> {
    let q = query.to_lowercase();

    if (q.contains("rna-seq") || q.contains("rnaseq"))
        && [
            "output", "outputs", "result", "results", "download", "release",
        ]
        .iter()
        .any(|t| q.contains(t))
    {
        if let Some(a) = rnaseq_output_extractive_answer(context, reason) {
            return Some(a);
        }
    }
    if q.contains("scaleway")
        && [
            "paid",
            "cleanup",
            "stop",
            "stops",
            "terminate",
            "terminal",
            "job",
            "resource",
            "resources",
        ]
        .iter()
        .any(|t| q.contains(t))
    {
        if let Some(a) = scaleway_cleanup_extractive_answer(context, reason) {
            return Some(a);
        }
    }
    if ["agent", "agents", "terminal", "cli"]
        .iter()
        .any(|t| q.contains(t))
        && ["project", "task", "status", "finished", "done"]
            .iter()
            .any(|t| q.contains(t))
    {
        if let Some(a) = agent_project_extractive_answer(context, reason) {
            return Some(a);
        }
    }
    if q.contains("oracle")
        && [
            "privacy",
            "safe",
            "zdr",
            "gdpr",
            "provider",
            "providers",
            "llm",
            "answers",
        ]
        .iter()
        .any(|t| q.contains(t))
    {
        if let Some(a) = oracle_privacy_extractive_answer(context, reason) {
            return Some(a);
        }
    }
    if q.contains("windows")
        && ["hello", "webcam", "camera", "unlock", "pin", "loop"]
            .iter()
            .any(|t| q.contains(t))
    {
        if let Some(a) = windows_hello_extractive_answer(context, reason) {
            return Some(a);
        }
    }
    None
}

fn combined_text(context: &[PreparedChunk]) -> String {
    context
        .iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find the first context chunk containing any of the given needles.
pub fn find_context_ref<'a>(
    context: &'a [PreparedChunk],
    needles: &[&str],
) -> Option<&'a PreparedChunk> {
    for needle in needles {
        for item in context {
            if item.text.to_lowercase().contains(needle) {
                return Some(item);
            }
        }
    }
    None
}

fn unique_context_refs(items: Vec<&PreparedChunk>) -> Vec<&PreparedChunk> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.chunk_id.clone()))
        .collect()
}

// ── RNA-seq output ─────────────────────────────────────────────────────

fn rnaseq_output_extractive_answer(
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> Option<NormalizedAnswer> {
    let combined = combined_text(context).to_lowercase();
    let required = ["output_renders", "artifact_url", "manifest_url"];
    if !required.iter().all(|t| combined.contains(t)) {
        return None;
    }
    let done_ref = find_context_ref(
        context,
        &["results ready", "status === \"done\"", "status: \"done\""],
    );
    let request_ref = find_context_ref(
        context,
        &[
            "requestoutputrenderrecordwithpayload",
            "outputs_not_ready",
            "createoutputrenderrecord",
            "enqueueoutputrender",
        ],
    );
    let callback_ref = find_context_ref(
        context,
        &[
            "syncoutputrenderrecordtojob",
            "normalizeoutputrenderstatuspayload",
            "status: \"ready\"",
            "manifest_url",
        ],
    );
    let download_ref = find_context_ref(
        context,
        &[
            "downloadrenderedartifact",
            "content-disposition",
            "registeredartifactisdownloadable",
        ],
    );
    let refs: Vec<&PreparedChunk> = [done_ref, request_ref, callback_ref, download_ref]
        .into_iter()
        .flatten()
        .collect();
    let refs = unique_context_refs(refs);
    if refs.len() < 2 {
        return None;
    }
    let answer = "After a successful RNA-seq run, the Worker reaches status `done`, sets `providerMessage` to \"Results ready\", and merges sanitized `output_renders` into the job. The browser-side render request goes through `requestOutputRenderRecordWithPayload`: it rejects non-`done` jobs with `outputs_not_ready`, reuses an existing ready render when possible, or creates/enqueues a new output render record. The signed render callback then stores `status: \"ready\"`, `artifact_url`, and `manifest_url`, and `syncOutputRenderRecordToJob` writes the render back to the indexed job. Actual download is served by `downloadRenderedArtifact`, which verifies the artifact is registered/downloadable, fetches the Scaleway object, and returns it with `Content-Disposition: attachment`.";
    Some(NormalizedAnswer {
        answer: truncate_text(answer, max_answer_chars()),
        citations: refs.iter().map(|r| context_citation(r)).collect(),
        not_found: false,
        suggested_path: None,
        answer_source: Some("extractive_synthesis".to_string()),
        fallback_reason: reason.map(String::from),
        llm_provider: None,
        llm_model: None,
    })
}

// ── Scaleway cleanup ───────────────────────────────────────────────────

fn scaleway_cleanup_extractive_answer(
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> Option<NormalizedAnswer> {
    let combined = combined_text(context).to_lowercase();
    if !(combined.contains("terminatescalewayinstance")
        && combined.contains("releasescalewayinstanceslot"))
    {
        return None;
    }
    let cleanup_ref = find_context_ref(
        context,
        &["cleanupscalewayinstanceafterterminal", "terminal"],
    );
    let terminate_ref = find_context_ref(
        context,
        &["terminatescalewayinstance", "delete", "with_volumes=all"],
    );
    let release_ref = find_context_ref(
        context,
        &[
            "releasescalewayinstanceslot",
            "scaleway_instance_active_key",
        ],
    );
    let refs: Vec<&PreparedChunk> = [cleanup_ref, terminate_ref, release_ref]
        .into_iter()
        .flatten()
        .collect();
    let refs = unique_context_refs(refs);
    if refs.is_empty() {
        return None;
    }
    let answer = "Paid Scaleway compute cleanup is implemented in `aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs`. `cleanupScalewayInstanceAfterTerminal` handles terminal/job cleanup, `terminateScalewayInstance` deletes the instance when termination is required, and `releaseScalewayInstanceSlot` clears the active instance slot so a paid VM is not kept reserved. The same provider code also handles related cleanup signals such as `with_volumes=all`, `delete`, and orphan-volume checks.";
    Some(NormalizedAnswer {
        answer: truncate_text(answer, max_answer_chars()),
        citations: refs.iter().map(|r| context_citation(r)).collect(),
        not_found: false,
        suggested_path: None,
        answer_source: Some("extractive_synthesis".to_string()),
        fallback_reason: reason.map(String::from),
        llm_provider: None,
        llm_model: None,
    })
}

// ── Agent project ──────────────────────────────────────────────────────

fn agent_project_extractive_answer(
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> Option<NormalizedAnswer> {
    let combined = combined_text(context).to_lowercase();
    if !(combined.contains("project_claim_task") && combined.contains("project_update_status")) {
        return None;
    }
    let read_ref = find_context_ref(
        context,
        &[
            "project_get",
            "project_list",
            "oracle_context",
            "oracle_ask",
        ],
    );
    let claim_ref = find_context_ref(context, &["project_claim_task"]);
    let update_ref = find_context_ref(context, &["project_update_status"]);
    let refs: Vec<&PreparedChunk> = [read_ref, claim_ref, update_ref]
        .into_iter()
        .flatten()
        .collect();
    let refs = unique_context_refs(refs);
    if refs.is_empty() {
        return None;
    }
    let answer = "Terminal agents interact through the local MCP tools in `oracle/server/aspis_mcp.py`, not by manually moving the React UI. They can read the project state with `project_list`/`project_get` and retrieve architecture context with `oracle_ask` or `oracle_context`. When work starts they call `project_claim_task`; when it is finished, blocked, or needs review they call `project_update_status`, which rewrites the project markdown state that the Projects UI reads.";
    Some(NormalizedAnswer {
        answer: truncate_text(answer, max_answer_chars()),
        citations: refs.iter().map(|r| context_citation(r)).collect(),
        not_found: false,
        suggested_path: None,
        answer_source: Some("extractive_synthesis".to_string()),
        fallback_reason: reason.map(String::from),
        llm_provider: None,
        llm_model: None,
    })
}

// ── Oracle privacy ─────────────────────────────────────────────────────

fn oracle_privacy_extractive_answer(
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> Option<NormalizedAnswer> {
    let combined = combined_text(context).to_lowercase();
    if !(combined.contains("openai")
        || combined.contains("openrouter")
        || combined.contains("deepseek"))
    {
        return None;
    }
    if !(combined.contains("zdr")
        || combined.contains("gdpr")
        || combined.contains("allowlisted")
        || combined.contains("provider not allowlisted"))
    {
        return None;
    }
    let vault_ref = find_context_ref(
        context,
        &[
            "allow only",
            "openai",
            "openrouter",
            "deepseek",
            "oracle_llm",
        ],
    );
    let answerer_ref = find_context_ref(
        context,
        &[
            "remote oracle llm provider is not allowlisted",
            "allowlisted",
            "allowed_hosts",
        ],
    );
    let graph_ref = find_context_ref(context, &["provider", "base_url", "deepseek"]);
    let refs: Vec<&PreparedChunk> = [vault_ref, answerer_ref, graph_ref]
        .into_iter()
        .flatten()
        .collect();
    let refs = unique_context_refs(refs);
    if refs.is_empty() {
        return None;
    }
    let answer = "The privacy gate is the provider allowlist, enforced in two places. The app settings/vault code restricts Oracle LLM providers to `openai`, `openrouter`, and `deepseek` (remote, OpenAI-compatible) plus local `omlx`/`ollama` (loopback-only), and the Rust oracle-core answerer rejects any remote provider outside that set. Remote calls require a saved API key and an HTTPS base URL that passes an SSRF guard (no loopback/IP-literal/intranet/metadata hosts); the endpoint host is NOT pinned per provider. When no key is configured Oracle returns an extractive, retrieval-only answer.";
    Some(NormalizedAnswer {
        answer: truncate_text(answer, max_answer_chars()),
        citations: refs.iter().map(|r| context_citation(r)).collect(),
        not_found: false,
        suggested_path: None,
        answer_source: Some("extractive_synthesis".to_string()),
        fallback_reason: reason.map(String::from),
        llm_provider: None,
        llm_model: None,
    })
}

// ── Windows Hello ──────────────────────────────────────────────────────

fn windows_hello_extractive_answer(
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> Option<NormalizedAnswer> {
    let combined = combined_text(context).to_lowercase();
    if !(combined.contains("windows hello") && combined.contains("unlock")) {
        return None;
    }
    let auth_ref = find_context_ref(
        context,
        &["windows hello", "unlock", "credential", "biometric"],
    );
    let state_ref = find_context_ref(context, &["cooldown", "retry", "unlock"]);
    let refs: Vec<&PreparedChunk> = [auth_ref, state_ref].into_iter().flatten().collect();
    let refs = unique_context_refs(refs);
    if refs.is_empty() {
        return None;
    }
    let answer = "Windows Hello unlock is controlled by the native auth/backend path and the locked-screen React flow. `src-tauri/src/backend/auth.rs` performs the Windows Hello/PIN/biometric unlock work, while the app state and locked screen gate repeated prompts with retry/cooldown state so webcam unlock cannot immediately reopen in a loop after a failed or cancelled attempt.";
    Some(NormalizedAnswer {
        answer: truncate_text(answer, max_answer_chars()),
        citations: refs.iter().map(|r| context_citation(r)).collect(),
        not_found: false,
        suggested_path: None,
        answer_source: Some("extractive_synthesis".to_string()),
        fallback_reason: reason.map(String::from),
        llm_provider: None,
        llm_model: None,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Structural synthesis (from structural_synthesis.py)
// ═══════════════════════════════════════════════════════════════════════════

/// Structural extractive answer grouped by file.
pub fn structural_extractive_answer(
    _query: &str,
    context: &[PreparedChunk],
    reason: Option<&str>,
) -> Option<NormalizedAnswer> {
    if context.is_empty() {
        return None;
    }
    // Group chunks by file_source (preserving insertion order).
    let mut by_file: Vec<(String, Vec<&PreparedChunk>)> = Vec::new();
    let mut file_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for chunk in context {
        let file_source = if chunk.file_source.is_empty() {
            "unknown".to_string()
        } else {
            chunk.file_source.clone()
        };
        if let Some(&idx) = file_index.get(&file_source) {
            by_file[idx].1.push(chunk);
        } else {
            let idx = by_file.len();
            file_index.insert(file_source.clone(), idx);
            by_file.push((file_source, vec![chunk]));
        }
    }
    let mut blocks: Vec<String> = Vec::new();
    let mut all_citations: Vec<CitationRef> = Vec::new();
    for (file_path, chunks) in &by_file {
        let mut lines = vec![format!("\u{1F4C4} `{}`", file_path)];
        for chunk in chunks {
            let kind = if chunk.kind.is_empty() {
                "text_slice"
            } else {
                &chunk.kind
            };
            let symbol = &chunk.symbol_name;
            let sig = &chunk.signature;
            let text = chunk.text.trim();
            let symbol_line = if !symbol.is_empty() {
                let mut line = format!("  - **{}** ({})", symbol, kind);
                if !sig.is_empty() {
                    let sig_preview: String = sig
                        .split('\n')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .chars()
                        .take(120)
                        .collect();
                    line = format!("{}: `{}`", line, sig_preview);
                }
                line
            } else {
                let mut first_line = String::new();
                for line_text in text.lines().take(3) {
                    let stripped = line_text.trim();
                    if !stripped.is_empty()
                        && !stripped.starts_with("//")
                        && !stripped.starts_with('#')
                        && !stripped.starts_with('*')
                        && !stripped.starts_with("/*")
                    {
                        first_line = stripped.chars().take(120).collect();
                        break;
                    }
                }
                if first_line.is_empty() {
                    format!("  - ({})", kind)
                } else {
                    format!("  - ({}): `{}`", kind, first_line)
                }
            };
            lines.push(symbol_line);
            all_citations.push(context_citation(chunk));
        }
        blocks.push(lines.join("\n"));
    }
    let body = blocks.join("\n\n");
    let _file_count = by_file.len();
    let mut symbols: Vec<String> = Vec::new();
    for (_, chunks) in &by_file {
        for c in chunks {
            if !c.symbol_name.is_empty() && !symbols.contains(&c.symbol_name) {
                symbols.push(c.symbol_name.clone());
            }
        }
    }
    // Python's structural_extractive_answer also emits a "summary" key, but
    // ask() IGNORES it (query_engine.py:186 builds summary from
    // generated["answer"]), so it never reaches any consumer — not ported.
    Some(NormalizedAnswer {
        answer: truncate_text(&body, max_answer_chars()),
        citations: all_citations,
        not_found: false,
        suggested_path: None,
        answer_source: Some("extractive_synthesis".to_string()),
        // Python: `reason or "structural_extractive"` — empty string is falsy.
        fallback_reason: Some(
            reason
                .filter(|r| !r.is_empty())
                .unwrap_or("structural_extractive")
                .to_string(),
        ),
        llm_provider: None,
        llm_model: None,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Generic best-sentence extraction
// ═══════════════════════════════════════════════════════════════════════════

/// Extract the best sentence from chunk text for the query.
pub fn best_sentence(text: &str, query: &str) -> Option<String> {
    let cleaned = sentence_ws_re().replace_all(text, " ").trim().to_string();
    if cleaned.is_empty() {
        return None;
    }
    let terms = crate::answer::context::excerpt_query_terms_set(query);
    // Python: re.split(r"(?<=[.!?])\s+|\n+", cleaned) — punctuation KEPT.
    let split = crate::answer::guardrails::split_after_sentence_punct(&cleaned, true);
    let candidates: Vec<&str> = split
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if candidates.is_empty() {
        return Some(truncate_text(&cleaned, 260));
    }
    if terms.is_empty() {
        return Some(truncate_text(candidates[0], 260));
    }
    let best = candidates
        .iter()
        .max_by_key(|&&sentence| {
            let lower = sentence.to_lowercase();
            terms
                .iter()
                .map(|term| {
                    lower.matches(term.as_str()).count() as i64
                        * crate::answer::context::term_weight_pub(term)
                })
                .sum::<i64>()
        })
        .copied()
        .unwrap_or(candidates[0]);
    Some(truncate_text(best, 260))
}

fn sentence_ws_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\s+").unwrap())
}
