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
    let mut file_blocks: Vec<(String, String)> = Vec::new();
    let header_end = files_section
        .find('\n')
        .map(|i| i + 1)
        .unwrap_or(files_section.len());
    let body = &files_section[header_end..];
    // Parse line-based (blocker 4): a "- <path>" line at a line START, optionally
    // followed by a ```-fenced content block. A "- " line INSIDE a file's fenced content
    // (markdown bullets, YAML lists) is consumed within the fence, so it never starts a
    // new block. An UNFENCED "- path" line — a not-yet-existing file, a path-only entry
    // beyond MAX_PROMPT_FILES, or a rejected path — yields EMPTY content and must NOT
    // swallow the next file's content: we never break the loop on a missing fence, and
    // we never scan forward across a subsequent path line for a fence.
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let path = match line.strip_prefix("- ") {
            Some(p) => p.trim().to_string(),
            None => continue,
        };
        let mut content = String::new();
        // Capture fenced content ONLY if the very next line opens a fence.
        if lines
            .peek()
            .map(|l| l.trim_start().starts_with("```"))
            .unwrap_or(false)
        {
            lines.next(); // consume the opening fence line
            for cl in lines.by_ref() {
                if cl.trim_start().starts_with("```") {
                    break; // closing fence
                }
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(cl);
            }
        }
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

    // Safety guard: if immutable parts (preamble + hard constraints + task)
    // alone exceed the budget, truncate the preamble to the remaining budget
    // while keeping the hard constraints and task block intact.
    // The file section + hard constraints + task block is the minimum viable
    // output; if that still exceeds budget, we cannot drop the task — that
    // is the caller's pre-spawn size gate's job.
    // Compute the token cost of everything except the preamble.
    // Slice: everything between preamble and (hard constraints + task block).
    // Guard against underflow when there's no file section (e.g., empty files).
    let non_preamble_len = prompt[hard_start..task_start].len() + task_block.len();
    let file_section = if out.len() > non_preamble_len {
        &out[preamble.len()..out.len() - non_preamble_len]
    } else {
        ""
    };
    let hard_task_tokens = estimate_tokens(&prompt[hard_start..task_start]) + estimate_tokens(task_block);
    let file_section_tokens = estimate_tokens(file_section);
    let non_preamble_tokens = file_section_tokens + hard_task_tokens;
    // If the non-preamble portion alone exceeds budget, there's nothing to fix
    // (we can't drop the task); leave out as-is.
    if non_preamble_tokens <= budget {
        if estimate_tokens(&out) > budget {
            // Rebuild out with a truncated preamble (dropping it ENTIRELY when its
            // budget is 0 — build(0) yields "" safely), keeping the file section,
            // hard constraints and task block intact. NOTE: no `preamble_budget > 0`
            // guard — that edge (preamble_budget == 0) is exactly when the preamble
            // must be dropped hardest, and skipping it returned an over-budget prompt.
            let build = |pb: usize| {
                let mut fixed = String::new();
                fixed.push_str(&truncate_to_token_budget(preamble, pb));
                fixed.push_str(file_section);
                fixed.push_str(&prompt[hard_start..task_start]);
                fixed.push_str(task_block);
                fixed
            };
            let mut pb = budget.saturating_sub(non_preamble_tokens);
            let mut fixed = build(pb);
            // Integer len/4 estimates under-count the concatenated whole, so the
            // summed budget can overshoot by a few tokens. Shrink the preamble in a
            // bounded loop until it fits (converges in 1-2 passes; once pb hits 0 the
            // preamble is gone and any residual is the caller's pre-spawn gate's job).
            for _ in 0..4 {
                let after = estimate_tokens(&fixed);
                if after <= budget || pb == 0 {
                    break;
                }
                pb = pb.saturating_sub(after - budget);
                fixed = build(pb);
            }
            out = fixed;
        }
    }

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

    #[test]
    fn compact_built_prompt_truncates_preamble_when_over_budget() {
        // A huge preamble (e.g. a 100k-char system prompt) with a tiny context
        // window (4096). The immutable parts (preamble + hard + task) alone
        // exceed budget, so the preamble should be truncated to fit.
        let preamble = "You are a highly capable mini-coder. ".repeat(25_000);
        let prompt = format!(
            "{preamble}\n\nFILE SCOPE (operate on ONLY these files):\n\nHARD CONSTRAINTS (safety — you MUST obey):\n- NEVER delete.\n\nTASK (do EXACTLY this, honoring all rules above):\nFix the bug.\n"
        );
        let (out, budget) = compact_built_prompt(&prompt, "Fix the bug.", 4_096, 0);
        let budget_tokens = 4_096 * 70 / 100; // 2867
        let out_tokens = estimate_tokens(&out);
        // The preamble was huge (100k chars ≈ 25k tokens), so it should have
        // been truncated. The hard constraints + task block should still be
        // present.
        assert!(
            out_tokens <= budget_tokens,
            "compacted {} tokens must be <= 70% of 4096 ({}), got {}",
            out_tokens,
            budget_tokens,
            out_tokens
        );
        // Task and hard constraints always survive (even if preamble is dropped)
        assert!(
            out.contains("TASK (do EXACTLY"),
            "task block must survive compaction"
        );
        assert!(
            out.contains("HARD CONSTRAINTS"),
            "constraints must survive"
        );
        // The preamble should be truncated (shorter than original).
        assert!(
            out.len() < 100_000,
            "preamble should have been truncated (out len: {})",
            out.len()
        );
    }

    #[test]
    fn compact_built_prompt_fits_budget_when_immutable_near_full() {
        // The immutable task block nearly fills the whole budget, leaving almost no
        // room for the preamble — this drives preamble_budget to ~0 and exercises the
        // guard branch that the old `preamble_budget > 0` check wrongly skipped
        // (regression: it returned a prompt tens of thousands of tokens over budget).
        let preamble = "You are a highly capable mini-coder. ".repeat(25_000); // ~925k chars
        let big_task = "do the thing ".repeat(800); // ~10.4k chars ≈ 2600 tokens
        let prompt = format!(
            "{preamble}\n\nFILE SCOPE (operate on ONLY these files):\n\nHARD CONSTRAINTS (safety — you MUST obey):\n- NEVER delete.\n\nTASK (do EXACTLY this, honoring all rules above):\n{big_task}\n"
        );
        let (out, _budget) = compact_built_prompt(&prompt, "do the thing", 4_096, 0);
        let budget_tokens = 4_096 * 70 / 100; // 2867
        // MUST fit the budget — the whole point of the guard.
        assert!(
            estimate_tokens(&out) <= budget_tokens,
            "over budget: {} > {}",
            estimate_tokens(&out),
            budget_tokens
        );
        // The 925k-char preamble must have been (near-)fully dropped.
        assert!(out.len() < 50_000, "preamble not dropped: out len {}", out.len());
        // Task + hard constraints still survive.
        assert!(out.contains("TASK (do EXACTLY"), "task must survive");
        assert!(out.contains("HARD CONSTRAINTS"), "constraints must survive");
    }

    #[test]
    fn compact_preserves_file_with_bullet_lines_in_content() {
        // A file whose CONTENT contains markdown bullet lines must parse as ONE
        // block, not split at each "- " (blocker 4: the old `"\n- "` split leaked
        // bullet lines into bogus extra file blocks).
        let content = "fn x() {}\n- not a new file\n- still same file";
        let prompt = format!(
            "sys\n\nFILE SCOPE (operate on ONLY these files):\n- src/a.rs\n```\n{content}\n```\n\nHARD CONSTRAINTS (safety):\n- obey\n\nTASK (do EXACTLY this):\nfix\n"
        );
        // Large window → nothing trimmed, the file is kept whole.
        let (out, budget) = compact_built_prompt(&prompt, "fix", 200_000, 0);
        assert_eq!(budget.files_kept, 1, "must parse exactly one file block, not split on bullets");
        assert!(out.contains("- not a new file"), "bullet content line must survive");
        assert!(out.contains("- still same file"), "bullet content line must survive");
    }

    #[test]
    fn compact_unfenced_path_between_fenced_files_keeps_all() {
        // Review BLOCKER regression: an UNFENCED "- path" line (e.g. a not-yet-existing
        // "create this file" target) sitting BETWEEN two fenced files must NOT swallow
        // the following file's content or make it disappear.
        let prompt = "sys\n\nFILE SCOPE (operate on ONLY these files):\n\
            - src/a.rs\n```\nfn a() {}\n```\n\
            - src/new.rs\n\
            - src/z.rs\n```\nfn z() {}\n```\n\
            \nHARD CONSTRAINTS (safety):\n- obey\n\nTASK (do EXACTLY this):\nfix\n";
        let (out, budget) = compact_built_prompt(prompt, "fix", 200_000, 0);
        assert_eq!(budget.files_kept, 3, "all three paths must parse as blocks (unfenced included)");
        assert!(out.contains("src/a.rs"), "a path survives");
        assert!(out.contains("src/new.rs"), "unfenced new-file path survives");
        assert!(out.contains("src/z.rs"), "z path survives (the bug dropped it)");
        assert!(out.contains("fn a() {}"), "a content survives");
        assert!(out.contains("fn z() {}"), "z content survives");
    }
}
