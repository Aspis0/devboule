//! Deterministic context compaction for local model prompts (Plan v5 Phase B).
//! Zero-LLM: BM25 scoring ranks file blocks by relevance to the task; keep the
//! top-N within 70% of the model's context window. No embeddings, no API calls.

use std::collections::HashMap;

/// Standard Okapi BM25 params.
pub const BM25_K1: f64 = 1.2;
pub const BM25_B: f64 = 0.75;

/// Budget report after compaction.
#[derive(Debug, Clone, Default)]
pub struct CompactBudget {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub percent_saved: f64,
    pub files_kept: usize,
    pub files_trimmed: usize,
    pub findings_kept: usize,
    pub findings_trimmed: usize,
}

/// Rough token estimate: ~4 chars per token.
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// Truncate to a token budget on a UTF-8 char boundary.
pub fn truncate_to_token_budget(text: &str, budget_tokens: usize) -> String {
    let max_chars = budget_tokens * 4;
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut end = max_chars;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// BM25 score: how relevant `document` is to `query`, given corpus stats.
/// `avg_doc_len`, `doc_count`, and `term_dfs` come from the file corpus.
pub fn bm25_score(
    query: &str,
    document: &str,
    avg_doc_len: f64,
    doc_count: usize,
    term_dfs: &HashMap<String, usize>,
) -> f64 {
    let dl = document.split_whitespace().count() as f64;
    let avg = avg_doc_len.max(1.0);
    query
        .to_lowercase()
        .split_whitespace()
        .map(|qt| {
            let tf = document
                .to_lowercase()
                .split_whitespace()
                .filter(|t| *t == qt)
                .count() as f64;
            if tf == 0.0 {
                return 0.0;
            }
            let df = *term_dfs.get(&qt.to_string()).unwrap_or(&1) as f64;
            let idf = ((doc_count as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
            idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg))
        })
        .sum()
}

/// Compact an already-built mini prompt to fit within 70% of the model's context window.
/// Parses the prompt into blocks by the markers `build_mini_prompt` emits, keeps
/// system + task + hard constraints (immutable), scores file blocks via BM25 vs the
/// task, and keeps the top-N files within budget. Returns compacted prompt + budget.
///
/// `context_window`: the model's max tokens. `current_usage`: tokens already in the
/// conversation (0 for one-shot minis). Budget = context_window*70/100 - current_usage.
pub fn compact_built_prompt(
    prompt: &str,
    task_description: &str,
    context_window: usize,
    current_usage: usize,
) -> (String, CompactBudget) {
    let budget = (context_window * 70 / 100).saturating_sub(current_usage);
    let tokens_before = estimate_tokens(prompt);

    // Split into segments by the markers build_mini_prompt emits. We keep:
    //  - preamble (everything before "FILE SCOPE") — identity/skill — IMMUTABLE
    //  - hard constraints block ("HARD CONSTRAINTS" … up to next marker) — IMMUTABLE
    //  - task block ("TASK (do EXACTLY" … to end) — IMMUTABLE
    //  - file blocks (between "FILE SCOPE" and "HARD CONSTRAINTS") — COMPACTABLE via BM25
    let preamble_end = prompt.find("FILE SCOPE").unwrap_or(prompt.len());
    let preamble = &prompt[..preamble_end];

    let files_section_start = preamble_end;
    // Search for HARD CONSTRAINTS only within the file section (not the whole prompt),
    // so file content that happens to contain "HARD CONSTRAINTS" doesn't mislead us.
    let hard_start = prompt[files_section_start..]
        .find("HARD CONSTRAINTS")
        .map(|i| i + files_section_start)
        .unwrap_or(prompt.len());
    let files_section = &prompt[files_section_start..hard_start];

    // Search for TASK only within the section after hard constraints (not the whole prompt),
    // so file content that happens to contain "TASK (do EXACTLY" doesn't mislead us.
    let task_start = prompt[hard_start..]
        .find("TASK (do EXACTLY")
        .map(|i| i + hard_start)
        .unwrap_or(prompt.len());
    let task_block = &prompt[task_start..];

    // Extract individual file blocks from the files_section: each is "- <path>\n```\n<content>\n```\n"
    // We split on "\n- " after the "FILE SCOPE" header line.
    let mut file_blocks: Vec<(String, String)> = Vec::new();
    let header_end = files_section
        .find('\n')
        .map(|i| i + 1)
        .unwrap_or(files_section.len());
    let body = &files_section[header_end..];
    for chunk in body.split("\n- ") {
        let chunk = chunk.trim_start_matches("\n- ");
        if chunk.is_empty() {
            continue;
        }
        // First line is the path; rest (in ``` fences) is content.
        let nl = chunk.find('\n').unwrap_or(chunk.len());
        let path: String = chunk[..nl].trim().to_string();
        let content = if let Some(start) = chunk.find("```\n") {
            let after = &chunk[start + 4..];
            if let Some(end) = after.find("\n```") {
                after[..end].to_string()
            } else {
                chunk.to_string()
            }
        } else {
            String::new()
        };
        file_blocks.push((path, content));
    }

    // BM25 score each file vs the task description.
    let docs: Vec<Vec<String>> = file_blocks
        .iter()
        .map(|(_, c)| {
            c.to_lowercase()
                .split_whitespace()
                .map(String::from)
                .collect()
        })
        .collect();
    let doc_count = docs.len();
    let avg_len = if doc_count > 0 {
        docs.iter().map(|d| d.len()).sum::<usize>() as f64 / doc_count as f64
    } else {
        1.0
    };
    let mut term_dfs: HashMap<String, usize> = HashMap::new();
    for doc in &docs {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in doc {
            if seen.insert(t.as_str()) {
                *term_dfs.entry(t.clone()).or_insert(0) += 1;
            }
        }
    }
    let scores: Vec<f64> = docs
        .iter()
        .map(|d| {
            bm25_score(
                task_description,
                &d.join(" "),
                avg_len,
                doc_count,
                &term_dfs,
            )
        })
        .collect();

    // Immutable token cost (preamble + hard + task).
    let immutable_tokens = estimate_tokens(preamble)
        + estimate_tokens(&prompt[hard_start..task_start])
        + estimate_tokens(task_block);
    let mut remaining = budget.saturating_sub(immutable_tokens);

    // Keep top-N files by score within budget.
    // kept_files stores owned (path, content) pairs. Content is either the full
    // file text (if it fits) or the pre-truncated text (if it was truncated to fit).
    let mut order: Vec<usize> = (0..file_blocks.len()).collect();
    order.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept_files: Vec<(String, String)> = Vec::new();
    let mut file_trimmed = 0;
    // File block overhead: path line + markdown fences (~5 tokens = ~20 chars).
    // When truncating a file, account for this overhead so the total output
    // stays within budget.
    const FILE_BLOCK_OVERHEAD: usize = 20;

    for &i in &order {
        let tokens = estimate_tokens(&file_blocks[i].1);
        if tokens <= remaining {
            // File fits within budget — keep full content.
            kept_files.push((file_blocks[i].0.clone(), file_blocks[i].1.clone()));
            remaining = remaining.saturating_sub(tokens);
        } else {
            // File doesn't fit — truncate it to fit (even if it's the first/only
            // file, we must truncate or the output exceeds budget).
            // Account for file block overhead (path + fences) when computing
            // the truncation budget.
            let trunc_budget = remaining.saturating_sub(FILE_BLOCK_OVERHEAD);
            let t = truncate_to_token_budget(&file_blocks[i].1, trunc_budget);
            if !t.is_empty() && (kept_files.is_empty() || kept_files.len() < 20) {
                kept_files.push((file_blocks[i].0.clone(), t));
                remaining = 0;
            }
            // Do NOT increment file_trimmed here — the final line below
            // (bug 3: truncated files were double-counted) already counts
            // correctly: total files minus files kept = files trimmed.
        }
    }
    file_trimmed += file_blocks.len().saturating_sub(kept_files.len());

    // Assemble. kept_files contains (path, content) where content is
    // already either full or pre-truncated.
    let mut out = String::new();
    out.push_str(preamble);
    out.push_str("FILE SCOPE (operate on ONLY these files):\n");
    for (path, content) in &kept_files {
        // Account for the file block overhead: path line + markdown fences
        // (~5 tokens = ~20 chars). Subtract this from remaining before
        // emitting, so the total output stays within budget.
        let overhead = 5; // tokens for path + fences
        let content_tokens = estimate_tokens(content);
        let file_block_tokens = content_tokens + overhead;
        out.push_str("- ");
        out.push_str(path);
        out.push('\n');
        out.push_str("```\n");
        out.push_str(content);
        if !content.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");
        remaining = remaining.saturating_sub(file_block_tokens);
    }
    out.push('\n');
    // Hard constraints + everything between files and task (immutable).
    out.push_str(&prompt[hard_start..task_start]);
    out.push_str(task_block);

    let tokens_after = estimate_tokens(&out);
    (
        out,
        CompactBudget {
            tokens_before,
            tokens_after,
            percent_saved: if tokens_before > 0 {
                (tokens_before.saturating_sub(tokens_after)) as f64 / tokens_before as f64 * 100.0
            } else {
                0.0
            },
            files_kept: kept_files.len(),
            files_trimmed: file_trimmed,
            findings_kept: 0,
            findings_trimmed: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bm25_relevant_file_scores_higher() {
        let query = "user authentication login";
        let auth = "fn login() { authenticate user password token }";
        let css = "body { color: red; margin: 0; padding: 0; }";
        let mut dfs: HashMap<String, usize> = HashMap::new();
        // build df over a 2-doc corpus
        for term in [
            "fn",
            "login",
            "authenticate",
            "user",
            "password",
            "token",
            "body",
            "color",
            "red",
            "margin",
            "padding",
        ] {
            dfs.insert(term.into(), 1);
        }
        let avg = ((auth.split_whitespace().count() + css.split_whitespace().count()) as f64) / 2.0;
        let s_auth = bm25_score(query, auth, avg, 2, &dfs);
        let s_css = bm25_score(query, css, avg, 2, &dfs);
        assert!(s_auth > s_css, "auth ({s_auth}) must beat css ({s_css})");
    }

    #[test]
    fn estimate_tokens_rough() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn truncate_respects_char_boundary() {
        let s = "x".repeat(100);
        let t = truncate_to_token_budget(&s, 5); // 20 chars
        assert!(t.chars().count() <= 20);
        assert!(t.starts_with('x'));
    }

    #[test]
    fn compact_built_prompt_trims_to_budget() {
        // A prompt way over budget: huge file content.
        let huge = "fn big() { ".to_string() + &"x ".repeat(50_000) + "}";
        let prompt = format!(
            "You are a mini-coder.\n\nFILE SCOPE (operate on ONLY these files):\n- src/big.rs\n```\n{huge}\n```\n\nHARD CONSTRAINTS (safety — you MUST obey):\n- NEVER delete.\n\nTASK (do EXACTLY this, honoring all rules above):\nFix the bug.\n"
        );
        let (out, budget) = compact_built_prompt(&prompt, "Fix the bug.", 8_192, 0);
        assert!(
            estimate_tokens(&out) <= 8_192 * 70 / 100,
            "compacted {} tokens must be <= 70% of 8192",
            estimate_tokens(&out)
        );
        assert!(budget.percent_saved > 0.0, "must save something");
        // Task always survives
        assert!(
            out.contains("TASK (do EXACTLY"),
            "task block must survive compaction"
        );
        // Hard constraints always survive
        assert!(out.contains("HARD CONSTRAINTS"), "constraints must survive");
    }
}
