//! Prompt assembly for the Oracle answer pipeline.
//!
//! Port of `answerer.py::build_answer_prompt` — the instruction text is
//! byte-exact.

use crate::answer::context::{redact_secret_tokens, PreparedChunk, NOT_FOUND_PHRASE};

/// Build the answer prompt from a query and prepared context chunks.
///
/// Mirrors `answerer.py::build_answer_prompt` — every sentence byte-exact.
pub fn build_answer_prompt(query: &str, context: &[PreparedChunk]) -> String {
    let blocks: Vec<String> = context
        .iter()
        .map(|item| {
            let chunk_index_str = match item.chunk_index {
                Some(v) => v.to_string(),
                None => "null".to_string(),
            };
            let start = item.start_char.unwrap_or(0);
            let end = item.end_char.unwrap_or(0);
            let redacted_text = redact_secret_tokens(&item.text);
            vec![
                format!("[{}]", item.r#ref),
                format!("file_source: {}", item.file_source),
                format!("chunk_id: {}", item.chunk_id),
                format!("chunk_index: {}", chunk_index_str),
                format!("location: chars {}-{}", start, end),
                "text:".to_string(),
                redacted_text,
            ]
            .join("\n")
        })
        .collect();

    let context_text = blocks.join("\n\n---\n\n");

    // Byte-exact prompt template from Python.
    format!(
        r#"You are Devboule Architecture Oracle.
Answer the user using ONLY the context chunks below.
Always answer in English, even if the user query is in another language.
Keep the answer short: at most 5 sentences.
Directly answer the user question; do not introduce the answer as "analysis", "provided code snippets", or similar meta-commentary.
Every factual claim must be supported by one or more provided chunk refs.
Do not use external knowledge. Do not invent paths, files, services, commands, or behavior.
If the user asks which file(s), name the exact file_source path(s) present in context.
For implementation/control questions, prefer source-code chunks over broad planning docs when both are relevant.
For process questions, explain the control flow and include exact function, route, field, and status names that appear in context.
Include at least three exact code symbols, route fragments, field names, or status values from the context when they are relevant.
Do not copy JSON objects from the context as your final answer; always return the answer wrapper object below.
If the context does not contain the answer, set not_found=true and make answer start with "{NOT_FOUND_PHRASE}".

Return strict JSON only, with this shape:
{{
  "answer": "short grounded answer",
  "citations": [{{"ref": "C1"}}],
  "not_found": false,
  "suggested_path": null
}}

User query:
{query}

Context chunks:
{context_text}
"#
    )
}
